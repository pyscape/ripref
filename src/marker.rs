/*!
The `[[rr:...]]` marker grammar that `[[rr:AD-2]]` fixes.

An **anchor** is the bare token a reader takes on the CLI. A **marker** is the
delimited form written into a document:

```text
[[rr:<escaped-anchor>]]
```

This module EMITs the marker ([`wrap`]). It is std-only by design: the opener
is the fixed five bytes `[[rr:`, and writers escape the structural bytes so the
first unescaped `]]` always terminates.
*/

/// The five-byte opener that makes a marker findable and unambiguous.
pub const OPENER: &str = "[[rr:";

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
