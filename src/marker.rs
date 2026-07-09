/*!
The `[[rr:...]]` marker grammar that `[[rr:AD-2]]` fixes.

An **anchor** is the bare token a reader takes on the CLI. A **marker** is the
delimited form written into a document:

```text
[[rr:<escaped-anchor>]]
```

This module EMITs the marker ([`wrap`]) and ACCEPTs one ([`decode`] for a whole
CLI token, [`scan_token`] for an occurrence inside text). It is std-only by
design: the opener is the fixed five bytes `[[rr:`, the terminator is the first
unescaped `]]`, and nothing follows the terminator, so a marker decodes offline
with no index. The canonical extraction regex `[[rr:AD-2]]` gives is the
conformance oracle (scripts/marker_regex_oracle.py), not a dependency: the
backslash-parity boundary is cleaner hand-rolled and matches the crate's
no-`regex` ethos.
*/

/// The five-byte opener that makes a marker findable and unambiguous.
pub const OPENER: &str = "[[rr:";

/// How [`decode`] interprets one reader CLI token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decoded {
    /// No `[[rr:` sentinel: an ordinary bare anchor. The caller owns it
    /// unchanged.
    Bare,
    /// A `[[rr:` sentinel that is NOT a well-formed marker (no terminator, an
    /// illegal raw byte or undefined escape in the body, or trailing text
    /// after `]]`). The string is the human-facing reason.
    Malformed(String),
    /// A well-formed marker: the unescaped anchor it delimits.
    Marker(String),
}

/// One occurrence parsed from the front of a text slice that begins with
/// [`OPENER`], for the scanners.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Token {
    /// A well-formed marker: the byte length consumed and the decoded anchor.
    Marker { len: usize, anchor: String },
    /// The opener is present but no well-formed marker follows.
    Malformed(String),
}

/// Wrap an anchor as the document marker `[[rr:<escaped>]]`.
///
/// Precondition: `anchor` contains no raw `\t`, `\r`, or `\n`. Those are
/// outside the grammar (a newline has no escape), and no extractor emits such
/// an anchor.
pub fn wrap(anchor: &str) -> String {
    format!("{OPENER}{}]]", escape(anchor))
}

/// Escape every literal `\`, `[`, and `]` so the body has exactly one
/// unescaped `]]` (its terminator). Uniform per-byte escaping, not just
/// escaping a `]]` run, is what lets an anchor ending in `]` (the key kind)
/// round-trip instead of silently truncating.
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

/// Decode one CLI token. Conforms to the `[[rr:AD-2]]` oracle regex
/// `\[\[rr:(?:\\[\\\[\]]|[^\\\]\[\t\n\r])*?\]\]` interpreted as an *anchored*
/// match:
/// the whole token must be the marker. A `[[rr:` sentinel followed by trailing
/// junk is [`Decoded::Malformed`], not a partial match; the scanners use
/// [`scan_token`], the same parse *unanchored*, to find markers inside text.
pub fn decode(token: &str) -> Decoded {
    if !token.starts_with(OPENER) {
        return Decoded::Bare;
    }
    match scan_token(token) {
        Token::Marker { len, anchor } if len == token.len() => Decoded::Marker(anchor),
        Token::Marker { .. } => Decoded::Malformed("trailing text after ]] in marker".to_string()),
        Token::Malformed(why) => Decoded::Malformed(why),
    }
}

