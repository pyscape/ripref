/*!
Languages: the common contract every first-class language satisfies — a
tree-sitter grammar plus the query that names its anchors.

Each submodule is one language: a [`Language`] value registered in
[`LANGUAGES`]. The indexer looks a file's extension up in that registry and
runs the matching language; the index format and reader are unaware of
language specifics. Adding a language is one new module and one [`LANGUAGES`]
entry — nothing else changes.

Two extraction modes implement the span rule of `[[rr:AD-1]]`: symbol
languages span each definition's whole extent (the `@span` capture, falling
back to the `@anchor` node), and titled-region languages (Markdown) span from
each title line to the next title of the same or higher rank. The Markdown
mode also applies the record kind's identity rule: a title opening with an ID
of uppercase letters, one hyphen, and digits, immediately followed by the
title's first colon, defines the ID as the identity.

Grammars are ordinary Rust crate dependencies (e.g. `tree-sitter-rust`); the
grammar's C is compiled by its own crate, never vendored here. Third-party
languages that ship as a prebuilt `.wasm` load through a separate path — see
the grammar-loading benchmark in benches/.
*/

use std::path::Path;

use streaming_iterator::StreamingIterator;
use tree_sitter::{Parser, Query, QueryCursor};
use tree_sitter_language::LanguageFn;

use crate::refidx::ForwardEntry;

pub mod markdown;
pub mod rust;

/// The capture name an anchors query uses to mark a node's identity text.
const ANCHOR_CAPTURE: &str = "anchor";
/// The optional capture naming the node whose extent is the definition span.
const SPAN_CAPTURE: &str = "span";

/// How a language's captures become anchors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Code definitions: each match is one anchor whose span is the `@span`
    /// node (the whole item), or the `@anchor` node when no `@span` exists.
    Symbols,
    /// Titled regions: each `@anchor` capture is a title; spans run from the
    /// title line to the next title of the same or higher rank, and the
    /// record identity rule applies.
    Sections,
}

/// A first-class language: a tree-sitter grammar and the query whose
/// captures become anchors.
pub struct Language {
    /// Stable identifier, e.g. `"rust"`, `"markdown"`.
    pub name: &'static str,
    /// File extensions this language claims, without the leading dot.
    pub extensions: &'static [&'static str],
    /// The grammar, from the language's crate.
    pub grammar: LanguageFn,
    /// S-expression query text.
    pub anchors_query: &'static str,
    /// How captures become anchors.
    pub mode: Mode,
}

/// Every first-class language, consulted by file extension during indexing.
pub static LANGUAGES: &[Language] = &[markdown::LANGUAGE, rust::LANGUAGE];

/// The language that claims `ext` (first match wins), or `None`. `ext` is
/// the file extension without a dot; `None` for an extensionless file.
pub fn for_extension(ext: Option<&str>) -> Option<&'static Language> {
    let ext = ext?;
    LANGUAGES.iter().find(|l| l.extensions.contains(&ext))
}

/// A raw capture: identity text plus 0-based start/end rows.
struct Capture {
    text: String,
    start_row: u64,
    end_row: u64,
}

impl Language {
    /// Read `disk_path` and emit one [`ForwardEntry`] per anchor. Any read or
    /// parse failure yields an empty result rather than a panic.
    pub fn extract(&self, rel_path: &str, disk_path: &Path) -> Vec<ForwardEntry> {
        match std::fs::read_to_string(disk_path) {
            Ok(content) => self.extract_from_str(rel_path, &content),
            Err(_) => Vec::new(),
        }
    }

    /// The parse-and-query core of [`extract`](Self::extract), over
    /// in-memory `content` — separated so it is unit-testable without
    /// touching disk.
    pub fn extract_from_str(&self, rel_path: &str, content: &str) -> Vec<ForwardEntry> {
        let captures = self.run_query(content);
        match self.mode {
            Mode::Symbols => captures
                .into_iter()
                .map(|c| ForwardEntry {
                    anchor: c.text,
                    location: format!("{rel_path}:{}-{}", c.start_row + 1, c.end_row + 1),
                })
                .collect(),
            Mode::Sections => sections(rel_path, content, captures),
        }
    }

