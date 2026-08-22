# Languages

`rr` references code and prose by stable, human-readable *anchors* (record
IDs, heading text, symbol names) instead of line numbers that shift with
every edit. To do that across every language a team uses, `rr` needs to know
what defines an anchor in each file kind. Each **language** pairs a
[Tree-sitter] grammar with a small S-expression query that names the anchors
in its parse tree.

First-class languages are ordinary Rust crate dependencies
(`tree-sitter-rust`, `tree-sitter-md`, `tree-sitter-python`, ...) compiled
into the binary and run
natively; loading a grammar is a function-pointer wrap (nanoseconds). Adding
one is a module plus a one-line registry entry; nothing else changes: the
indexer, the index format, and the reader are unaware of language specifics.

Third-party languages that ship as a prebuilt `.wasm` are a separate, runtime
path (no `rr` rebuild needed to add one). It costs more to load; see the
benchmark section in the root [`README.md`](../../README.md) and
[`benches/grammar_loader.rs`](../../benches/grammar_loader.rs).

## The contract

Every language is one [`Language`](mod.rs) value:

| field           | meaning                                                                     |
| --------------- | --------------------------------------------------------------------------- |
| `name`          | stable identifier, e.g. `"rust"`                                             |
| `extensions`    | file extensions it claims, without the dot, e.g. `&["rs"]`                   |
| `grammar`       | the grammar `LanguageFn`, from its crate (e.g. `tree_sitter_rust::LANGUAGE`) |
| `anchors_query` | an S-expression query; each `@anchor` capture names one anchor               |
| `mode`          | how captures become anchors: `Symbols` or `Sections`                        |
| `level`         | a title line's rank in `Sections` (lower outranks); `Symbols` languages pass a function returning a constant |
| `titles`        | `Some(finder)` to find titles lexically instead of via `grammar`/`anchors_query`; tree-sitter languages pass `None` |

The shared engine ([`Language::extract`](mod.rs)) parses a file with the
grammar, runs the query, and applies the span rule the mode implements:

- **`Symbols`** (code): each match is one anchor whose identity is the
  `@anchor` node's text and whose definition spans the `@span` node (the
  whole item), falling back to the `@anchor` node's own lines.
- **`Sections`** (titled regions, e.g. Markdown): each `@anchor` capture is a
  title; the anchor spans from the title line to the next title of the same
  or higher rank. The record identity rule applies: a title opening with an
  ID of uppercase letters, one hyphen, and digits, immediately before the
  title's first colon, defines the ID as the identity.

## Adding a language

**1.** Add the grammar crate:

```sh
cargo add tree-sitter-<lang>
```

**2.** Create `src/languages/<lang>.rs` with a `Language` const:

```rust
use crate::languages::{Language, Mode};

pub const LANGUAGE: Language = Language {
    name: "<lang>",
    extensions: &["<ext>"],
    grammar: tree_sitter_<lang>::LANGUAGE,
    anchors_query: "(function_definition name: (identifier) @anchor) @span",
    mode: Mode::Symbols,
    level: |_| u32::MAX,
    titles: None,
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
asserting the anchors and spans you expect (see the `rust_*` / `markdown_*`
tests).

## Adding a line-oriented format

A format whose titles are a keyword at the start of a line (Gherkin's
`Feature`/`Scenario`) needs no grammar: skip step 1 above, and give
`Language` a lexical `titles` finder instead of `anchors_query`.
`grammar`/`anchors_query` are still required fields, so point them at any
grammar and an empty query; they go unused.

```rust
use crate::languages::{Language, Mode};

pub const LANGUAGE: Language = Language {
    name: "<lang>",
    extensions: &["<ext>"],
    grammar: tree_sitter_md::LANGUAGE,
    anchors_query: "",
    mode: Mode::Sections,
    level,
    titles: Some(titles),
};

fn titles(content: &str) -> Vec<(String, u64)> {
    // one (title text, 0-based row) pair per line that opens a region
}

fn level(line: &str) -> u32 {
    // Feature: 1, Rule: 2, everything else 3
}
```

Register it the same way (step 3 above). See [`gherkin.rs`](gherkin.rs) for
a complete example.

## Writing the anchors query

Mark the node whose *text* is the identity with `@anchor`, and the node whose
*extent* is the definition with `@span` (usually the whole item, as in
`(function_item name: (identifier) @anchor) @span`). Helper captures in a
pattern are ignored. Many grammar crates ship a `TAGS_QUERY` (the queries
GitHub uses for code navigation) that already identifies a language's
definitions, a good starting point.

## Current languages

| module                    | extensions         | anchors                                                                                          |
| ------------------------- | ------------------ | ------------------------------------------------------------------------------------------------ |
| [`markdown`](markdown.rs) | `.md`, `.markdown` | ATX headings as records and headings, spanning their sections                                     |
| [`rust`](rust.rs)         | `.rs`              | functions & methods, structs, enums, unions, traits, type aliases, consts, statics, modules, macros, spanning their definitions |
| [`gherkin`](gherkin.rs)   | `.feature`         | `Feature`/`Rule`/`Scenario`/`Scenario Outline`/`Example` keywords as titled regions                |
| [`python`](python.rs)     | `.py`              | functions, methods and classes, spanning their definitions (a decorated `def` spans from `def`, not its decorators) |

[Tree-sitter]: https://tree-sitter.github.io/
