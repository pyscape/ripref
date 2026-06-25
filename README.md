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

> [!IMPORTANT]
> **Status: pre-release, in active design.** The commands below describe intended
> behavior, and not all of it is implemented yet:
>
> - Working today: `rr index`, `rr read` (a bare anchor, a pinned
>   `anchor@commit` / `anchor~commit`, or a pasted `[[rr:...]]` citation marker),
>   `rr at` (with `--cite` to emit the marker), `rr cite`, `rr track`,
>   `rr verify`, `rr uncite`, `rr untrack`.
> - Planned, not yet implemented: `rr search`, `rr enforce`, and the
>   `rr index --watch` mode.
>
> Reference docs are generated from the code (rustdoc and `rr --help`).

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

Go the other way: ask which anchor covers a line you are looking at. `rr at`
takes a `file:line` and prints the anchor identity (the address) of the tightest
(innermost) anchor covering that line. Pass that identity to `rr read` to resolve
it; add `--cite` to emit the document citation marker `[[rr:anchor]]` (the form
you paste into prose) instead of the bare address:

```
$ rr at src/handlers.py:15
my_module::handler

$ rr at src/handlers.py:15 --cite
[[rr:my_module::handler]]
```

Add `--all` to see the whole nest the line sits in, outermost (whole file) first,
for when the tightest anchor is not the one you mean:

```
$ rr at src/handlers.py:15 --all
src/handlers.py
my_module::handler
```

The text output is anchor names only, never line numbers (the fragile coordinate
ripref replaces); `--format json` carries the full list with spans, for editors
and agents citing the code under a cursor.

> [!NOTE]
> The examples from here down (`rr search`, `rr enforce`) are planned and not yet
> implemented; they show intended behavior. Everything above (`rr index`,
> `rr read`, `rr at`) works today.

Find every reference to an anchor (across documentation and code) before you
change or delete it. `rr search` finds the `[[rr:...]]` citation markers written
in prose, comments, and commit messages; the anchor argument itself stays bare:

```
$ rr search my_module::handler   # planned, not yet implemented
docs/architecture.md:14
docs/api/handlers.md:7
tests/features/requests.feature:22
src/handlers.py:8
```

Flag drift and dangling references, for a pre-commit hook or CI. `rr enforce`
scans for `[[rr:...]]` markers and reports those that are dangling or stale:

```
$ rr enforce --changed   # planned, not yet implemented
docs/data-model.md:51: dangling reference: legacy.Account (no such anchor)
README.md:88: line-number reference: parser.go:42 (use an anchor)
```

Emit JSON instead of text for piping into other tools:

```
$ rr search my_module::handler --format json   # planned, not yet implemented
```

### Why should I use ripref?

Your editor already jumps to a symbol's references, but only within code and
only inside that editor. It has no idea that a design doc, a `.feature` spec, an
architecture decision record, or an OpenAPI operation refers to the same thing.
That is the gap ripref fills. It indexes prose and code into one anchor
namespace, so a single

```
$ rr search my_module::handler   # planned, not yet implemented
```

lists every citation in documentation, specs and code at once, and runs in a
pre-commit hook or in CI, not just interactively in one editor. Getting that out
of vim or vscode is neither cheap nor easy; here it is one command.

The rest follows from that:

- ripref references code by _meaning_, not by line number, so your docs,
  comments and specs keep pointing at the right thing across edits and
  refactors.
- ripref reads are cheap. Every read memory-maps a single prebuilt index (one
  page-cached copy shared across all readers) and resolves the anchor against
  the sorted forward section. The mapping and header parse are microseconds;
  the lookup itself currently materializes that section on every call (an O(n)
  allocation; see [BENCHMARKS.md](BENCHMARKS.md)), with an in-place bisect
  planned.
