/*!
The `[[rr:...]]` citation grammar that `[[rr:AD-1]]` specifies
(`doc/adr/0001-citation-syntax.md`). (Dogfood: per `[[rr:AD-1]]` rule R1 a
citation in prose is backtick-wrapped, which both neutralizes rendering and
stays findable as a code-span citation.)

An **anchor (address)** is the bare token a reader/writer takes on the CLI. A
**citation** is the delimited form written into a document:

```text
[[rr:<escaped-anchor>]]            live reference
[[rr:<escaped-anchor>]]@<commit>   snapshot pin
[[rr:<escaped-anchor>]]~<commit>   tracking pin
```

This module EMITs the marker ([`wrap`]) and ACCEPTs a pasted one ([`decode`]).
It is std-only by design: the opener is the fixed five bytes `[[rr:`, the
terminator is the first unescaped `]]`, and any pin sits *outside* the brackets,
so a marker decodes offline with no index and no known-anchor-wins — the whole
point of `[[rr:AD-1]]`. The canonical extraction regex `[[rr:AD-1]]` gives is the
conformance oracle (`[[rr:scripts/citation_regex_oracle.py]]`), not a dependency:
the backslash-parity boundary is cleaner hand-rolled and matches the crate's
no-`regex` ethos. The reader this feeds is `[[rr:run_read]]` in
`[[rr:src/commands.rs]]`; the pin kind is `[[rr:Sigil]]` from `[[rr:src/cli.rs]]`.
*/

use crate::cli::Sigil;

/// The five-byte opener that makes a citation findable and unambiguous.
const OPENER: &str = "[[rr:";

/// A fully-decoded citation. Owned, because unescaping the body produces a fresh
/// `String` that must outlive dispatch (`[[rr:Reference]]` only borrows).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Citation {
    /// The unescaped anchor (the address the marker delimits).
    pub anchor: String,
    /// The pin attached outside `]]`, if any: a `<7..40 hex>` commit.
    pub pin: Option<(Sigil, String)>,
}

/// How [`decode`] interprets one reader CLI token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decoded {
    /// No `[[rr:` sentinel: an ordinary bare token. The caller's existing
    /// bare-anchor / `anchor@commit` path owns it unchanged.
    Bare,
    /// A `[[rr:` sentinel that is NOT a well-formed citation (no terminator, an
    /// illegal raw byte in the body, or a present-but-invalid pin). A usage
    /// error: the user clearly meant a citation, so do not silently fall back to
    /// bare parsing. The string is the human-facing reason.
    Malformed(String),
    /// A well-formed citation.
    Citation(Citation),
}

/// Wrap an anchor as the document citation marker `[[rr:<escaped>]]`.
///
/// Precondition: `anchor` contains no raw `\t`, `\r`, or `\n`. Those are outside
/// the citable grammar (the oracle excludes them and a newline has no escape),
/// and no extractor emits such an anchor — `cite`/`track` already refuse
/// tab/newline anchors. A pin, when wanted, is appended by the caller *outside*
/// the returned string (`format!("{}@{short}", wrap(anchor))`).
pub fn wrap(anchor: &str) -> String {
    format!("{OPENER}{}]]", escape(anchor))
}

/// Escape every literal `\`, `[`, and `]` so the body has exactly one unescaped
/// `]]` (its terminator). Uniform per-byte escaping — not just escaping a `]]`
/// run — is what lets an anchor ending in `]` (the manifest-key kind) round-trip
/// instead of silently truncating.
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + OPENER.len() + 2);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '[' => out.push_str("\\["),
            ']' => out.push_str("\\]"),
            c => out.push(c),
        }
    }
    out
}

