# ripref (rr)

ripref is a tool for referencing code and documentation by stable anchors
instead of fragile line numbers. It recursively indexes the current directory
into a single compact, memory-mapped index that maps each anchor (a record
ID, a heading, a code symbol, a manifest key, a scenario, an API operation)
to the locations where it is defined. With the index built, ripref resolves
an anchor to its definition, turns a file and line back into the marker that
covers it, lists every marker a project writes, and judges the references
that have gone wrong: dangling, ambiguous, malformed, or naming files that no
longer exist. ripref is language agnostic and has first-class support on
Windows, macOS and Linux.

A reference like `parser.go:42` is wrong the moment a line is inserted above
it, and nothing tells you it broke. An anchor names _what_ you meant (the
function, the heading, the decision) and stays correct across edits, moves
and refactors. ripref makes that the cheap default and turns reference rot
into a build-time (or lint-time) error.

Dual-licensed under MIT or the [UNLICENSE](https://unlicense.org).

> [!IMPORTANT]
> **Status: pre-release.** The binary is converging on the contract that the
> decision records under doc/ad fix and this README describes.

## Documentation

ripref is documented from the code and its records:

- `rr --help` lists every command and flag; the help text is generated from
  the flag definitions, so it can't drift from the parser.
- `cargo doc --open` renders the API, including the on-disk index byte
  layout, which is specified in the `refidx` module (src/refidx.rs).
- Built-in defaults are in [`rr.toml`](rr.toml); a project overrides them
  with its own `.rr.toml`.
- The design is fixed by five decision records under doc/ad: the domain
  model (`[[rr:AD-1]]`), the marker syntax (`[[rr:AD-2]]`), the verbs
  (`[[rr:AD-3]]`), the output contract (`[[rr:AD-4]]`), and path mentions
  (`[[rr:AD-5]]`).

In-page: [Quick examples](#quick-examples),
[Why use it](#why-should-i-use-ripref), [Anchors](#anchors),
[How it works](#how-it-works), [Configuration](#configuration),
[Shared options](#shared-options), [Installation](#installation)

### Quick examples

Build the index for the current directory:

```
$ rr index
indexed 1,204 anchors and 87 path mentions across 318 files
```

Resolve an anchor (or a pasted marker; the readers accept both) to where it
is defined:

```
$ rr read my_module::handler
src/handlers.py:8-26

$ rr read '[[rr:my_module::handler]]'
src/handlers.py:8-26
```

Go the other way: `rr at` takes a `file:line` and prints the marker of the
innermost anchor covering that line, the form you paste into prose, a
comment, or a commit message:

```
$ rr at src/handlers.py:15
[[rr:my_module::handler]]
```

Add `--all` to see the whole nest the line sits in, outermost first, for when
the tightest anchor is not the one you mean:

```
$ rr at docs/guide.md:57 --all
[[rr:Guide]]
[[rr:docs/guide.md#Configuration]]
```

A marker wraps the anchor's minimal unambiguous form: unqualified while the
identity is unique, path-qualified when it is not (here a second
`Configuration` heading exists elsewhere). `--format json` carries the bare
anchor alongside the marker, for editors and agents.

List the markers a project writes, before you change or delete what they
point at. With an anchor argument, `rr search` lists only the markers of that
anchor; with none, every marker; under `--mentions`, the paths prose names:

```
$ rr search my_module::handler
docs/architecture.md:14: [[rr:my_module::handler]]
docs/api/handlers.md:7: [[rr:my_module::handler]]
tests/features/requests.feature:22: [[rr:my_module::handler]]
3 markers
```

Judge every reference in scoped text, for a pre-commit hook or CI. `rr
verify` reports six finding kinds: malformed, dangling, and ambiguous
markers, a marker wrapping a bare path, a bare `path:line` reference, and a
stale path mention:

```
$ rr verify
docs/data-model.md:51: dangling marker: [[rr:legacy.Account]]
README.md:88: bare path:line reference: parser.go:42
2 findings
```

Emit JSON instead of text for piping into other tools:

```
$ rr search my_module::handler --format json
```

### Why should I use ripref?

Your editor already jumps to a symbol's references, but only within code and
only inside that editor. It has no idea that a design doc, a `.feature` spec,
an architecture decision record, or an OpenAPI operation refers to the same
thing. That is the gap ripref fills. It indexes prose and code into one
anchor namespace, so a single

```
$ rr search my_module::handler
```

lists every reference in documentation, specs and code at once, and runs in a
pre-commit hook or in CI, not just interactively in one editor. Getting that
out of vim or vscode is neither cheap nor easy; here it is one command.

The rest follows from that:

- ripref references code by _meaning_, not by line number, so your docs,
  comments and specs keep pointing at the right thing across edits and
  refactors.
- ripref reads are cheap. Every read memory-maps a single prebuilt index
  (one page-cached copy shared across all readers) and resolves the anchor
  against the sorted forward section; see [BENCHMARKS.md](BENCHMARKS.md) for
  measured costs.
- ripref checks freshness cheaply and without ever hashing file contents: a
  single `stat` per in-scope file, or, on a clean working tree, one
  `git status` plus a `rev-parse` to confirm `HEAD` still matches the index
  stamp. When the index is stale, ripref says so (exit code 3) instead of
  returning a stale answer.
- ripref is language agnostic. The default profile resolves records,
  headings, scenarios, manifest keys and API operations out of the box;
  language symbols come from per-language Tree-sitter grammars paired with a
  small query. First-class languages are built in; third-party grammars load
  from WebAssembly without rebuilding ripref.
- ripref degrades gracefully. When the index is stale and you can't rebuild,
  fall back to [ripgrep](https://github.com/BurntSushi/ripgrep) (always
  fresh, if slower) as a correct floor.
- ripref has structured output (`--format json`) so it composes cleanly with
  editors, linters and CI.

In other words, use ripref if you want references that span documentation
and code, that survive change, and that are checked fast enough to run on
every commit, save, or even read.

### Why shouldn't I use ripref?

- You only need to _find_ text. If you're searching for a pattern rather
  than resolving a named reference, use
  [ripgrep](https://github.com/BurntSushi/ripgrep) or grep, that's what
  they're for.
- You work entirely inside code, in a single editor, and never reference it
  from documentation or specs. If your editor's "find references" already
  covers what you need, ripref may be more than you want.
- The language or artifact you care about isn't supported yet and you don't
  want to add it. (Please file an issue, or contribute a grammar + query.)

### Anchors

An _anchor_ is the stable identity an artifact already has: the name a
definition bears, never a path. The default profile declares six kinds:

| Kind      | Defines                                | Example anchor                                 |
| --------- | -------------------------------------- | ---------------------------------------------- |
| record    | a titled region opening with an ID     | `AD-42`                                        |
| heading   | any other titled region                | `docs/guide.md#Configuration`                  |
| symbol    | a code definition                      | `my_module::handler`                           |
| key       | a manifest entry                       | `pyproject.toml#[tool.poetry] name`            |
| scenario  | a gherkin title                        | `tests/features/auth.feature#User can log in`  |
| operation | an API operation                       | `createUser`                                   |

An anchor may carry a path qualifier, `path#identity`, which narrows it to
identities defined in one file; the heading, key, and scenario examples above
are shown qualified because their identities commonly repeat across files. A
bare line number such as `http.go:42` is never an anchor; it is exactly the
kind of reference `rr verify` exists to retire. Which kinds exist and which
patterns define them is configuration, not hardcoded (see
[Configuration](#configuration)); the invariants every kind obeys, an
identity is never a bare path among them, are fixed by `[[rr:AD-1]]`.

An anchor plays two roles. On the CLI it is written bare: the argument to
`rr read` or `rr search`. Written into a document (prose, a comment, a commit
message) it is wrapped as the **marker**, the delimited form one
deterministic scan can always find, exactly as the examples above show;
`[[rr:AD-2]]` fixes that grammar. A
path written in plain prose needs no wrapper at all: it is a **path
mention**, which the index records and `rr verify` keeps honest
(`[[rr:AD-5]]`).

### How it works

ripref splits cleanly into one writer and many readers.

`rr index` is the single writer. It scans the working tree (respecting
`.gitignore` by default), extracts every anchor's definitions and every path
mention in scoped text, and writes one flat, sorted, memory-mappable file (by
default `.ref-cache/index`). It is the only part of ripref that writes
anything, and the index is derived and rebuildable: ripref stores no file
content and needs no version-control system.

`read` and `at` answer from the index: a reader memory-maps it, keeping one
page-cached copy shared across every concurrent reader. `search` is purely
lexical and reads no index at all, so it also runs on text outside any
project. `verify` scans text the way `search` does, then judges what it finds
against the index and the live tree.

#### Freshness

Freshness is a `stat`, not a hash. The index records its build time; a
reader compares that against the newest modification time among the files it
would resolve against. If anything is newer, the index is stale and the
reader exits 3 rather than answer from stale data. Recover by re-running
`rr index`, fall back to ripgrep (always fresh), or pass `--no-freshness` to
accept the index as-is. The comparison is second-granular (filesystem
`mtime` resolution); a file written within the same second as the index
build is treated as fresh by design.

At repository scale it is this freshness walk, not the lookup, that
dominates a query, so the stat-walk runs in parallel (`std::thread::scope`,
no added dependency). On a real ~2,200-file, ~26k-anchor tree the parallel
walk is about 15 ms, roughly 3x faster than a serial reduction, and the gap
widens as the tree grows (see the [`freshness`](benches/freshness.rs)
micro-benchmark). Two paths skip the walk entirely: `--no-freshness` answers
from the index as-is, and a clean working tree whose `HEAD` matches the
index stamp is known-fresh without any `stat` at all.

The on-disk index format is specified in the `refidx` module
(src/refidx.rs).

#### Languages: native and WebAssembly

Extracting a language's symbols pairs a
[Tree-sitter](https://tree-sitter.github.io/) grammar with a small query
(see src/languages/mod.rs). A grammar can reach ripref two ways, and they
cost very differently to load:

- **First-class (native).** The grammar is a Rust crate dependency
  (`tree-sitter-rust`, `tree-sitter-md`, ...) compiled into the binary.
  Loading it is a function-pointer wrap (effectively free).
- **Third-party (WebAssembly).** The grammar ships as a prebuilt
  `parser.wasm` loaded at runtime, so adding a language needs no rebuild of
  ripref. The runtime compiles the module on load.

[`benches/grammar_loader.rs`](benches/grammar_loader.rs) measures both on
the same markdown grammar (`cargo bench --bench grammar_loader --features
wasm`):

| operation                              | native  | WebAssembly |
| -------------------------------------- | ------- | ----------- |
| `language_init` (load the grammar)     | ~1 ns   | ~145 ms     |
| `query_compile` (compile the query)    | ~1 ms   | ~0.8 ms     |
| `parse` + extract (small doc / README) | 33 us / 5.9 ms | 49 us / 8.6 ms |

The headline is `language_init`. Native is a pointer wrap; the WebAssembly
path pays a one-time ~145 ms to compile the grammar module (**per process
start**), because tree-sitter's `WasmStore` does not expose wasmtime's
ahead-of-time (`Module::serialize`) cache. With that cache the load drops to
well under a millisecond; [`examples/wasm_load_probe.rs`](examples/wasm_load_probe.rs)
measures the decomposition and that cached ceiling. Once loaded, query
compilation is at parity and parsing is ~1.5x slower under the sandbox.

So ripref keeps its built-in languages native and treats the WebAssembly
path as opt-in extensibility; making it cheap enough for routine use means
teaching the loader to cache compiled modules, which `WasmStore` would first
need to expose.

### Configuration

ripref ships language-agnostic defaults: it respects `.gitignore` and
indexes everything that isn't ignored, just like ripgrep. Project-specific
conventions (which paths are in scope, which anchor kinds exist and what
defines them, which finding kinds the gate reports) are **configuration**,
not built-in rules, so ripref stays general and any one project's
conventions are just a profile on top of it.

The built-in defaults (the base layer your config merges over) are in
[`rr.toml`](rr.toml) at the repository root of ripref itself. Drop a
`.rr.toml` at your project root to override them: scope, the anchor kinds
and their patterns, per-language scan regions, and the `verify` rule set.
This repository's own [`.rr.toml`](.rr.toml) excludes its test fixtures from
the gate, since fixtures hold deliberate violations. Additional languages
are a Tree-sitter grammar plus a query, built in or loaded from WebAssembly.

### Shared options

Every command accepts:

- `--index <path>` (or the `REF_INDEX` environment variable): location of
  the index. Defaults to `.ref-cache/index`.
- `--format text|json`: human-readable text (default), or one `rr-json`
  envelope for piping into other tools (`[[rr:AD-4]]`).
- `--color auto|always|never`: when to colorize output (default `auto`);
  `--no-color` is shorthand for `--color never`.
- `--no-freshness`: answer from the index as-is instead of exiting 3 when it
  is stale; for consumers (a completion popup, a preview) that prefer a
  slightly-old answer to an error.

Exit codes are consistent across commands: every verb asks a question, and
the code reports how it was answered.

- `0`: the question got its answer.
- `1`: the adverse answer: `read` or `at` found nothing or resolved
  ambiguously, `search` found no matching marker, `verify` has findings.
- `2`: usage error.
- `3`: the index is stale; rebuild with `rr index`, fall back to ripgrep, or
  pass `--no-freshness`. `search` reads no index and never returns 3.

### Installation

The binary name for ripref is `rr`.

If you're a Rust programmer, ripref can be installed with `cargo`:

```
$ cargo install ripref
```

To build and install from a checkout of this repository:

```
$ cargo install --path .
```

Precompiled binaries for Windows, macOS and Linux are attached to each
[release](https://github.com/pyscape/ripref/releases).

### Building

ripref is written in Rust, so you'll need a
[Rust installation](https://www.rust-lang.org/) in order to compile it.
ripref compiles with the latest stable release of the Rust compiler. To
build:

```
$ git clone https://github.com/pyscape/ripref
$ cd ripref
$ cargo build --release
$ ./target/release/rr --version
```

### Running tests

ripref has both unit tests (each component in isolation) and integration
tests (the compiled `rr` binary driven end to end against the spec). To run
the full suite, use:

```
$ cargo test --all
```

from the repository root.

### Debugging

VS Code is configured for step-through debugging of the integration tests in
tests/cli.rs. You need two extensions:

- [`rust-lang.rust-analyzer`](https://marketplace.visualstudio.com/items?itemName=rust-lang.rust-analyzer)
- [`ms-vscode.cpptools`](https://marketplace.visualstudio.com/items?itemName=ms-vscode.cpptools)

Open the Extensions panel; both appear under **Workspace Recommendations**
and can be installed from there. CodeLLDB is not used; it crashes on Windows
with the MSVC toolchain (exit `0xC0000005`).

Once installed, two ways to launch a debug session:

- **CodeLens** (quickest): open tests/cli.rs, click the **Debug** link that
  appears above any `#[test]` function.
- **Run and Debug panel** (`Ctrl+Shift+D`, then `F5`): pick
  **"Debug: tests/cli.rs (all tests)"** to run every test under the
  debugger, or **"Debug: tests/cli.rs (filter by name)"** to be prompted for
  a substring and run only matching tests.

Both launch configurations run `cargo test --test cli --no-run` before
attaching, so the binary is always current. Set breakpoints anywhere in
src/ or tests/ before pressing `F5`.

### Linting and formatting

Rust style is enforced with `rustfmt` (formatter) and `clippy` (linter),
both bundled with the toolchain. The lint posture is declared as crate-level
attributes in src/lib.rs / src/main.rs, and convenience aliases (in
.cargo/config.toml) provide the CI gates:

```
$ cargo fmt          # rewrite to canonical style
$ cargo fmt-check    # verify formatting without rewriting (CI gate)
$ cargo lint         # clippy --all-targets, warnings as errors (CI gate)
```

Markdown docs use the same formatter-then-linter split, kept Rust-native via
[`rumdl`](https://github.com/rvben/rumdl) (`cargo install rumdl --locked`),
no Node/npm required. The rule posture lives in .rumdl.toml.

```
$ rumdl fmt .         # rewrite Markdown to canonical style
$ rumdl fmt --check . # verify formatting without rewriting (CI gate)
$ rumdl check .       # lint, non-zero exit on any finding (CI gate)
```

Markdown is kept ASCII-only as well, so docs stay portable and diffs stay
clean: no em dashes, curly quotes, or other non-ASCII punctuation (use
parentheses or commas for asides, and reserve `--` for documenting
command-line flags). CI fails on any non-ASCII character, via the same
[`rg`](https://github.com/BurntSushi/ripgrep) scan you can run locally (test
fixtures under tests/data are exempt, since they may hold deliberate
non-ASCII):

```
$ rg -n --column -g '*.md' -g '!tests/data/**' '[^\x00-\x7F]'
```

### Test coverage

Coverage is measured with source-based LLVM instrumentation via
[`cargo-llvm-cov`](https://github.com/taiki-e/cargo-llvm-cov)
(cross-platform, unlike `tarpaulin`). One-time setup, then run a coverage
alias:

```
$ rustup component add llvm-tools-preview
$ cargo install cargo-llvm-cov

$ cargo cov          # per-file coverage table
$ cargo cov-gate     # fail if line coverage drops below the threshold
$ cargo cov-html     # browsable report under target/llvm-cov/html/
```

`cargo cov-gate` is the CI gate, so coverage is enforced rather than merely
observed.

### License

ripref is dual-licensed under the MIT license and the Unlicense (see
[LICENSE](LICENSE)). You may use it under the terms of either.
