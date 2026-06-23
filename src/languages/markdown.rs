//! Markdown: ATX headings (`# …` through `###### …`) as anchors, via the
//! `tree-sitter-md` block grammar. Editing the query below changes what gets
//! indexed; no other code changes.

use crate::languages::Language;

pub const LANGUAGE: Language = Language {
    name: "markdown",
    extensions: &["md", "markdown"],
    grammar: tree_sitter_md::LANGUAGE,
    // In tree-sitter-md, `heading_content` is a field of `atx_heading` whose
    // node is `inline` (the heading text). Capture that node.
    anchors_query: "(atx_heading heading_content: (inline) @anchor)",
};
