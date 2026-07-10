/*!
Lexical scanning for markers over structured hosts.

`search` and `verify` (`[[rr:AD-3]]`) run this over scoped text. The region
rules are `[[rr:AD-2]]`'s: in a Markdown host, prose and inline code spans whose
content begins with the marker opener are read and fenced blocks are invisible;
a structureless host is read per raw line.
*/

use crate::marker;

/// One scanner hit, located by 1-based line number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub line: u64,
    pub what: What,
}

/// What the scanner finds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum What {
    /// A well-formed marker: the raw bytes as written and the decoded anchor.
    Marker { raw: String, anchor: String },
    /// A `[[rr:` opener with no well-formed marker behind it.
    Malformed { reason: String },
}

/// The host structure a file exposes to the scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Host {
    /// Markdown regions: prose and qualifying inline spans; fences invisible.
    Markdown,
    /// No declared structure: every raw line is scanned.
    Plain,
}

/// The host the default profile declares for a file extension.
pub fn host_for(ext: Option<&str>) -> Host {
    match ext {
        Some("md") | Some("markdown") => Host::Markdown,
        _ => Host::Plain,
    }
}

/// Scan one file's content for markers across every scanned region.
pub fn scan(content: &str, host: Host) -> Vec<Found> {
    let mut out = Vec::new();
    let mut fence: Option<&str> = None; // the delimiter that opened the fence
    for (i, line) in content.lines().enumerate() {
        let lineno = (i + 1) as u64;
        if host == Host::Markdown {
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
        } else {
            scan_segment(line, false, lineno, &mut out);
        }
    }
    out
}

/// Scan one region segment for markers. A code span is read only when its
/// content begins with the marker opener.
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
                from = start + len;
            }
            marker::Token::Malformed(reason) => {
                out.push(Found {
                    line: lineno,
                    what: What::Malformed { reason },
                });
                from = start + marker::OPENER.len();
            }
        }
    }
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
            })
            .collect()
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
    fn plain_host_scans_every_line() {
        let text = "```\n[[rr:not-md-fence]]\n```\n";
        let got = kinds(text, Host::Plain);
        assert_eq!(got, vec!["2:marker:not-md-fence"], "{got:?}");
    }
}
