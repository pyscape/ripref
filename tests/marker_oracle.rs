//! Tier 3 grammar conformance: a differential test of `ripref::marker::decode`
//! against the *canonical AD-2 regex*, run via scripts/marker_regex_oracle.py.
//!
//! Zero Cargo dependency: the oracle is an external `python3` process, gated
//! like the repo's git-gated tests — absent the runner, the test skips rather
//! than fails. The Rust adversarial table is fed THROUGH the real regex, so
//! the hand-rolled decoder and the canonical regex cannot silently drift.
//!
//! Contract checked: anchored `re.fullmatch` (ACCEPT/REJECT for the whole
//! token) == `decode` returning `Marker` vs `Bare | Malformed`. The
//! Bare-vs-Malformed refinement is decode-internal and pinned by the Tier 1
//! in-crate table.

use std::io::Write;
use std::process::{Command, Stdio};

use ripref::marker::{decode, wrap, Decoded};

/// Locate a Python interpreter, or `None` to skip (mirrors `git_available()`).
fn python() -> Option<&'static str> {
    for cand in ["python3", "python"] {
        let ok = Command::new(cand)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok {
            return Some(cand);
        }
    }
    None
}

/// Deterministic xorshift64 so the fuzz corpus is reproducible without `rand`.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
}

#[test]
fn decode_matches_canonical_regex_oracle() {
    let Some(py) = python() else {
        eprintln!("skipping: python3 not available");
        return;
    };
    let script = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/scripts/marker_regex_oracle.py"
    );

    // The adversarial table (apples-to-apples through the real regex)...
    let mut corpus: Vec<String> = [
        "[[rr:a]]",
        "[[rr:]]",
        r"[[rr:a\]]",
        r"[[rr:a\\]]",
        r"[[rr:a\]]]",
        "[[rr:arr[0]]]",
        r"[[rr:arr\[0\]]]",
        "[[rr:a]b]]",
        r"[[rr:a\]b]]",
        "[[rr:foo[[rr:bar]]",
        r"[[rr:foo\[\[rr:bar]]",
        "[[rr:a]]@a1b2c3d",
        "[[rr:a]]~deadbee",
        "[[rr:a]]xyz",
        "[[rr:a]] ",
        r"[[rr:a\zb]]",
        "[[rr:a\\\tb]]",
        "[[rr:a\tb]]",
        "[[rr:a\nb]]",
        "[[rr:café 日本語 🦀]]",
        " [[rr:a]]",
        "[[foo]]",
        "[[RR:a]]",
        "[[rr:a",
        r"[[rr:a\\\]]",
        "support@example.com",
        "README.md",
        "~/path",
        "AD-42",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();

    // ...everything the emitter produces (must be ACCEPT)...
    for anchor in [
        "a",
        "",
        "a\\",
        "a]",
        "]]",
        "arr[0]",
        "pyproject.toml#[tool.poetry] name",
        "Index build: the writer",
        "support@example.com",
        "src/cli.rs#parse_reference",
        "AD-42",
    ] {
        corpus.push(wrap(anchor));
    }

    // ...plus a deterministic fuzz corpus biased toward the opener.
    let pool: Vec<char> = "[]\\@~rr: \t0123abcdef"
        .chars()
        .chain(['\n', 'é', '🦀', ']'])
        .collect();
    let mut rng = Rng(0x0dd0_11ce_5eed_cafe);
    for _ in 0..1500 {
        let len = (rng.next() % 40) as usize;
        let s: String = (0..len)
            .map(|_| pool[(rng.next() as usize) % pool.len()])
            .collect();
        let s = if rng.next() & 1 == 0 {
            format!("[[rr:{s}")
        } else {
            s
        };
        // NUL is the corpus separator, so it cannot appear in a token.
        corpus.push(s.replace('\0', ""));
    }

    // Pipe the corpus through the oracle, NUL-separated.
    let mut child = Command::new(py)
        .arg(script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn the oracle");
    {
        let stdin = child.stdin.as_mut().expect("oracle stdin");
        for tok in &corpus {
            stdin.write_all(tok.as_bytes()).unwrap();
            stdin.write_all(b"\0").unwrap();
        }
    }
    let out = child.wait_with_output().expect("oracle run");
    assert!(out.status.success(), "oracle exited nonzero");
    let verdicts: Vec<&str> = std::str::from_utf8(&out.stdout)
        .expect("oracle output is UTF-8")
        .lines()
        .collect();
    assert_eq!(verdicts.len(), corpus.len(), "one verdict per corpus token");

    for (tok, verdict) in corpus.iter().zip(verdicts) {
        let oracle_accepts = verdict.starts_with("ACCEPT");
        let decode_accepts = matches!(decode(tok), Decoded::Marker(_));
        assert_eq!(
            oracle_accepts, decode_accepts,
            "decode and the canonical regex diverge on {tok:?}: oracle said {verdict}"
        );
        if oracle_accepts {
            // The oracle echoes the matched span; an anchored ACCEPT must
            // cover the whole token.
            assert_eq!(
                verdict.split('\t').nth(1),
                Some(tok.as_str()),
                "oracle match must span the whole token for {tok:?}"
            );
        }
    }
}