    /// Run the anchors query. In [`Mode::Symbols`], each match yields one
    /// capture whose rows come from the `@span` node when present; in
    /// [`Mode::Sections`], each `@anchor` capture yields a title at its own
    /// row.
    fn run_query(&self, content: &str) -> Vec<Capture> {
        let language = tree_sitter::Language::new(self.grammar);
        let Ok(query) = Query::new(&language, self.anchors_query) else {
            return Vec::new();
        };
        let Some(anchor_idx) = query.capture_index_for_name(ANCHOR_CAPTURE) else {
            return Vec::new();
        };
        let span_idx = query.capture_index_for_name(SPAN_CAPTURE);

        let mut parser = Parser::new();
        if parser.set_language(&language).is_err() {
            return Vec::new();
        }
        let Some(tree) = parser.parse(content, None) else {
            return Vec::new();
        };

        let mut cursor = QueryCursor::new();
        let mut matches = cursor.matches(&query, tree.root_node(), content.as_bytes());
        let mut out = Vec::new();
        while let Some(m) = matches.next() {
            let mut text = None;
            let mut anchor_rows = None;
            let mut span_rows = None;
            for cap in m.captures {
                if cap.index == anchor_idx {
                    let Ok(t) = cap.node.utf8_text(content.as_bytes()) else {
                        continue;
                    };
                    let t = t.trim();
                    if t.is_empty() {
                        continue;
                    }
                    text = Some(t.to_string());
                    anchor_rows = Some((
                        cap.node.start_position().row as u64,
                        cap.node.end_position().row as u64,
                    ));
                } else if Some(cap.index) == span_idx {
                    span_rows = Some((
                        cap.node.start_position().row as u64,
                        cap.node.end_position().row as u64,
                    ));
                }
            }
            if let (Some(text), Some(anchor_rows)) = (text, anchor_rows) {
                let (start_row, end_row) = span_rows.unwrap_or(anchor_rows);
                out.push(Capture {
                    text,
                    start_row,
                    end_row,
                });
            }
        }
        out
    }
}

/// Turn title captures into section-spanned entries: each title's region runs
/// to the line before the next title of the same or higher rank (a smaller
/// level number outranks), or the end of the file. Applies the record
/// identity rule of `[[rr:AD-1]]` per title.
fn sections(rel_path: &str, content: &str, mut titles: Vec<Capture>) -> Vec<ForwardEntry> {
    let lines: Vec<&str> = content.lines().collect();
    let total = lines.len() as u64;
    titles.sort_by_key(|c| c.start_row);
    let levels: Vec<u32> = titles
        .iter()
        .map(|t| heading_level(lines.get(t.start_row as usize).copied().unwrap_or("")))
        .collect();
    titles
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let end_row = titles[i + 1..]
                .iter()
                .zip(&levels[i + 1..])
                .find(|(_, &lvl)| lvl <= levels[i])
                .map(|(next, _)| next.start_row - 1)
                .unwrap_or_else(|| total.saturating_sub(1).max(t.start_row));
            let identity = record_id(&t.text).unwrap_or(&t.text);
            ForwardEntry {
                anchor: identity.to_string(),
                location: format!("{rel_path}:{}-{}", t.start_row + 1, end_row + 1),
            }
        })
        .collect()
}

/// The ATX heading level of a line (1-6), or `u32::MAX` for a line that is
/// not a heading (a defensive default that never outranks a real title).
fn heading_level(line: &str) -> u32 {
    let trimmed = line.trim_start();
    let hashes = trimmed.bytes().take_while(|&b| b == b'#').count();
    if (1..=6).contains(&hashes) {
        hashes as u32
    } else {
        u32::MAX
    }
}

