//! Rust: item definitions — functions and methods, structs, enums, unions,
//! traits, type aliases, consts, statics, modules, and macros — as anchors,
//! via the `tree-sitter-rust` grammar. Each `@span` capture is the whole
//! item, so a symbol's definition spans its body (`[[rr:AD-1]]`).

use crate::languages::{Language, Mode};

pub const LANGUAGE: Language = Language {
    name: "rust",
    extensions: &["rs"],
    grammar: tree_sitter_rust::LANGUAGE,
    anchors_query: ANCHORS,
    mode: Mode::Symbols,
};

// `function_item` also matches methods — they are `function_item` nodes
// nested in an `impl_item` — so methods become anchors without a separate
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
