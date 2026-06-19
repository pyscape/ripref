ripref (rr)
-----------
ripref is a tool for citing code by stable anchors instead of fragile line
numbers. It recursively indexes the current directory into a single compact,
memory-mapped index that maps each anchor (a file path, a symbol, a document
heading, a decision record, an API operation) to its location in the tree.
Once the index is built, ripref can dereference an anchor, find everything that
cites it across documentation and code, and flag references that have drifted
or gone dangling as the code changes. ripref is language agnostic and has
first-class support on Windows, macOS and Linux.

A reference like `parser.go:42` is wrong the moment a line is inserted above
it, and nothing tells you it broke. An anchor names *what* you meant (the
function, the heading, the decision) and stays correct across edits, moves and
refactors. ripref makes that the cheap default and turns reference rot into a
build-time (or lint-time) error.

Dual-licensed under MIT or the [UNLICENSE](https://unlicense.org).

> **Status: pre-release, in active design.** ripref is being specified
> README-first; the pages under [`doc/`](doc/) are the working spec the
> implementation is being built to. The commands below describe intended
> behavior.

### Documentation

The full reference lives in [`doc/`](doc/):

* [`doc/CLI.md`](doc/CLI.md): every command, flag, exit code and example.
* [`doc/FORMAT.md`](doc/FORMAT.md): the on-disk index byte layout.
* [`doc/JSON.md`](doc/JSON.md): the `--format json` output schema.
* [`doc/examples/clam.rr.toml`](doc/examples/clam.rr.toml): an example
  configuration profile.

In-page: [Quick examples](#quick-examples) ·
[Why use it](#why-should-i-use-ripref) · [Anchors](#anchors) ·
[How it works](#how-it-works) · [Configuration](#configuration) ·
[Shared options](#shared-options) · [Installation](#installation)

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
only inside that editor. It has no idea that a design doc, a `.feature` spec,
an architecture decision record, or an OpenAPI operation refers to the same
thing. That is the gap ripref fills. It indexes prose and code into one anchor
namespace, so a single

```
$ rr search my_module::handler
```

lists every citation in documentation, specs and code at once, and runs in a
pre-commit hook or in CI, not just interactively in one editor. Getting that
out of vim or vscode is neither cheap nor easy; here it is one command.

The rest follows from that:

* ripref references code by *meaning*, not by line number, so your docs,
  comments and specs keep pointing at the right thing across edits and
  refactors.
* ripref is fast. Every read memory-maps a single prebuilt index and
  binary-searches it (no rescan, no deserialization, no allocation) so a
  lookup costs microseconds and the thousands of lookups a build fans out share
  one page-cached copy.
* ripref checks freshness with a single `stat` per in-scope file. There is no
  content hashing and no `git` on the read path, and when the index is stale
  ripref tells you (exit code 3) instead of returning a stale answer.
* ripref is language agnostic. Out of the box it resolves file paths, document
  headings, scenarios, decision records, manifest keys and API operations;
  language-specific symbols come from plugins, so teaching ripref a new
  language is a query file, not a patch to ripref.
* ripref degrades gracefully. When the index is stale and you can't rebuild,
  fall back to [ripgrep](https://github.com/BurntSushi/ripgrep) (always fresh,
  if slower) as a correct floor.
* ripref has structured output (`--format json`) so it composes cleanly with
  editors, linters and CI.

In other words, use ripref if you want references that span documentation and
code, that survive change, and that are checked fast enough to run on every
every commit, save, or even read.

### Why shouldn't I use ripref?

* You only need to *find* text. If you're searching for a pattern rather than
  resolving a named reference, use
  [ripgrep](https://github.com/BurntSushi/ripgrep) or grep, that's what
  they're for.
* You work entirely inside code, in a single editor, and never cite it from
  documentation or specs. If your editor's "find references" already covers
  what you need, ripref may be more than you want.
* The language or artifact you care about has no plugin yet and you don't want
  to write one. (Please file an issue, or a plugin.)

### Anchors

An *anchor* is the stable identity an artifact already has. Every command
accepts the same grammar:

| Anchor kind | Example |
| ----------- | ------- |
| file path | `src/server/http.go` |
| symbol | `my_module::handler` |
| scenario | `tests/features/auth.feature#"User can log in"` |
| record | `AD-42` |
| heading | `docs/guide.md#configuration` |
| manifest key | `pyproject.toml#[tool.poetry] name` |
| API operation | `createUser` |

A bare line number such as `http.go:42` is never an anchor, it is exactly the
thing `rr enforce` flags. Which concrete patterns map to which kind is set by
configuration, not hardcoded; see [Configuration](#configuration) and the full
grammar in [`doc/CLI.md`](doc/CLI.md).

### How it works

ripref splits cleanly into one writer and many readers.

`rr index` is the single writer. It scans the working tree (respecting
`.gitignore` by default), extracts every anchor and its location, and writes
one flat, sorted, memory-mappable file (by default `.ref-cache/index`). It is
the only part of ripref that runs `git`, which it uses once to stamp the index.
Keep it running with `rr index --watch` to rebuild as files change.

`read`, `search` and `enforce` are readers. They memory-map the index and
binary-search it: no per-call rescan, no deserialization, no allocation, and a
single page-cached copy shared across every concurrent reader. This is why a
lookup costs microseconds and a build that fans out thousands of them doesn't
fall over.

Freshness is a `stat`, not a hash. The index records its build time; a reader
compares that against the newest modification time among the files it would
resolve against. If anything is newer, the index is stale and the reader exits
3 rather than answer from stale data. Recover by re-running `rr index`, or fall
back to ripgrep, which is always fresh.

The on-disk index format is specified in [`doc/FORMAT.md`](doc/FORMAT.md).

### Configuration

ripref ships language-agnostic defaults( it respects `.gitignore` and indexes
everything that isn't ignored, just like ripgrep. Project-specific conventions)
which paths are in scope, what counts as a "record" or a scenario, which files
are durable prose subject to enforcement, are **configuration**, not built-in
rules, so ripref stays general and any one project's conventions are just a
profile on top of it.

Drop a `.rr.toml` at the repository root to set them.
[`doc/examples/clam.rr.toml`](doc/examples/clam.rr.toml) is a worked profile
that teaches ripref one project's conventions end to end (scope, the anchor
kinds it recognizes, and per-language scan rules) using only built-in
extractors. Additional languages plug in as query files rather than patches to
ripref.

### Shared options

Every command accepts:

* `--index <path>` (or the `REF_INDEX` environment variable): location of the
  index. Defaults to `.ref-cache/index`.
* `--format text|json`: human-readable text (default), or one JSON document
  for piping into other tools (see [`doc/JSON.md`](doc/JSON.md)).
* `--color auto|always|never`: when to colorize output (default `auto`);
  `--no-color` is shorthand for `--color never`.

Exit codes are consistent across commands:

* `0`: success.
* `1`: findings: `enforce` saw violations, or `read`/`search` found nothing or
  an ambiguous match.
* `2`: usage error.
* `3`: the index is stale; rebuild with `rr index`, or fall back to ripgrep.

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

ripref has both unit tests and integration tests. To run the full suite, use:

```
$ cargo test --all
```

from the repository root.

### License

ripref is dual-licensed under the [MIT license](LICENSE-MIT) and the
[Unlicense](UNLICENSE). You may use it under the terms of either.