- ripref checks freshness cheaply and without ever hashing file contents: a
  single `stat` per in-scope file, or, on a clean working tree, one `git status`
  plus a `rev-parse` to confirm `HEAD` still matches the index stamp (which
  short-circuits the stat walk). So a live read does run `git` for the freshness
  check; what it never does is hash file contents. When the index is stale ripref
  tells you (exit code 3) instead of returning a stale answer.
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
kind of reference `rr enforce` (planned) is designed to flag. Which concrete
patterns map to which kind is set by configuration, not hardcoded; see
[Configuration](#configuration).

An anchor plays two distinct roles. As an **address** it is the bare identity
you pass to `rr read`, `rr at`, or `rr cite` on the CLI; it stays bare.
As a **citation** it is the delimited form `[[rr:anchor]]` that you write INTO
a document (prose, comment, commit message) so that `rr search` and `rr enforce`
can find it by scanning. The kinds table above shows address forms; that is how
you refer to an anchor on the command line. When you write a reference into a
document, wrap it: `[[rr:my_module::handler]]`, `[[rr:AD-42]]`. See
`doc/adr/0001-citation-syntax.md` (AD-1) for the full rationale and marker
grammar.

### How it works

ripref splits cleanly into one writer and many readers.

`rr index` is the single writer. It scans the working tree (respecting
`.gitignore` by default), extracts every anchor and its location, and writes one
flat, sorted, memory-mappable file (by default `.ref-cache/index`). It is the
only part of ripref that runs `git`, which it uses once to stamp the index. A
planned `rr index --watch` mode (not yet implemented) will keep it running to
rebuild as files change.

`read` and `at` are the readers today; `search` and `enforce` are planned (not
yet implemented). A reader memory-maps the index, keeping one page-cached copy
shared across every concurrent reader, and answers each query from the forward
section. The mapping and header parse are microseconds; the lookup currently
scans and materializes the forward records on every call (an O(n) allocation
today; see [BENCHMARKS.md](BENCHMARKS.md)), with an in-place bisect planned.

#### Freshness

Freshness is a `stat`, not a hash. The index records its build time; a reader
compares that against the newest modification time among the files it would
resolve against. If anything is newer, the index is stale and the reader exits 3
rather than answer from stale data. Recover by re-running `rr index`, or fall
back to ripgrep, which is always fresh. The comparison is second-granular
(filesystem `mtime` resolution); a file written within the same second as the
index build is treated as fresh by design.

At repository scale it is this freshness walk, not the lookup, that dominates a
query, so the stat-walk runs in parallel (`std::thread::scope`, no added
dependency). On a real ~2,200-file, ~26k-anchor tree the parallel walk is about
15 ms, roughly 3x faster than the serial reduction it replaced, and the gap
widens as the tree grows (see the
[`freshness`](benches/freshness.rs) micro-benchmark). Two paths skip the walk
entirely: `--no-freshness` answers from the index as-is, and a clean working tree
whose `HEAD` matches the index stamp is known-fresh without any `stat` at all.

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
| `language_init` (load the grammar)     | ~1 ns   | ~145 ms     |
| `query_compile` (compile the query)    | ~1 ms   | ~0.8 ms     |
| `parse` + extract (small doc / README) | 33 us / 5.9 ms | 49 us / 8.6 ms |

The headline is `language_init`. Native is a pointer wrap; the WebAssembly path
pays a one-time ~145 ms to compile the grammar module (**per process start**),
because tree-sitter's `WasmStore` does not expose wasmtime's ahead-of-time
(`Module::serialize`) cache. With that cache the load drops to well under a
millisecond; [`examples/wasm_load_probe.rs`](examples/wasm_load_probe.rs)
measures the decomposition and that cached ceiling. Once loaded, query
compilation is at parity and parsing is ~1.5x slower under the sandbox.

So ripref keeps its built-in languages native and treats the WebAssembly path as
opt-in extensibility; making it cheap enough for routine use means teaching the
loader to cache compiled modules, which `WasmStore` would first need to expose.

### Snapshots and tracking references

A live anchor always resolves to current content. Two further intents let a
reference depend on a _version_ of an anchor:

- A **snapshot** freezes an anchor's content so you can recover it later, even
  after history is rewritten. `rr cite <anchor>` stores the anchor's file as it
  is at `HEAD` and prints a pinned citation marker `[[rr:anchor]]@<short-commit>`.
  `rr read [[rr:anchor]]@<short-commit>` (or the bare `anchor@<short-commit>`)
  then prints that frozen source.
- A **tracking** reference baselines an anchor so you are told when it drifts.
  `rr track <anchor>` records the current content and prints
  `[[rr:anchor]]~<short-commit>`. `rr read [[rr:anchor]]~<short-commit>` (and
  `rr verify`) report whether the anchor's file still matches that baseline: `OK`,
  `OK (moved)` if it was renamed but is unchanged, or `DRIFTED` (exit 4) if its
  content changed.

```
$ rr cite docs/guide.md#configuration
[[rr:docs/guide.md#configuration]]@a1b2c3d

$ rr read '[[rr:docs/guide.md#configuration]]@a1b2c3d'
docs/guide.md@a1b2c3d:40-83
## Configuration
...the frozen section...
```

The evidence is durable and lives in a committed, content-addressed sidecar
under `.rr/` (a manifest `.rr/refs` plus an object store `.rr/objects/`), not in
the derived index. Because the bytes are stored (not a git pointer), a snapshot
recovers even after the commit it was taken at is rebased or force-pushed away
and garbage-collected, and after the file is renamed. Each object is named by its
git blob id, so recovery re-hashes the bytes and verifies them against that name;
a corrupt or missing object is reported broken (exit 5) rather than shown as
evidence. `rr cite` writes `.rr/` into your working tree for you to commit; it
refuses a path that is not committed as-is, or that uses a clean filter or
Git-LFS (whose stored bytes would not be what you saw).

`rr verify` classifies pinned references as ok / drifted / moved / broken,
returning the worst exit code, and fails closed if a committed manifest line was
removed without an explicit `rr uncite` / `rr untrack` tomb. It is a good fit for
a pre-commit hook or a CI job. Drift is always a content comparison
(`git hash-object` against the stored baseline), so it catches an edit even when
the file's size and modification time are unchanged or the edit is hidden from
`git status` by `--skip-worktree`.

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
- `1`: findings: `read` found nothing or an ambiguous match, or `at` found no
  anchor covering the line (the planned `enforce` will also use `1` for
  violations, and `search` for no matches).
- `2`: usage error.
- `3`: the index is stale; rebuild with `rr index`, or fall back to ripgrep.
- `4`: drifted: a tracked reference's current content differs from the baseline
  it was pinned at (`rr read anchor~commit`, `rr verify`).
- `5`: broken: a snapshot or commit that cannot be resolved or recovered, or an
  ambiguous pin (`rr read anchor@commit`, `rr verify`). `verify` returns the
  worst code across the references it checked.

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
