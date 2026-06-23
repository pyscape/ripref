/*!
Languages: the common contract every first-class language satisfies — a
tree-sitter grammar plus the query that names its anchors.

Each submodule is one language: a [`Language`] value registered in [`LANGUAGES`].
The indexer looks a file's extension up in that registry and runs the matching
language; the index format and reader are unaware of language specifics. Adding a
language is one new module and one [`LANGUAGES`] entry — nothing else changes.

Grammars are ordinary Rust crate dependencies (e.g. `tree-sitter-rust`); the
grammar's C is compiled by its own crate, never vendored here. Third-party
languages that ship as a prebuilt `.wasm` load through a separate path — see the
grammar-loading benchmark in `benches/` for its cost.
*/

use std::path::Path;

use streaming_iterator::StreamingIterator;
use tree_sitter::{Parser, Query, QueryCursor};
use tree_sitter_language::LanguageFn;

use crate::refidx::ForwardEntry;

pub mod markdown;
pub mod rust;

/// The capture name an anchors query uses to mark a node as an anchor.
const ANCHOR_CAPTURE: &str = "anchor";

/// A first-class language: a tree-sitter grammar and the query whose `@anchor`
/// captures become anchors. This is the contract every language shares — the
/// engine in [`Language::extract`] is generic over it.
pub struct Language {
    /// Stable identifier, e.g. `"rust"`, `"markdown"`.
    pub name: &'static str,
    /// File extensions this language claims, without the leading dot.
    pub extensions: &'static [&'static str],
    /// The grammar, from the language's crate (e.g. `tree_sitter_rust::LANGUAGE`).
    pub grammar: LanguageFn,
    /// S-expression query text; each `@anchor` capture yields one anchor.
    pub anchors_query: &'static str,
}

/// Every first-class language, consulted by file extension during indexing.
pub static LANGUAGES: &[Language] = &[markdown::LANGUAGE, rust::LANGUAGE];

/// The language that claims `ext` (first match wins), or `None`. `ext` is the
/// file extension without a dot; `None` for an extensionless file.
pub fn for_extension(ext: Option<&str>) -> Option<&'static Language> {
    let ext = ext?;
    LANGUAGES.iter().find(|l| l.extensions.contains(&ext))
}

impl Language {
    /// Read `disk_path` and emit one [`ForwardEntry`] per `@anchor` capture.
    /// Any read or parse failure yields an empty result rather than a panic.
    pub fn extract(&self, rel_path: &str, disk_path: &Path) -> Vec<ForwardEntry> {
        match std::fs::read_to_string(disk_path) {
            Ok(content) => self.extract_from_str(rel_path, &content),
            Err(_) => Vec::new(),
        }
    }

    /// The parse-and-query core of [`extract`](Self::extract), over in-memory
    /// `content` — separated so it is unit-testable without touching disk.
    pub fn extract_from_str(&self, rel_path: &str, content: &str) -> Vec<ForwardEntry> {
        let language = tree_sitter::Language::new(self.grammar);
        let Ok(query) = Query::new(&language, self.anchors_query) else {
            return Vec::new();
        };
        let Some(anchor_idx) = query.capture_index_for_name(ANCHOR_CAPTURE) else {
            return Vec::new();
        };

        let mut parser = Parser::new();
        if parser.set_language(&language).is_err() {
            return Vec::new();
        }
        let Some(tree) = parser.parse(content, None) else {
            return Vec::new();
        };

        let mut cursor = QueryCursor::new();
        let mut matches = cursor.matches(&query, tree.root_node(), content.as_bytes());
        let mut entries = Vec::new();
        while let Some(m) = matches.next() {
            for cap in m.captures {
                if cap.index != anchor_idx {
                    continue; // ignore helper captures; only @anchor nodes count
                }
                let Ok(text) = cap.node.utf8_text(content.as_bytes()) else {
                    continue;
                };
                let text = text.trim().to_string();
                if text.is_empty() {
                    continue;
                }
                let lineno = cap.node.start_position().row as u64 + 1;
                entries.push(ForwardEntry {
                    anchor: text,
                    location: format!("{rel_path}:{lineno}-{lineno}"),
                });
            }
        }
        entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anchors(language: &Language, rel_path: &str, src: &str) -> Vec<String> {
        language
            .extract_from_str(rel_path, src)
            .into_iter()
            .map(|e| e.anchor)
            .collect()
    }

    #[test]
    fn markdown_extracts_atx_headings() {
        let got = anchors(
            &markdown::LANGUAGE,
            "doc.md",
            "# Title\n\nbody\n\n## Section One\n",
        );
        assert!(got.contains(&"Title".to_string()), "got {got:?}");
        assert!(got.contains(&"Section One".to_string()), "got {got:?}");
    }

    #[test]
    fn rust_extracts_item_definitions() {
        let src = "pub fn alpha() {}\nstruct Beta;\nenum Gamma { A }\n\
                   trait Delta {}\nmod epsilon {}\ntype Zeta = u8;\n";
        let got = anchors(&rust::LANGUAGE, "lib.rs", src);
        for want in ["alpha", "Beta", "Gamma", "Delta", "epsilon", "Zeta"] {
            assert!(
                got.contains(&want.to_string()),
                "missing {want}; got {got:?}"
            );
        }
    }

    #[test]
    fn rust_captures_methods_inside_impls() {
        let got = anchors(
            &rust::LANGUAGE,
            "s.rs",
            "struct S;\nimpl S {\n  fn method(&self) {}\n}\n",
        );
        assert!(got.contains(&"method".to_string()), "got {got:?}");
    }

    #[test]
    fn for_extension_maps_known_and_unknown() {
        assert_eq!(for_extension(Some("rs")).map(|l| l.name), Some("rust"));
        assert_eq!(for_extension(Some("md")).map(|l| l.name), Some("markdown"));
        assert!(for_extension(Some("xyz")).is_none());
        assert!(for_extension(None).is_none());
    }
}
