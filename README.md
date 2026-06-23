# ripref (rr)

ripref is a tool for citing code and documentation by stable anchors instead of
fragile line numbers. It recursively indexes the current directory into a single
compact, memory-mapped index that maps each anchor (a file path, a symbol, a
document heading, a decision record, an API operation) to its location in the
tree. Once the index is built, ripref can dereference an anchor, turn a file and
line back into the anchors that cover it, find everything that cites it across
documentation and code, and flag references that have drifted or gone dangling
as the code changes. ripref is language agnostic and has first-class support on
Windows, macOS and Linux.

A reference like `parser.go:42` is wrong the moment a line is inserted above it,
and nothing tells you it broke. An anchor names _what_ you meant (the function,
the heading, the decision) and stays correct across edits, moves and refactors.
ripref makes that the cheap default and turns reference rot into a build-time
(or lint-time) error.

Dual-licensed under MIT or the [UNLICENSE](https://unlicense.org).

> **Status: pre-release, in active design.** Reference docs are generated from
> the code (rustdoc and `rr --help`). The commands below describe intended
> behavior.

## Documentation

ripref is documented from the code:

- `rr --help` lists every command and flag; the help text is generated from the
  flag definitions, so it can't drift from the parser.
- `cargo doc --open` renders the API, including the on-disk index byte layout,
  which is specified in the `refidx` module (`src/refidx.rs`).
- Built-in defaults are in [`rr.toml`](rr.toml); the build, tests and coverage
  are covered in the sections below.

In-page: [Quick examples](#quick-examples),
[Why use it](#why-should-i-use-ripref), [Anchors](#anchors),
[How it works](#how-it-works), [Configuration](#configuration),
[Shared options](#shared-options), [Installation](#installation)

### Quick examples

Build the index for the current directory:

```
$ rr index
indexed 1,204 anchors across 318 files
```

Dereference an anchor to see what it points at:

```
$ rr read my_module::handler
src/handlers.py:8-26
def handler(request):
    ...
```

Go the other way: turn a `file:line` back into the anchors whose definition
covers it, listed outermost (whole file) first. Each line is an anchor and its
location, so it feeds straight back into `rr read`, handy for an editor or agent
that wants to cite the code under a cursor (columns are tab-separated):

```
$ rr at src/handlers.py:15
src/handlers.py       src/handlers.py:1-40
my_module::handler    src/handlers.py:8-26
```

Find every reference to an anchor (across documentation and code) before you
change or delete it:

```
$ rr search my_module::handler
docs/architecture.md:14
docs/api/handlers.md:7
tests/features/requests.feature:22
src/handlers.py:8
```

Flag drift and dangling references, for a pre-commit hook or CI:

```
$ rr enforce --changed
docs/data-model.md:51: dangling reference: legacy.Account (no such anchor)
README.md:88: line-number reference: parser.go:42 (use an anchor)
```

Emit JSON instead of text for piping into other tools:

```
$ rr search my_module::handler --format json
```

### Why should I use ripref?

Your editor already jumps to a symbol's references, but only within code and
only inside that editor. It has no idea that a design doc, a `.feature` spec, an
architecture decision record, or an OpenAPI operation refers to the same thing.
That is the gap ripref fills. It indexes prose and code into one anchor
namespace, so a single

```
$ rr search my_module::handler
```

lists every citation in documentation, specs and code at once, and runs in a
pre-commit hook or in CI, not just interactively in one editor. Getting that out
of vim or vscode is neither cheap nor easy; here it is one command.

The rest follows from that:

- ripref references code by _meaning_, not by line number, so your docs,
  comments and specs keep pointing at the right thing across edits and
  refactors.
- ripref is fast. Every read memory-maps a single prebuilt index and
  binary-searches it (no rescan, no deserialization, no allocation) so a
  lookup costs microseconds and the thousands of lookups a build fans out share
  one page-cached copy.
- ripref checks freshness with a single `stat` per in-scope file. There is no
  content hashing and no `git` on the read path, and when the index is stale
  ripref tells you (exit code 3) instead of returning a stale answer.
- ripref is language agnostic. Out of the box it resolves file paths, document
  headings, scenarios, decision records, manifest keys and API operations;
  language-specific symbols come from per-language Tree-sitter grammars paired
  with a small query. First-class languages are built in; third-party grammars
  load from WebAssembly without rebuilding ripref.
- ripref degrades gracefully. When the index is stale and you can't rebuild,
  fall back to [ripgrep](https://github.com/BurntSushi/ripgrep) (always fresh,
  if slower) as a correct floor.
- ripref has structured output (`--format json`) so it composes cleanly with
  editors, linters and CI.

In other words, use ripref if you want references that span documentation and
code, that survive change, and that are checked fast enough to run on every
commit, save, or even read.

### Why shouldn't I use ripref?

- You only need to _find_ text. If you're searching for a pattern rather than
  resolving a named reference, use
  [ripgrep](https://github.com/BurntSushi/ripgrep) or grep, that's what
  they're for.
- You work entirely inside code, in a single editor, and never cite it from
  documentation or specs. If your editor's "find references" already covers
  what you need, ripref may be more than you want.
- The language or artifact you care about isn't supported yet and you don't
  want to add it. (Please file an issue, or contribute a grammar + query.)

### Anchors

An _anchor_ is the stable identity an artifact already has. Every command
accepts the same grammar:

| Anchor kind   | Example                                         |
| ------------- | ----------------------------------------------- |
| file path     | `src/server/http.go`                            |
| symbol        | `my_module::handler`                            |
| scenario      | `tests/features/auth.feature#"User can log in"` |
| record        | `AD-42`                                         |
| heading       | `docs/guide.md#configuration`                   |
| manifest key  | `pyproject.toml#[tool.poetry] name`             |
| API operation | `createUser`                                    |

A bare line number such as `http.go:42` is never an anchor, it is exactly the
thing `rr enforce` flags. Which concrete patterns map to which kind is set by
configuration, not hardcoded; see [Configuration](#configuration).

### How it works

ripref splits cleanly into one writer and many readers.

`rr index` is the single writer. It scans the working tree (respecting
`.gitignore` by default), extracts every anchor and its location, and writes one
flat, sorted, memory-mappable file (by default `.ref-cache/index`). It is the
only part of ripref that runs `git`, which it uses once to stamp the index. Keep
it running with `rr index --watch` to rebuild as files change.

`read`, `at`, `search` and `enforce` are readers. They memory-map the index and
binary-search it: no per-call rescan, no deserialization, no allocation, and a
single page-cached copy shared across every concurrent reader. This is why a
lookup costs microseconds and a build that fans out thousands of them doesn't
fall over.

#### Freshness

Freshness is a `stat`, not a hash. The index records its build time; a reader
compares that against the newest modification time among the files it would
resolve against. If anything is newer, the index is stale and the reader exits 3
rather than answer from stale data. Recover by re-running `rr index`, or fall
back to ripgrep, which is always fresh. The comparison is second-granular
(filesystem `mtime` resolution); a file written within the same second as the
index build is treated as fresh by design.

The on-disk index format is specified in the `refidx` module (`src/refidx.rs`).

#### Languages: native and WebAssembly

Extracting a language's anchors pairs a
[Tree-sitter](https://tree-sitter.github.io/) grammar with a small query (see
[`src/languages`](src/languages/README.md)). A grammar can reach ripref two
ways, and they cost very differently to load:

- **First-class (native).** The grammar is a Rust crate dependency
  (`tree-sitter-rust`, `tree-sitter-md`, ...) compiled into the binary. Loading
  it is a function-pointer wrap (effectively free).
- **Third-party (WebAssembly).** The grammar ships as a prebuilt `parser.wasm`
  loaded at runtime, so adding a language needs no rebuild of ripref. The
  runtime compiles the module on load.

[`benches/grammar_loader.rs`](benches/grammar_loader.rs) measures both on the
same markdown grammar (`cargo bench --bench grammar_loader --features wasm`):

| operation                              | native  | WebAssembly |
| -------------------------------------- | ------- | ----------- |
| `language_init` (load the grammar)     | ~1 ns   | ~140 ms     |
| `query_compile` (compile the query)    | ~0.8 ms | ~0.8 ms     |
| `parse` + extract (small doc / README) | 29 us / 3.8 ms | 44 us / 5.9 ms |

The headline is `language_init`. Native is a pointer wrap; the WebAssembly path
pays a one-time ~140 ms to compile the grammar module (**per process start**),
because tree-sitter's `WasmStore` does not expose wasmtime's ahead-of-time
(`Module::serialize`) cache. With that cache the load drops to well under a
millisecond; [`examples/wasm_load_probe.rs`](examples/wasm_load_probe.rs)
measures the decomposition and that cached ceiling. Once loaded, query
compilation is at parity and parsing is ~1.5x slower under the sandbox.

So ripref keeps its built-in languages native and treats the WebAssembly path as
opt-in extensibility; making it cheap enough for routine use means teaching the
loader to cache compiled modules, which `WasmStore` would first need to expose.

### Configuration

ripref ships language-agnostic defaults( it respects `.gitignore` and indexes
everything that isn't ignored, just like ripgrep. Project-specific conventions)
which paths are in scope, what counts as a "record" or a scenario, which files
are durable prose subject to enforcement, are **configuration**, not built-in
rules, so ripref stays general and any one project's conventions are just a
profile on top of it.

Drop a `.rr.toml` at the repository root to override them; rr's built-in
defaults (the base layer your config merges over) are in [`rr.toml`](rr.toml) at
the repository root. A project's own `.rr.toml` can teach ripref its conventions
end to end (scope, the anchor kinds it recognizes, and per-language scan rules)
using only built-in extractors. Additional languages are a Tree-sitter grammar
plus a query, built in or loaded from WebAssembly.

### Shared options

Every command accepts:

- `--index <path>` (or the `REF_INDEX` environment variable): location of the
  index. Defaults to `.ref-cache/index`.
- `--format text|json`: human-readable text (default), or one JSON document
  for piping into other tools.
- `--color auto|always|never`: when to colorize output (default `auto`);
  `--no-color` is shorthand for `--color never`.

Exit codes are consistent across commands:

- `0`: success.
- `1`: findings: `enforce` saw violations, `read`/`search` found nothing or an
  ambiguous match, or `at` found no anchor covering the line.
- `2`: usage error.
- `3`: the index is stale; rebuild with `rr index`, or fall back to ripgrep.

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
[Rust installation](https://www.rust-lang.org/) in order to compile it. ripref
compiles with the latest stable release of the Rust compiler. To build:

```
$ git clone https://github.com/pyscape/ripref
$ cd ripref
$ cargo build --release
$ ./target/release/rr --version
```

### Running tests

ripref has both unit tests (each component in isolation) and integration tests
(the compiled `rr` binary driven end to end against the spec). To run the full
suite, use:

```
$ cargo test --all
```

from the repository root.

### Debugging

VS Code is configured for step-through debugging of the integration tests in
`tests/cli.rs`. You need two extensions:

- [`rust-lang.rust-analyzer`](https://marketplace.visualstudio.com/items?itemName=rust-lang.rust-analyzer)
- [`ms-vscode.cpptools`](https://marketplace.visualstudio.com/items?itemName=ms-vscode.cpptools)

Open the Extensions panel; both appear under **Workspace Recommendations** and
can be installed from there. CodeLLDB is not used; it crashes on Windows with
the MSVC toolchain (exit `0xC0000005`).

Once installed, two ways to launch a debug session:

- **CodeLens** (quickest): open `tests/cli.rs`, click the **Debug** link that
  appears above any `#[test]` function.
- **Run and Debug panel** (`Ctrl+Shift+D`, then `F5`): pick
  **"Debug: tests/cli.rs (all tests)"** to run every test under the debugger,
  or **"Debug: tests/cli.rs (filter by name)"** to be prompted for a substring
  and run only matching tests.

Both launch configurations run `cargo test --test cli --no-run` before
attaching, so the binary is always current. Set breakpoints anywhere in `src/`
or `tests/` before pressing `F5`.

### Linting and formatting

Rust style is enforced with `rustfmt` (formatter) and `clippy` (linter), both
bundled with the toolchain. The lint posture is declared as crate-level
attributes in `src/lib.rs` / `src/main.rs`, and convenience aliases (in
`.cargo/config.toml`) provide the CI gates:

```
$ cargo fmt          # rewrite to canonical style
$ cargo fmt-check    # verify formatting without rewriting (CI gate)
$ cargo lint         # clippy --all-targets, warnings as errors (CI gate)
```

Markdown docs use the same formatter-then-linter split, kept Rust-native via
[`rumdl`](https://github.com/rvben/rumdl) (`cargo install rumdl --locked`), no
Node/npm required. The rule posture lives in `.rumdl.toml`.

```
$ rumdl fmt .         # rewrite Markdown to canonical style
$ rumdl fmt --check . # verify formatting without rewriting (CI gate)
$ rumdl check .       # lint, non-zero exit on any finding (CI gate)
```

Markdown is kept ASCII-only as well, so docs stay portable and diffs stay clean:
no em dashes, curly quotes, or other non-ASCII punctuation (use parentheses or
commas for asides, and reserve `--` for documenting command-line flags). CI
fails on any non-ASCII character, via the same
[`rg`](https://github.com/BurntSushi/ripgrep) scan you can run locally (test
fixtures under `tests/data` are exempt, since they may hold deliberate
non-ASCII):

```
$ rg -n --column -g '*.md' -g '!tests/data/**' '[^\x00-\x7F]'
```

### Test coverage

Coverage is measured with source-based LLVM instrumentation via
[`cargo-llvm-cov`](https://github.com/taiki-e/cargo-llvm-cov) (cross-platform,
unlike `tarpaulin`). One-time setup, then run a coverage alias:

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

ripref is dual-licensed under the [MIT license](LICENSE-MIT) and the
[Unlicense](UNLICENSE). You may use it under the terms of either.
