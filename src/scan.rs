/*!
Lexical scanners for markers and path mentions.

`search` and `verify` (`[[rr:AD-3]]`) run these over scoped text; `index`
runs the mention half to fill the mention table (`[[rr:AD-5]]`). The region
rules are `[[rr:AD-2]]`'s: in a Markdown host, prose and inline code spans
whose content begins with the marker opener are read and fenced blocks are
invisible; a structureless host is read per raw line. Mentions qualify only
in prose, and marker interiors are excluded from the mention scan.
*/

use crate::config::Config;
use crate::marker;

/// One scanner hit, located by 1-based line number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub line: u64,
    pub what: What,
}

/// What the scanners find.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum What {
    /// A well-formed marker: the raw bytes as written and the decoded anchor.
    Marker { raw: String, anchor: String },
    /// A `[[rr:` opener with no well-formed marker behind it.
    Malformed { reason: String },
    /// A path mention; `line_ref` is set when `:` and digits follow it (the
    /// bare `path:line` form).
    Mention { token: String, line_ref: bool },
}

/// The host structure a file exposes to the scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Host {
    /// Markdown regions: prose and qualifying inline spans; fences invisible.
    Markdown,
    /// A `[scan.<lang>]` comment host: the text after `syntax.line` per raw
    /// line is read as prose; everything else on the line is invisible.
    Comments(&'static CommentSyntax),
    /// No declared structure: every raw line is scanned.
    Plain,
}

/// The host the profile declares for a file extension: Markdown as ever;
/// otherwise a `[scan.<lang>]` entry that lists `"comments"` and whose
/// `COMMENT_SYNTAX` row claims the extension gives `Host::Comments`, and
/// anything else is `Host::Plain`.
pub fn host_for(ext: Option<&str>, cfg: &Config) -> Host {
    match ext {
        Some("md") | Some("markdown") => Host::Markdown,
        Some(ext) => cfg
            .scan
            .iter()
            .filter(|(_, eligible)| eligible.iter().any(|e| e == "comments"))
            .find_map(|(lang, _)| {
                COMMENT_SYNTAX
                    .iter()
                    .find(|(name, exts, _)| name == lang && exts.contains(&ext))
            })
            .map_or(Host::Plain, |(_, _, syntax)| Host::Comments(syntax)),
        None => Host::Plain,
    }
}

/// A language's comment delimiters: the region a `[scan.<lang>]` table with
/// `eligible = ["comments"]` reads, per [[rr:AD-2]].
#[derive(Debug, PartialEq, Eq)]
pub struct CommentSyntax {
    pub line: &'static str,
    /// Only the closer matching whichever opener started the block ends it,
    /// so a `'''` docstring survives a stray `"""` inside it.
    pub block: &'static [(&'static str, &'static str)],
    /// False for a Python docstring, which is a string rather than a
    /// comment, so a # inside it must not read as one.
    pub block_is_comment: bool,
    /// True where `'` may be a lifetime or loop label rather than a quote.
    /// Quoting on every `'` leaves `&'a str` open and swallows the //.
    pub tick_is_char_literal: bool,
}

const COMMENT_SYNTAX: &[(&str, &[&str], CommentSyntax)] = &[
    (
        "rust",
        &["rs"],
        CommentSyntax {
            line: "//",
            block: &[("/*", "*/")],
            block_is_comment: true,
            tick_is_char_literal: true,
        },
    ),
    (
        "python",
        &["py"],
        CommentSyntax {
            line: "#",
            block: &[("\"\"\"", "\"\"\""), ("'''", "'''")],
            block_is_comment: false,
            tick_is_char_literal: false,
        },
    ),
];

/// The comment syntax for a `[scan.<lang>]` table name, or `None` if the
/// language has no declared comment syntax.
pub fn comment_syntax(lang: &str) -> Option<&'static CommentSyntax> {
    COMMENT_SYNTAX
        .iter()
        .find(|(name, ..)| *name == lang)
        .map(|(_, _, syntax)| syntax)
}

