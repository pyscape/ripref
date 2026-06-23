# Languages

`rr` cites code and prose by stable, human-readable *anchors* (heading text,
symbol names, section identifiers) instead of line numbers that shift with every
edit. To do that across every language a team uses, `rr` needs to know what
counts as an anchor in each file kind. Each **language** pairs a [Tree-sitter]
grammar with a small S-expression query that names the anchors in its parse
tree.

First-class languages are ordinary Rust crate dependencies (`tree-sitter-rust`,
`tree-sitter-md`, ...) compiled into the binary and run natively; loading a
grammar is a function-pointer wrap (nanoseconds). Adding one is a module plus a
one-line registry entry; nothing else changes: the indexer, the index format,
and the reader are unaware of language specifics.

Third-party languages that ship as a prebuilt `.wasm` are a separate, runtime
path (no `rr` rebuild needed to add one). It costs more to load (see the
benchmark section in the root [`README.md`](../../README.md) and
[`benches/grammar_loader.rs`](../../benches/grammar_loader.rs)).

## The contract

Every language is one [`Language`](mod.rs) value:

| field | meaning |
|---|---|
| `name` | stable identifier, e.g. `"rust"` |
| `extensions` | file extensions it claims, without the dot, e.g. `&["rs"]` |
| `grammar` | the grammar `LanguageFn`, from its crate (e.g. `tree_sitter_rust::LANGUAGE`) |
| `anchors_query` | an S-expression query; each `@anchor` capture becomes one anchor |

The shared engine ([`Language::extract`](mod.rs)) parses a file with the
grammar, runs the query, and emits one anchor per `@anchor` capture, identical
for every language.

## Adding a language

**1.** Add the grammar crate:

```sh
cargo add tree-sitter-<lang>
```

**2.** Create `src/languages/<lang>.rs` with a `Language` const:

```rust
use crate::languages::Language;

pub const LANGUAGE: Language = Language {
    name: "<lang>",
    extensions: &["<ext>"],
    grammar: tree_sitter_<lang>::LANGUAGE,
    anchors_query: "(function_definition name: (identifier) @anchor)",
};
```

**3.** Declare and register it in [`mod.rs`](mod.rs):

```rust
pub mod <lang>;                       // <- declare

pub static LANGUAGES: &[Language] = &[
    markdown::LANGUAGE,
    rust::LANGUAGE,
    <lang>::LANGUAGE,                 // <- register
];
```

That's it, no other file changes. Add a test in `mod.rs`'s `tests` module
asserting the anchors you expect (see the `rust_*` / `markdown_*` tests).

## Writing the anchors query

Mark the node whose *text* is the anchor with `@anchor`. Only `@anchor` captures
are emitted, so helper captures in a pattern are ignored. Each match yields one
anchor at the captured node's start line. Many grammar crates ship a
`TAGS_QUERY` (the queries GitHub uses for code navigation) that already
identifies a language's definitions, a good starting point.

## Current languages

| module | extensions | anchors |
|---|---|---|
| [`markdown`](markdown.rs) | `.md`, `.markdown` | ATX headings |
| [`rust`](rust.rs) | `.rs` | functions & methods, structs, enums, unions, traits, type aliases, consts, statics, modules, macros |

[Tree-sitter]: https://tree-sitter.github.io/