/// The record identity rule: a title opening with an ID of uppercase ASCII
/// letters (digits allowed after the first), one hyphen, and digits,
/// immediately followed by the title's first colon, defines the ID. Any
/// other title is a heading whose identity is its full text.
pub fn record_id(title: &str) -> Option<&str> {
    let (head, _) = title.split_once(':')?;
    let (alpha, digits) = head.split_once('-')?;
    let mut chars = alpha.chars();
    let first_upper = chars.next().is_some_and(|c| c.is_ascii_uppercase());
    let alpha_ok = first_upper && chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit());
    let digits_ok = !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit());
    (alpha_ok && digits_ok).then_some(head)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(language: &Language, rel_path: &str, src: &str) -> Vec<(String, String)> {
        language
            .extract_from_str(rel_path, src)
            .into_iter()
            .map(|e| (e.anchor, e.location))
            .collect()
    }

    #[test]
    fn markdown_headings_span_their_sections() {
        let src = "# Title\n\nbody\n\n## Section One\n\ntext\ntext\n\n## Section Two\n\nend\n";
        let got = entries(&markdown::LANGUAGE, "doc.md", src);
        assert!(
            got.contains(&("Title".to_string(), "doc.md:1-12".to_string())),
            "H1 spans the document: {got:?}"
        );
        assert!(
            got.contains(&("Section One".to_string(), "doc.md:5-9".to_string())),
            "H2 spans to the next H2: {got:?}"
        );
        assert!(
            got.contains(&("Section Two".to_string(), "doc.md:10-12".to_string())),
            "final H2 spans to EOF: {got:?}"
        );
    }

    #[test]
    fn markdown_record_titles_define_the_id() {
        let src = "# AD-7: A decision\n\nbody\n\n## Decision outcome\n\ntext\n";
        let got = entries(&markdown::LANGUAGE, "doc/x.md", src);
        assert!(
            got.contains(&("AD-7".to_string(), "doc/x.md:1-7".to_string())),
            "record ID is the identity: {got:?}"
        );
        assert!(
            got.iter().any(|(a, _)| a == "Decision outcome"),
            "ordinary headings keep full text: {got:?}"
        );
    }

    #[test]
    fn record_id_rule_edges() {
        assert_eq!(record_id("AD-7: title"), Some("AD-7"));
        assert_eq!(record_id("RFC-7231: hypertext"), Some("RFC-7231"));
        assert_eq!(record_id("COVID-19: notes"), Some("COVID-19"));
        assert_eq!(
            record_id("AD-7 addendum: x"),
            None,
            "ID must touch the colon"
        );
        assert_eq!(record_id("utf-8: encoding"), None, "lowercase is no ID");
        assert_eq!(record_id("AD-7"), None, "no colon, no record rule");
        assert_eq!(record_id("AD-7-a: x"), None, "one hyphen exactly");
        assert_eq!(record_id("A D-7: x"), None, "no spaces in the ID");
    }

    #[test]
    fn rust_symbols_span_their_definitions() {
        let src = "pub fn alpha() {\n    let x = 1;\n    x;\n}\nstruct Beta;\n";
        let got = entries(&rust::LANGUAGE, "lib.rs", src);
        assert!(
            got.contains(&("alpha".to_string(), "lib.rs:1-4".to_string())),
            "fn spans its body: {got:?}"
        );
        assert!(
            got.contains(&("Beta".to_string(), "lib.rs:5-5".to_string())),
            "unit struct spans its line: {got:?}"
        );
    }

    #[test]
    fn rust_captures_methods_inside_impls() {
        let got = entries(
            &rust::LANGUAGE,
            "s.rs",
            "struct S;\nimpl S {\n  fn method(&self) {}\n}\n",
        );
        assert!(
            got.iter().any(|(a, _)| a == "method"),
            "methods are anchors: {got:?}"
        );
    }

    #[test]
    fn for_extension_maps_known_and_unknown() {
        assert_eq!(for_extension(Some("rs")).map(|l| l.name), Some("rust"));
        assert_eq!(for_extension(Some("md")).map(|l| l.name), Some("markdown"));
        assert!(for_extension(Some("xyz")).is_none());
        assert!(for_extension(None).is_none());
    }
}