/// Parse one marker from the front of `s`, which must begin with [`OPENER`].
/// The body ends at the first `]]` whose first `]` is unescaped; the returned
/// anchor is unescaped (the normative strip-then-unescape decode of
/// `[[rr:AD-2]]`). Raw `\t`/`\r`/`\n` cannot occur in a body, so a marker
/// always sits on one line; the escapes are exactly `\[`, `\]`, and `\\`, and
/// any other escape makes the token malformed.
pub fn scan_token(s: &str) -> Token {
    debug_assert!(s.starts_with(OPENER));
    let body = &s[OPENER.len()..];
    let mut anchor = String::new();
    let mut chars = body.char_indices();
    loop {
        let Some((i, c)) = chars.next() else {
            return Token::Malformed("unterminated [[rr: marker".to_string());
        };
        match c {
            '\\' => match chars.next() {
                None => return Token::Malformed("marker ends in a dangling backslash".to_string()),
                Some((_, n @ ('\\' | '[' | ']'))) => anchor.push(n),
                Some(_) => {
                    return Token::Malformed(
                        "undefined escape in marker (only \\[ \\] \\\\ exist)".to_string(),
                    )
                }
            },
            ']' => {
                // The terminator is the first `]]` whose first `]` is
                // unescaped. Peek a clone so a non-terminator `]` is not
                // consumed.
                let mut peek = chars.clone();
                if matches!(peek.next(), Some((_, ']'))) {
                    let len = OPENER.len() + i + 2;
                    return Token::Marker { len, anchor };
                }
                return Token::Malformed("unescaped ] in marker body".to_string());
            }
            '[' => return Token::Malformed("unescaped [ in marker body".to_string()),
            '\t' | '\r' | '\n' => {
                return Token::Malformed("control character in marker body".to_string())
            }
            c => anchor.push(c),
        }
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
        Marker(&'static str),
    }

    fn assert_decode(token: &str, exp: &Exp) {
        let got = decode(token);
        match (&got, exp) {
            (Decoded::Bare, Exp::Bare) => {}
            (Decoded::Malformed(_), Exp::Malformed) => {}
            (Decoded::Marker(a), Exp::Marker(want)) => {
                assert_eq!(a, want, "anchor mismatch for {token:?}");
            }
            _ => panic!("{token:?}: expected {exp:?}, got {got:?}"),
        }
    }

    // --- Tier 1: adversarial table (mirrors scripts/marker_regex_oracle.py
    // READER_TABLE; the Tier 3 differential test feeds these through the real
    // regex so the two can never silently drift). ---
    #[test]
    fn decode_adversarial_table() {
        let table: &[(&str, Exp)] = &[
            ("[[rr:a]]", Exp::Marker("a")),
            ("[[rr:]]", Exp::Marker("")),
            (r"[[rr:a\]]", Exp::Malformed), // odd backslash eats the first ]
            (r"[[rr:a\\]]", Exp::Marker(r"a\")), // even backslash -> terminator
            (r"[[rr:a\]]]", Exp::Marker("a]")),
            ("[[rr:arr[0]]]", Exp::Malformed), // raw [ and ] must be escaped
            (r"[[rr:arr\[0\]]]", Exp::Marker("arr[0]")),
            ("[[rr:a]b]]", Exp::Malformed), // lone unescaped ] mid-body
            (r"[[rr:a\]b]]", Exp::Marker("a]b")),
            ("[[rr:foo[[rr:bar]]", Exp::Malformed), // raw nested sentinel
            (r"[[rr:foo\[\[rr:bar]]", Exp::Marker("foo[[rr:bar")),
            // Nothing follows the terminator: a suffix is trailing junk.
            ("[[rr:a]]@a1b2c3d", Exp::Malformed),
            ("[[rr:a]]~deadbee", Exp::Malformed),
            ("[[rr:a]]xyz", Exp::Malformed),
            ("[[rr:a]] ", Exp::Malformed),
            // Escapes are exactly \[ \] \\; anything else is undefined.
            (r"[[rr:a\zb]]", Exp::Malformed),
            ("[[rr:a\\\tb]]", Exp::Malformed),
            ("[[rr:a\tb]]", Exp::Malformed), // raw TAB in body
            ("[[rr:a\nb]]", Exp::Malformed), // raw LF
            ("[[rr:café 日本語 🦀]]", Exp::Marker("café 日本語 🦀")), // UTF-8
            (" [[rr:a]]", Exp::Bare),        // leading space: not a marker token
            ("[[foo]]", Exp::Bare),          // wrong sentinel
            ("[[RR:a]]", Exp::Bare),         // case-sensitive
            ("[[rr:a", Exp::Malformed),      // unterminated
            (r"[[rr:a\\\]]", Exp::Malformed), // \\ + \] -> no terminator
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

    #[test]
    fn scan_token_reports_consumed_length() {
        match scan_token("[[rr:a]] and more") {
            Token::Marker { len, anchor } => {
                assert_eq!(anchor, "a");
                assert_eq!(len, "[[rr:a]]".len());
            }
            other => panic!("expected a marker, got {other:?}"),
        }
    }

    // --- Tier 2: properties (no oracle needed; seeded xorshift, no `rand`). ---

    /// Deterministic xorshift64* so the property runs are reproducible without
    /// a PRNG dependency. Seed must be nonzero.
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
    fn round_trip_holds_for_random_anchors() {
        // A charset heavy on the dangerous bytes, plus multibyte chars. No raw
        // control chars: those are outside the grammar (wrap's precondition).
        let charset: Vec<char> = r#"\[]:@~#. abcAB12"#.chars().chain("é日🦀".chars()).collect();
        let mut rng = Rng::new(0x1234_5678_9abc_def1);
        for _ in 0..2000 {
            let len = (rng.next() % 14) as usize;
            let anchor: String = (0..len).map(|_| *rng.pick(&charset)).collect();
            match decode(&wrap(&anchor)) {
                Decoded::Marker(a) => assert_eq!(a, anchor, "round-trip broke for {anchor:?}"),
                other => panic!("wrap/decode broke for {anchor:?}: {other:?}"),
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
            // Bias half the corpus to start with the opener, to stress the
            // body scan.
            let s = if rng.next() & 1 == 0 {
                format!("{OPENER}{s}")
            } else {
                s
            };
            let _ = decode(&s);
        }
    }
}
