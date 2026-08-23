//! Rust: the ten named item forms (`fn`, `struct`, `enum`, `union`,
//! `trait`, `type`, `const`, `static`, `mod`, `macro_rules!`) as the
//! symbol anchors of `[[rr:AD-1#Decision outcome]]`, via the
//! `tree-sitter-rust` grammar. Editing the query below changes which
//! items anchor, nothing else.

use std::sync::OnceLock;

use crate::languages::{Language, Mode, Source};

pub(crate) static LANGUAGE: Language = Language {
    extensions: &["rs"],
    source: Source::Grammar {
        grammar: tree_sitter_rust::LANGUAGE,
        anchors_query: ANCHORS,
    },
    mode: Mode::Symbols,
    level: |_| u32::MAX,
    records: false,
    compiled: OnceLock::new(),
};

// `function_item` also matches methods: they are `function_item` nodes
// nested in an `impl_item`, so methods become anchors without a separate
// pattern.
const ANCHORS: &str = r"
(function_item name: (identifier) @anchor) @span
(struct_item name: (type_identifier) @anchor) @span
(enum_item name: (type_identifier) @anchor) @span
(union_item name: (type_identifier) @anchor) @span
(trait_item name: (type_identifier) @anchor) @span
(type_item name: (type_identifier) @anchor) @span
(const_item name: (identifier) @anchor) @span
(static_item name: (identifier) @anchor) @span
(mod_item name: (identifier) @anchor) @span
(macro_definition name: (identifier) @anchor) @span
";