/// The byte length of the Rust char literal at the start of s, or None
/// if it is not one, which is how `'a` is told from `'x'`.
fn char_literal_len(s: &str) -> Option<usize> {
    let mut chars = s.chars();
    if chars.next() != Some('\'') {
        return None;
    }
    let mut len = 1;
    let c = chars.next()?;
    len += c.len_utf8();
    if c == '\\' {
        let esc = chars.next()?;
        len += esc.len_utf8();
        match esc {
            'n' | 't' | 'r' | '\\' | '0' | '\'' | '"' => {}
            'x' => {
                for _ in 0..2 {
                    let h = chars.next()?;
                    if !h.is_ascii_hexdigit() {
                        return None;
                    }
                    len += h.len_utf8();
                }
            }
            'u' => {
                if chars.next()? != '{' {
                    return None;
                }
                len += 1;
                loop {
                    let h = chars.next()?;
                    len += h.len_utf8();
                    if h == '}' {
                        break;
                    }
                    if !h.is_ascii_hexdigit() {
                        return None;
                    }
                }
            }
            _ => return None,
        }
    }
    if chars.next()? == '\'' {
        Some(len + 1)
    } else {
        None
    }
}

enum CommentStart {
    Line { len: usize },
    Block { len: usize, close: &'static str },
}

/// Covers Rust's `r"`, `r#"`, `r##"`, `br"`, ... and Python's `r"`, `br"`:
/// a raw string has no escapes, so `\` inside one is a literal byte, not
/// an escape.
fn ends_in_raw_prefix(prefix: &[u8]) -> bool {
    let mut end = prefix.len();
    while end > 0 && prefix[end - 1] == b'#' {
        end -= 1;
    }
    matches!(prefix[..end], [.., b'r' | b'R'])
}

/// The leftmost syntax.line or syntax.block opener outside a quoted
/// string, with its byte offset.
fn next_comment_start(line: &str, syntax: &CommentSyntax) -> Option<(usize, CommentStart)> {
    let bytes = line.as_bytes();
    let line_marker = syntax.line.as_bytes();
    let mut quote: Option<(u8, bool)> = None;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if let Some((q, has_escapes)) = quote {
            if has_escapes && b == b'\\' {
                i += 2;
                continue;
            }
            if b == q {
                quote = None;
            }
        } else if let Some(&(open, close)) = syntax
            .block
            .iter()
            .find(|(open, _)| bytes[i..].starts_with(open.as_bytes()))
        {
            // Must come before the quote arm. Read one byte at a time,
            // """ looks like an empty string plus a dangling quote.
            return Some((
                i,
                CommentStart::Block {
                    len: open.len(),
                    close,
                },
            ));
        } else if bytes[i..].starts_with(line_marker) {
            return Some((
                i,
                CommentStart::Line {
                    len: line_marker.len(),
                },
            ));
        } else if b == b'\'' && syntax.tick_is_char_literal {
            i += char_literal_len(&line[i..]).unwrap_or(1);
            continue;
        } else if b == b'"' || b == b'\'' {
            let has_escapes = !(b == b'"' && ends_in_raw_prefix(&bytes[..i]));
            quote = Some((b, has_escapes));
        }
        i += 1;
    }
    None
}

/// Test-only: the text after a line-comment opener, via the same scan
/// `next_comment_start` gives the shipped path. `None` for a block opener
/// or no opener on the line.
#[cfg(test)]
fn line_comment_text<'a>(line: &'a str, syntax: &CommentSyntax) -> Option<&'a str> {
    match next_comment_start(line, syntax)? {
        (off, CommentStart::Line { len }) => Some(line[off + len..].trim()),
        (_, CommentStart::Block { .. }) => None,
    }
}

