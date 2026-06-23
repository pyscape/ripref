//! Rust: item definitions — functions and methods, structs, enums, unions,
//! traits, type aliases, consts, statics, modules, and macros — as anchors, via
//! the `tree-sitter-rust` grammar.

use crate::languages::Language;

pub const LANGUAGE: Language = Language {
    name: "rust",
    extensions: &["rs"],
    grammar: tree_sitter_rust::LANGUAGE,
    anchors_query: ANCHORS,
};

// `function_item` also matches methods — they are `function_item` nodes nested
// in an `impl_item` — so methods become anchors without a separate pattern.
const ANCHORS: &str = r"
(function_item name: (identifier) @anchor)
(struct_item name: (type_identifier) @anchor)
(enum_item name: (type_identifier) @anchor)
(union_item name: (type_identifier) @anchor)
(trait_item name: (type_identifier) @anchor)
(type_item name: (type_identifier) @anchor)
(const_item name: (identifier) @anchor)
(static_item name: (identifier) @anchor)
(mod_item name: (identifier) @anchor)
(macro_definition name: (identifier) @anchor)
";