/// Decode one CLI token. Conforms to the `[[rr:AD-1]]` oracle regex
/// `\[\[rr:(?:\\.|[^\\\]\[\t\n\r])*?\]\](?:[@~][0-9a-fA-F]{7,40})?` interpreted
/// as an *anchored* match (the whole token must be the citation): a `[[rr:`
/// sentinel followed by trailing junk, or a malformed pin, is [`Decoded::Malformed`],
/// not a partial match. (The future `rr search` scanner uses the same regex
/// *unanchored* to find citations inside text; that is a deliberately laxer
/// contract.)
pub fn decode(token: &str) -> Decoded {
    let Some(body) = token.strip_prefix(OPENER) else {
        return Decoded::Bare;
    };

    let mut anchor = String::with_capacity(body.len());
    let mut chars = body.chars();
    loop {
        let Some(c) = chars.next() else {
            return Decoded::Malformed("unterminated [[rr: citation".to_string());
        };
        match c {
            '\\' => match chars.next() {
                None => {
                    return Decoded::Malformed("citation ends in a dangling backslash".to_string())
                }
                // The oracle's `\\.` matches any escaped char except a raw LF.
                Some('\n') => return Decoded::Malformed("escaped newline in citation".to_string()),
                Some(n) => anchor.push(n),
            },
            ']' => {
                // The terminator is the first `]]` whose first `]` is unescaped.
                // Peek a clone so a non-terminator `]` is not consumed.
                let mut peek = chars.clone();
                if peek.next() == Some(']') {
                    chars = peek;
                    break;
                }
                return Decoded::Malformed("unescaped ] in citation body".to_string());
            }
            '[' => return Decoded::Malformed("unescaped [ in citation body".to_string()),
            '\t' | '\r' | '\n' => {
                return Decoded::Malformed("control character in citation body".to_string())
            }
            c => anchor.push(c),
        }
    }

    match parse_pin(chars.as_str()) {
        Ok(pin) => Decoded::Citation(Citation { anchor, pin }),
        Err(why) => Decoded::Malformed(why),
    }
}