/// Scan one file's content. Markers and malformed openers come from every
/// scanned region; mentions come from prose only.
pub fn scan(content: &str, host: Host) -> Vec<Found> {
    let mut out = Vec::new();
    let mut fence: Option<&str> = None; // the delimiter that opened the fence
    let mut awaiting_close: Option<&'static str> = None;
    for (i, line) in content.lines().enumerate() {
        let lineno = (i + 1) as u64;
        match host {
            Host::Markdown => {
                let trimmed = line.trim_start();
                let delim = if trimmed.starts_with("```") {
                    Some("```")
                } else if trimmed.starts_with("~~~") {
                    Some("~~~")
                } else {
                    None
                };
                match (fence, delim) {
                    (None, Some(d)) => {
                        fence = Some(d);
                        continue;
                    }
                    (Some(open), Some(d)) if open == d => {
                        fence = None;
                        continue;
                    }
                    (Some(_), _) => continue, // inside a fence: invisible
                    (None, None) => {}
                }
                for (text, is_span) in split_inline(line) {
                    scan_segment(text, is_span, lineno, &mut out);
                }
            }
            // Comment text is read exactly as a Plain line is, so mentions
            // qualify there too, per [[rr:AD-5]].
            Host::Comments(syntax) => {
                let mut pos = 0;
                loop {
                    if let Some(close) = awaiting_close {
                        match line[pos..].find(close) {
                            Some(idx) => {
                                if syntax.block_is_comment {
                                    scan_segment(&line[pos..pos + idx], false, lineno, &mut out);
                                }
                                pos += idx + close.len();
                                awaiting_close = None;
                            }
                            None => {
                                if syntax.block_is_comment {
                                    scan_segment(&line[pos..], false, lineno, &mut out);
                                }
                                break;
                            }
                        }
                    } else {
                        match next_comment_start(&line[pos..], syntax) {
                            Some((off, CommentStart::Block { len, close })) => {
                                pos += off + len;
                                awaiting_close = Some(close);
                            }
                            Some((off, CommentStart::Line { len })) => {
                                scan_segment(&line[pos + off + len..], false, lineno, &mut out);
                                break;
                            }
                            None => break,
                        }
                    }
                }
            }
            Host::Plain => scan_segment(line, false, lineno, &mut out),
        }
    }
    out
}

/// Scan one region segment. A code span is read only when its content begins
/// with the marker opener, and never for mentions.
fn scan_segment(text: &str, is_span: bool, lineno: u64, out: &mut Vec<Found>) {
    if is_span {
        if text.starts_with(marker::OPENER) {
            match marker::scan_token(text) {
                marker::Token::Marker { len, anchor } => out.push(Found {
                    line: lineno,
                    what: What::Marker {
                        raw: text[..len].to_string(),
                        anchor,
                    },
                }),
                marker::Token::Malformed(reason) => out.push(Found {
                    line: lineno,
                    what: What::Malformed { reason },
                }),
            }
        }
        return;
    }

    // Prose: find every marker occurrence, remembering the spans they cover
    // so the mention pass skips marker interiors.
    let mut covered: Vec<(usize, usize)> = Vec::new();
    let mut from = 0;
    while let Some(rel) = text[from..].find(marker::OPENER) {
        let start = from + rel;
        match marker::scan_token(&text[start..]) {
            marker::Token::Marker { len, anchor } => {
                out.push(Found {
                    line: lineno,
                    what: What::Marker {
                        raw: text[start..start + len].to_string(),
                        anchor,
                    },
                });
                covered.push((start, start + len));
                from = start + len;
            }
            marker::Token::Malformed(reason) => {
                out.push(Found {
                    line: lineno,
                    what: What::Malformed { reason },
                });
                covered.push((start, text.len()));
                from = start + marker::OPENER.len();
            }
        }
    }

    mentions_in(text, &covered, lineno, out);
}

/// Tokenize `text` for path mentions, skipping any byte range in `covered`.
fn mentions_in(text: &str, covered: &[(usize, usize)], lineno: u64, out: &mut Vec<Found>) {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if !is_token_byte(bytes[i]) {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && is_token_byte(bytes[i]) {
            i += 1;
        }
        if covered.iter().any(|&(s, e)| start < e && i > s) {
            continue; // inside (or overlapping) a marker: excluded
        }
        let mut token = &text[start..i];
        // A sentence-final dot is punctuation, not path text.
        while let Some(t) = token.strip_suffix('.') {
            token = t;
        }
        if !is_path_shaped(token) {
            continue;
        }
        let line_ref =
            bytes.get(i) == Some(&b':') && bytes.get(i + 1).is_some_and(|b| b.is_ascii_digit());
        out.push(Found {
            line: lineno,
            what: What::Mention {
                token: token.to_string(),
                line_ref,
            },
        });
    }
}

