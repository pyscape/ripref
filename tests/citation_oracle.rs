//! Tier 3 grammar conformance: a differential test of `ripref::citation::decode`
//! against the *canonical AD-1 regex*, run via `scripts/citation_regex_oracle.py`.
//!
//! Zero Cargo dependency: the oracle is an external `python3` process, gated like
//! the repo's `git_available()` tests — absent the runner, the test skips rather
//! than fails. The Rust adversarial table is fed THROUGH the real regex, so the
//! hand-rolled decoder and the canonical regex cannot silently drift.
//!
//! Contract checked: anchored `re.fullmatch` (ACCEPT/REJECT for the whole token)
//! == `decode` returning `Citation` vs `Bare | Malformed`. The Bare-vs-Malformed
//! refinement is decode-internal and pinned by the Tier 1 in-crate table.

use std::io::Write;
use std::process::{Command, Stdio};

use ripref::citation::{decode, Decoded};

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
        "/scripts/citation_regex_oracle.py"
    );

    // The adversarial table (apples-to-apples through the real regex) ...
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
        "[[rr:a]]~deadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
        "[[rr:a]]@a1b2c3",
        "[[rr:a]]@aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "[[rr:a]]@",
        "[[rr:a]]@a1b2c3d!",
        "[[rr:a]]@a1b2c3z",
        "[[rr:a]]xyz",
        "[[rr:support@example.com]]@a1b2c3d",
        "[[rr:a\tb]]",
        "[[rr:a\\\tb]]",
        "[[rr:a\nb]]",
        "[[rr:a\\\nb]]",
        "[[rr:café 日本語 🦀]]",
        " [[rr:a]]",
        "[[rr:a]] ",
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

    // ... plus a seeded random corpus, biased toward the opener to stress the
    // body scan and the pin boundary. Min length 1 so no token is empty (an
    // empty trailing token would desync the NUL framing).
    let pool: Vec<char> = "[]\\@~rr: \t\n0123456789abcdefABCDEF.#"
        .chars()
        .chain(['é', '🦀'])
        .collect();
    let mut rng = Rng(0x0bad_c0de_1234_5678);
    for _ in 0..3000 {
        let len = 1 + (rng.next() % 30) as usize;
        let body: String = (0..len)
            .map(|_| pool[(rng.next() as usize) % pool.len()])
            .collect();
        corpus.push(if rng.next() & 1 == 0 {
            format!("[[rr:{body}")
        } else {
            body
        });
    }

    // NUL-framed because adversarial tokens contain newlines; NUL cannot appear
    // in a valid UTF-8 anchor.
    let input = corpus.join("\0");
    let mut child = Command::new(py)
        .arg(script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn the regex oracle");
    let mut stdin = child.stdin.take().unwrap();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(input.as_bytes());
        // drop closes stdin -> EOF for the oracle
    });
    let out = child
        .wait_with_output()
        .expect("oracle did not produce output");
    writer.join().unwrap();
    assert!(
        out.status.success(),
        "oracle process failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8(out.stdout).expect("oracle stdout is utf-8");
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines.len(),
        corpus.len(),
        "oracle emitted {} verdicts for {} tokens",
        lines.len(),
        corpus.len()
    );

    let mut diverged = 0usize;
    for (tok, line) in corpus.iter().zip(lines) {
        let oracle_accepts = line.starts_with("ACCEPT");
        let rust_accepts = matches!(decode(tok), Decoded::Citation(_));
        if oracle_accepts != rust_accepts {
            diverged += 1;
            eprintln!(
                "DIVERGENCE on {tok:?}: oracle={oracle_accepts} ({line:?}), decode={:?}",
                decode(tok)
            );
        }
    }
    assert_eq!(
        diverged, 0,
        "{diverged} token(s) diverged from the canonical regex"
    );
}