/// Parse the suffix after `]]`: empty (live), `@<7..40 hex>` (snapshot), or
/// `~<7..40 hex>` (tracking). Anything else is malformed — the reader is strict
/// about a pin a user wrote, rather than silently dropping it.
fn parse_pin(suffix: &str) -> Result<Option<(Sigil, String)>, String> {
    let mut it = suffix.chars();
    let sigil = match it.next() {
        None => return Ok(None),
        Some('@') => Sigil::Snapshot,
        Some('~') => Sigil::Tracking,
        Some(_) => return Err("trailing text after ]] in citation".to_string()),
    };
    let hex = it.as_str();
    if (7..=40).contains(&hex.len()) && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(Some((sigil, hex.to_string())))
    } else {
        Err("invalid pin: expected [@~]<7..40 hex> after ]]".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Expected outcome for a decode table row.
    #[derive(Debug)]
    enum Exp {
        Bare,
        Malformed,
        Plain(&'static str),
        Pinned(&'static str, Sigil, &'static str),
    }

    fn assert_decode(token: &str, exp: &Exp) {
        let got = decode(token);
        match (&got, exp) {
            (Decoded::Bare, Exp::Bare) => {}
            (Decoded::Malformed(_), Exp::Malformed) => {}
            (Decoded::Citation(c), Exp::Plain(a)) => {
                assert_eq!(c.anchor, *a, "anchor mismatch for {token:?}");
                assert!(
                    c.pin.is_none(),
                    "expected no pin for {token:?}, got {:?}",
                    c.pin
                );
            }
            (Decoded::Citation(c), Exp::Pinned(a, sig, commit)) => {
                assert_eq!(c.anchor, *a, "anchor mismatch for {token:?}");
                assert_eq!(
                    c.pin,
                    Some((*sig, commit.to_string())),
                    "pin mismatch for {token:?}"
                );
            }
            _ => panic!("{token:?}: expected {exp:?}, got {got:?}"),
        }
    }

    // --- Tier 1: adversarial table (mirrors scripts/citation_regex_oracle.py
    // READER_TABLE; the Tier 3 differential test feeds these through the real
    // regex so the two can never silently drift). ---
    #[test]
    fn decode_adversarial_table() {
        use Sigil::{Snapshot, Tracking};
        let table: &[(&str, Exp)] = &[
            ("[[rr:a]]", Exp::Plain("a")),
            ("[[rr:]]", Exp::Plain("")),
            (r"[[rr:a\]]", Exp::Malformed), // odd backslash eats the first ]
            (r"[[rr:a\\]]", Exp::Plain(r"a\")), // even backslash -> terminator
            (r"[[rr:a\]]]", Exp::Plain("a]")),
            ("[[rr:arr[0]]]", Exp::Malformed), // raw [ and ] must be escaped
            (r"[[rr:arr\[0\]]]", Exp::Plain("arr[0]")),
            ("[[rr:a]b]]", Exp::Malformed), // lone unescaped ] mid-body
            (r"[[rr:a\]b]]", Exp::Plain("a]b")),
            ("[[rr:foo[[rr:bar]]", Exp::Malformed), // raw nested sentinel
            (r"[[rr:foo\[\[rr:bar]]", Exp::Plain("foo[[rr:bar")),
            ("[[rr:a]]@a1b2c3d", Exp::Pinned("a", Snapshot, "a1b2c3d")),
            (
                "[[rr:a]]~deadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
                Exp::Pinned("a", Tracking, "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef"),
            ),
            ("[[rr:a]]@a1b2c3", Exp::Malformed), // 6 hex too short
            (
                "[[rr:a]]@aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                Exp::Malformed,
            ), // 41 too long
            ("[[rr:a]]@", Exp::Malformed),       // empty pin
            ("[[rr:a]]@a1b2c3d!", Exp::Malformed), // trailing junk after pin
            ("[[rr:a]]@a1b2c3z", Exp::Malformed), // non-hex
            ("[[rr:a]]xyz", Exp::Malformed),     // trailing junk, no pin
            (
                "[[rr:support@example.com]]@a1b2c3d",
                Exp::Pinned("support@example.com", Snapshot, "a1b2c3d"),
            ),
            ("[[rr:a\tb]]", Exp::Malformed),       // raw TAB in body
            ("[[rr:a\\\tb]]", Exp::Plain("a\tb")), // escaped TAB accepted
            ("[[rr:a\nb]]", Exp::Malformed),       // raw LF
            ("[[rr:a\\\nb]]", Exp::Malformed),     // escaped LF not accepted
            ("[[rr:café 日本語 🦀]]", Exp::Plain("café 日本語 🦀")), // UTF-8
            (" [[rr:a]]", Exp::Bare),              // leading space: not a marker
            ("[[rr:a]] ", Exp::Malformed),         // trailing space
            ("[[foo]]", Exp::Bare),                // wrong sentinel
            ("[[RR:a]]", Exp::Bare),               // case-sensitive
            ("[[rr:a", Exp::Malformed),            // unterminated
            (r"[[rr:a\\\]]", Exp::Malformed),      // \\ + \] -> no terminator
            // Bare extras: ordinary anchors must pass through untouched.
            ("support@example.com", Exp::Bare),
            ("README.md", Exp::Bare),
            ("~/path", Exp::Bare),
            ("AD-42", Exp::Bare),
        ];
        for (token, exp) in table {
            assert_decode(token, exp);
        }
    }

    // --- Tier 2: properties (no oracle needed; seeded xorshift, no `rand`). ---

    /// Deterministic xorshift64* so the property runs are reproducible without a
    /// PRNG dependency. Seed must be nonzero.
    struct Rng(u64);
    impl Rng {
        fn new(seed: u64) -> Rng {
            Rng(seed)
        }
        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            x
        }
        fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
            &xs[(self.next() as usize) % xs.len()]
        }
    }

    #[test]
    fn round_trip_holds_for_random_citable_anchors() {
        // A charset heavy on the dangerous bytes, plus multibyte chars. No raw
        // control chars: those are outside the citable set (wrap's precondition).
        let charset: Vec<char> = r#"\[]:@~#. abcAB12"#.chars().chain("é日🦀".chars()).collect();
        let mut rng = Rng::new(0x1234_5678_9abc_def1);
        for _ in 0..2000 {
            let len = (rng.next() % 14) as usize;
            let anchor: String = (0..len).map(|_| *rng.pick(&charset)).collect();

            // Live round-trip.
            match decode(&wrap(&anchor)) {
                Decoded::Citation(Citation {
                    anchor: a,
                    pin: None,
                }) => {
                    assert_eq!(a, anchor, "live round-trip broke for {anchor:?}")
                }
                other => panic!("wrap/decode broke for {anchor:?}: {other:?}"),
            }
            // Pinned round-trip (pin appended outside the brackets).
            let hex = "a1b2c3d";
            match decode(&format!("{}@{hex}", wrap(&anchor))) {
                Decoded::Citation(Citation {
                    anchor: a,
                    pin: Some((Sigil::Snapshot, c)),
                }) => {
                    assert_eq!(a, anchor, "pinned round-trip anchor broke for {anchor:?}");
                    assert_eq!(c, hex, "pinned round-trip commit broke for {anchor:?}");
                }
                other => panic!("pinned wrap/decode broke for {anchor:?}: {other:?}"),
            }
        }
    }

    #[test]
    fn decode_never_panics_on_arbitrary_input() {
        // Pool stuffed with the structural bytes plus raw control chars and
        // multibyte chars; decode must always return, never panic or hang.
        let pool: Vec<char> = "[]\\@~rr: \t0123abcdef"
            .chars()
            .chain(['\n', '\r', 'é', '🦀', ']'])
            .collect();
        let mut rng = Rng::new(0xdead_beef_0bad_f00d);
        for _ in 0..5000 {
            let len = (rng.next() % 48) as usize;
            let s: String = (0..len).map(|_| *rng.pick(&pool)).collect();
            // Bias half the corpus to start with the opener, to stress the body scan.
            let s = if rng.next() & 1 == 0 {
                format!("{OPENER}{s}")
            } else {
                s
            };
            let _ = decode(&s);
        }
    }
}