/// The mention token charset: path text plus `/` separators. Anything else
/// delimits a token.
fn is_token_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'/')
}

/// Whether a token is lexically a path: two or more nonempty `/`-separated
/// segments (`[[rr:AD-5]]`). Root-relative only: a leading, trailing, or
/// doubled separator disqualifies, and so does a `.` or `..` segment, since a
/// mention never traverses out of the tree it is judged against.
pub fn is_path_shaped(token: &str) -> bool {
    token.contains('/')
        && token.split('/').count() >= 2
        && token
            .split('/')
            .all(|s| !s.is_empty() && s != "." && s != "..")
}

/// Split one Markdown line into prose and inline-code-span segments. Spans
/// follow the backtick-run rule: an opener of N backticks closes at the next
/// run of exactly N; an unclosed opener is literal prose.
fn split_inline(line: &str) -> Vec<(&str, bool)> {
    let mut parts = Vec::new();
    let bytes = line.as_bytes();
    let mut pos = 0;
    let mut prose_from = 0;
    while pos < bytes.len() {
        if bytes[pos] != b'`' {
            pos += 1;
            continue;
        }
        let open_start = pos;
        while pos < bytes.len() && bytes[pos] == b'`' {
            pos += 1;
        }
        let ticks = pos - open_start;
        // Find the next run of exactly `ticks` backticks.
        let mut probe = pos;
        let mut close: Option<(usize, usize)> = None;
        while probe < bytes.len() {
            if bytes[probe] != b'`' {
                probe += 1;
                continue;
            }
            let run_start = probe;
            while probe < bytes.len() && bytes[probe] == b'`' {
                probe += 1;
            }
            if probe - run_start == ticks {
                close = Some((run_start, probe));
                break;
            }
        }
        // An unclosed opener leaves `close` empty: the backticks are literal
        // prose, and the scan just keeps going.
        if let Some((close_start, close_end)) = close {
            if prose_from < open_start {
                parts.push((&line[prose_from..open_start], false));
            }
            parts.push((&line[pos..close_start], true));
            pos = close_end;
            prose_from = close_end;
        }
    }
    if prose_from < line.len() {
        parts.push((&line[prose_from..], false));
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(content: &str, host: Host) -> Vec<String> {
        scan(content, host)
            .into_iter()
            .map(|f| match f.what {
                What::Marker { anchor, .. } => format!("{}:marker:{anchor}", f.line),
                What::Malformed { .. } => format!("{}:malformed", f.line),
                What::Mention { token, line_ref } => {
                    format!(
                        "{}:mention:{token}{}",
                        f.line,
                        if line_ref { ":line" } else { "" }
                    )
                }
            })
            .collect()
    }

    #[test]
    fn host_for_resolves_via_scan_config() {
        let cfg = Config {
            verify_in_scope: Vec::new(),
            verify_exclude: Vec::new(),
            scan: vec![("python".to_string(), vec!["comments".to_string()])],
        };
        assert_eq!(host_for(Some("md"), &cfg), Host::Markdown);
        assert!(matches!(host_for(Some("py"), &cfg), Host::Comments(_)));
        assert_eq!(host_for(Some("go"), &cfg), Host::Plain); // not declared eligible
        let bare = Config {
            verify_in_scope: Vec::new(),
            verify_exclude: Vec::new(),
            scan: Vec::new(),
        };
        assert_eq!(host_for(Some("py"), &bare), Host::Plain); // no [scan.python] at all
    }

    #[test]
    fn comment_syntax_resolves_known_languages_only() {
        assert_eq!(comment_syntax("rust").unwrap().line, "//");
        assert!(!comment_syntax("python").unwrap().block_is_comment);
        assert!(comment_syntax("go").is_none());
    }

    #[test]
    fn comment_text_is_quote_aware() {
        let py = comment_syntax("python").unwrap();
        let rust = comment_syntax("rust").unwrap();
        assert_eq!(line_comment_text("# x", py), Some("x"));
        assert_eq!(line_comment_text(r#"s = "a # b" # c"#, py), Some("c"));
        assert_eq!(
            line_comment_text(r#"let u = "http://x"; // y"#, rust),
            Some("y")
        );
        assert_eq!(line_comment_text("let x = 1;", rust), None);
    }

    #[test]
    fn lifetimes_and_loop_labels_do_not_open_a_quote() {
        let rust = comment_syntax("rust").unwrap();
        assert_eq!(
            line_comment_text("fn f() -> &'static str { \"x\" } // gone-rust", rust),
            Some("gone-rust")
        );
        assert_eq!(
            line_comment_text("fn f<'a>(x: &'a str, y: &'a str) {} // gone2", rust),
            Some("gone2")
        );
        assert_eq!(
            line_comment_text("'outer: loop { // gone3", rust),
            Some("gone3")
        );
        // Char literals still validate and self-terminate as before.
        assert_eq!(line_comment_text("let c = 'x'; // ok", rust), Some("ok"));
        assert_eq!(
            line_comment_text(r"let n = '\n'; // ok2", rust),
            Some("ok2")
        );
        assert_eq!(
            line_comment_text(r"let h = '\u{2764}'; // ok3", rust),
            Some("ok3")
        );
        // Fails unless the literal is consumed whole: the " would open a
        // string and eat the comment.
        assert_eq!(line_comment_text("let q = '\"'; // ok4", rust), Some("ok4"));
    }

    #[test]
    fn comments_host_reads_only_the_comment() {
        let py = comment_syntax("python").unwrap();
        let text = concat!(
            "s = \"see [[rr:AD-1]] docs/design/x.md\"  ",
            "# see [[rr:AD-2]] docs/design/x.md\n"
        );
        let got = kinds(text, Host::Comments(py));
        assert_eq!(
            got,
            vec![
                "1:marker:AD-2".to_string(),
                "1:mention:docs/design/x.md".to_string(),
            ],
            "{got:?}"
        );
    }

    #[test]
    fn block_comment_spans_lines_and_can_share_a_line_with_a_line_comment() {
        let rust = comment_syntax("rust").unwrap();
        let text = "/*\nsee [[rr:x]] here\n*/\n";
        let got = kinds(text, Host::Comments(rust));
        assert_eq!(got, vec!["2:marker:x"], "{got:?}");

        let text = "/* a */ code // see [[rr:y]]\n";
        let got = kinds(text, Host::Comments(rust));
        assert_eq!(got, vec!["1:marker:y"], "{got:?}");
    }

    #[test]
    fn docstring_hash_is_not_a_comment() {
        let py = comment_syntax("python").unwrap();
        let text = "\"\"\"\n# [[rr:x]] not a comment\n\"\"\"\n# [[rr:y]] a real comment\n";
        let got = kinds(text, Host::Comments(py));
        assert_eq!(got, vec!["4:marker:y"], "{got:?}");
    }

    #[test]
    fn single_quote_docstring_hash_is_not_a_comment() {
        let py = comment_syntax("python").unwrap();
        let text = "'''\n# [[rr:k]] hidden\n'''\n# [[rr:m]] real\n";
        let got = kinds(text, Host::Comments(py));
        assert_eq!(got, vec!["4:marker:m"], "{got:?}");
    }

    #[test]
    fn mismatched_triple_quote_does_not_close_the_block() {
        let py = comment_syntax("python").unwrap();
        let text = "'''\n\"\"\" # [[rr:still-hidden]]\n'''\n# [[rr:m]] real\n";
        let got = kinds(text, Host::Comments(py));
        assert_eq!(got, vec!["4:marker:m"], "{got:?}");
    }

    // Known limits of the line-local quote/block model, pinned rather than
    // fixed: real nesting and cross-line string state would need a lexer
    // per language, which is disproportionate to how rarely these shapes
    // occur. If one of these ever starts failing, that's a design change,
    // not a regression.

    #[test]
    fn nested_block_comment_closes_at_its_first_close() {
        let rust = comment_syntax("rust").unwrap();
        let got = kinds("/* /* */ [[rr:o]] */\n", Host::Comments(rust));
        assert_eq!(got, Vec::<String>::new(), "{got:?}");
    }

    #[test]
    fn embedded_newline_in_a_string_reads_as_a_fresh_comment_line() {
        let rust = comment_syntax("rust").unwrap();
        let got = kinds("let s = \"a\n// [[rr:p]]\n\";\n", Host::Comments(rust));
        assert_eq!(got, vec!["2:marker:p"], "{got:?}");
    }

    #[test]
    fn raw_string_inner_quote_closes_the_tracked_quote_early() {
        let rust = comment_syntax("rust").unwrap();
        let got = kinds("let r = r#\"a \" b\"#; // [[rr:f]]\n", Host::Comments(rust));
        assert_eq!(got, Vec::<String>::new(), "{got:?}");
    }

    #[test]
    fn raw_string_trailing_backslash_is_not_an_escape() {
        let rust = comment_syntax("rust").unwrap();
        let got = kinds("let s = r\"C:\\\"; // [[rr:x]]\n", Host::Comments(rust));
        assert_eq!(got, vec!["1:marker:x"], "{got:?}");
    }

    #[test]
    fn finds_markers_in_prose_and_qualifying_spans() {
        let text = "see [[rr:AD-1]] and `[[rr:AD-2]]` and `rg '\\[\\[rr:'` here\n";
        let got = kinds(text, Host::Markdown);
        assert_eq!(got, vec!["1:marker:AD-1", "1:marker:AD-2"], "{got:?}");
    }

    #[test]
    fn fenced_blocks_are_invisible() {
        let text = "[[rr:a]]\n```\n[[rr:fenced]]\nsrc/fenced.rs\n```\n[[rr:b]]\n";
        let got = kinds(text, Host::Markdown);
        assert_eq!(got, vec!["1:marker:a", "6:marker:b"], "{got:?}");
    }

    #[test]
    fn malformed_opener_is_reported() {
        let got = kinds("an unpaired [[rr:oops opener\n", Host::Markdown);
        assert_eq!(got, vec!["1:malformed"], "{got:?}");
    }

    #[test]
    fn mentions_come_from_prose_only() {
        let text = "the parser in src/cli.rs, not `src/other.rs`, and and/or aside\n";
        let got = kinds(text, Host::Markdown);
        assert_eq!(
            got,
            vec!["1:mention:src/cli.rs", "1:mention:and/or"],
            "{got:?}"
        );
    }

    #[test]
    fn marker_interiors_are_not_mentions() {
        let text = "[[rr:src/cli.rs#parse_reference]] narrows it\n";
        let got = kinds(text, Host::Markdown);
        assert_eq!(got, vec!["1:marker:src/cli.rs#parse_reference"], "{got:?}");
    }

    #[test]
    fn path_line_lookahead_sets_line_ref() {
        let got = kinds("broken at src/cli.rs:42 yesterday\n", Host::Plain);
        assert_eq!(got, vec!["1:mention:src/cli.rs:line"], "{got:?}");
    }

    #[test]
    fn sentence_final_dot_is_stripped() {
        let got = kinds("it lives in doc/ad.\n", Host::Plain);
        assert_eq!(got, vec!["1:mention:doc/ad"], "{got:?}");
    }

    #[test]
    fn path_shape_rejects_urls_and_fragments() {
        assert!(is_path_shaped("src/cli.rs"));
        assert!(is_path_shaped("doc/ad"));
        assert!(is_path_shaped("and/or"));
        assert!(!is_path_shaped("README.md"));
        assert!(!is_path_shaped("/abs/path"));
        assert!(!is_path_shaped("dir/"));
        assert!(!is_path_shaped("a//b"));
        assert!(!is_path_shaped("//host/share"));
        assert!(!is_path_shaped("../../README.md"));
        assert!(!is_path_shaped("./relative/form"));
        assert!(!is_path_shaped("src/../escape"));
    }

    #[test]
    fn plain_host_scans_every_line() {
        let text = "```\n[[rr:not-md-fence]]\n```\n";
        let got = kinds(text, Host::Plain);
        assert_eq!(got, vec!["2:marker:not-md-fence"], "{got:?}");
    }
}
