# Editor integration: complete a reference as you type

> [!NOTE]
> The headline command here, `rr complete`, is a proposal: it is not implemented
> and not in the current command set. The implemented and in-progress commands
> resolve and pin references (`rr read`, `rr at`, and the new `rr cite` /
> `rr track` / `rr verify`), but none enumerates anchors by prefix, and none emits
> the anchor namespace for a plugin to filter on its own; `rr index --format json`
> reports counts, not anchor names. This document is the guiding spec for that
> work: it describes the intended workflow and, in
> [What rr must provide](#what-rr-must-provide), the exact capabilities rr needs
> to support it well, each tagged with its status.

Your editor completes the symbols it has parsed: in code, in one language,
inside its own project model. Citing those symbols is a different act. When you
write a reference into a design doc, a comment, a `.feature` spec, or an ADR,
you type an anchor name from memory with no completion at all, and a mistyped or
guessed anchor is a reference that resolves to nothing. The editor cannot help
here: it does not know rr's anchor namespace, and its completion does not even
fire in Markdown, TOML, or gherkin. This use case feeds rr's index to the editor
as a completion source, so typing the start of an anchor offers the real anchors
that exist (across code and prose), and the citation you insert is correct the
moment you write it.

## The pattern

As you type an anchor, the editor queries the index for anchors that share your
prefix and offers them; resolving the highlighted one fills a preview pane:

```
rr complete <prefix>                # proposed: anchors with this name prefix
rr complete <prefix> --format json  # proposed: matches with kind + location
rr read <anchor>                    # today: resolve a candidate for the preview
rr read <anchor> --locate           # today: just file:start-end, no body
```

- Query the prefix under the cursor. `rr complete my_module::han` bisects the
  index's sorted `forward` section to the run of anchors beginning
  `my_module::han` and returns them, so the plugin offers real anchors, never a
  guess.
- Complete where the LSP stays silent. The candidates are anchors, not language
  symbols, so completion is meant to fire in Markdown, `.feature`, TOML, and
  comments, the non-code files where you write citations and the editor's own
  completion offers nothing.
- Preview the target inline. For the highlighted candidate, `rr read <anchor>`
  returns its location and body, so the detail pane shows the code a citation
  points at before you commit to it.
- What you insert is the anchor, the durable handle, never a line number;
  `rr read` reverses it to a location when a reader later follows it. The anchor
  you complete is a live reference by default; when a citation needs to freeze the
  exact version you saw (the evidence behind a decision) or to warn you when its
  target changes, promote it with `rr cite` (a frozen `anchor@<commit>` snapshot)
  or `rr track` (a `anchor~<commit>` that `rr verify` flags on drift). Completion
  supplies the anchor; `cite` and `track` add the pin.

Worked example: writing a design doc, you type the start of the handler's
anchor.

```
docs/architecture.md
> The request flow starts in my_module::han|
```

`rr complete my_module::han` returns `my_module::handle_retry` and
`my_module::handler`; the plugin offers both, and the detail pane shows the
picked anchor's body from `rr read my_module::handler`. The doc cites an anchor
that exists and keeps pointing at the handler after the code around it shifts.

## What rr must provide

This use case is the contract it puts on rr. Each capability below is what a
good completion integration needs, why it needs it, and where rr stands.
Status tags: `[today]` ships now, `[in progress]` is being built with the
snapshot/tracking work, `[planned]` is in the CLI spec but unimplemented,
`[proposed]` is new and specified here.

- **Prefix enumeration, `rr complete <prefix>` `[proposed]`.** The core query:
  take the partial anchor under the cursor, return the anchors that start with
  it. The `forward` section is already sorted by anchor name and binary-searched,
  so this is a bisect to the prefix run, then a walk while the key still matches
  (a `Reader::prefix_lookup` beside the existing `forward_lookup`): no new index
  section, no format change, no new dependency. An empty prefix enumerates the
  whole namespace, which also gives agents the anchor map that
  [a stable codebase map](agent-codebase-map.md) assumes.
- **Name, kind, and location on every match `[proposed]`.** Each result must
  carry the anchor name (the token the editor inserts), its kind (symbol,
  heading, record, scenario, operation, path: for an icon, for grouping, for the
  kind filter below), and its location (`file:start-end`, for the preview and a
  jump). rr's JSON envelopes should all carry `kind` (the `at` envelope ships
  today; `read`, `cite`, `track`, `verify`, and `complete` should match), so
  completion has it on each candidate. The `forward` record stores no kind, so
  derive it from the anchor grammar at query time: no new index field needed.
- **A result bound and a stable order, `--limit <N>` `[proposed]`.** A one- or
  two-character prefix can match thousands of anchors; a popup wants the top N,
  not the tree. `forward` is a deterministic total order (sorted by name), so
  results do not reshuffle between keystrokes, and `--limit` caps how many cross
  the boundary. Ranking beyond that sort (by kind, by proximity to the current
  file, by recent use) and fuzzy (non-prefix) matching are the plugin's job
  unless a later flag does them.
- **A kind filter, `--kind <symbol|heading|record|...>` `[proposed]`.** Context
  narrows the candidates: only headings after a `#`, only symbols after a `::`.
  The plugin passes the kind implied by the surrounding text so the popup is not
  polluted with the wrong sort of anchor. Depends on kinds being first-class in
  the output, above.
- **Freshness that does not interrupt typing, `--no-freshness` `[today` for
  `read`/`at`, needed on `complete]`.** A reader exits `3` when the index is
  older than the tree, correct for a one-shot resolve but wrong mid-keystroke:
  the popup must still answer. `rr read` and `rr at` already take `--no-freshness`
  to answer from the index as-is; `rr complete` needs the same, so a stale index
  degrades to slightly-old candidates rather than an error.
- **Inline preview, `rr read <anchor>` `[--locate today; body in progress]`.**
  For the highlighted candidate the detail pane shows the target. `--locate`
  gives the `file:start-end` and works today; the source body (bounded by
  `-C`/`--context` so the pane is a snippet, not a 500-line function) is the
  same source-body print being built for `rr read <anchor>@<commit>`, so the
  live-anchor preview lands with it. Only the candidate list it previews is
  missing. Pass `--no-freshness` here too so the preview does not error while
  you type.
- **Current candidates without a manual rebuild, `rr index --watch`
  `[planned]`.** Completion is only as fresh as the last `rr index`, and the
  writer is the expensive path, so rebuilding on every save is the wrong shape.
  The planned `--watch` mode (debounced rebuild on change) keeps the candidate
  set current during a session; until it lands, the plugin re-indexes on save and
  accepts the lag between saves.
- **A cheap per-keystroke invocation `[today` one-shot; daemon `proposed]`.**
  Shelling out to `rr complete` per keystroke only works if process start plus
  mmap-open stays well under the popup's budget (tens of milliseconds). The
  lookup itself is a memory-mapped binary search, microseconds; the cost to watch
  is startup. If that proves too high, a long-lived query process (an
  `rr serve`-style daemon the plugin talks to) is the escape hatch; debouncing
  keystrokes is the plugin's half.
- **Plumbing that already exists `[today]`.** `--index <path>` (or `REF_INDEX`)
  points at the index when the editor's working directory is not the repository
  root; `--no-color` keeps escape codes out of the buffer; exit codes for this live
  reader stay `0` match, `1` none, `2` usage, `3` stale (the pinned and tracked
  reads add `4` drifted and `5` broken, which do not apply to completion).

### The proposed `rr complete` interface

A reader, like `read` and `at`: it opens the index read-only and binary-searches
it. Synopsis:

```
rr complete [--limit <N>] [--kind <KIND>] [--no-freshness] <prefix>
```

Each match is an object `{ "anchor", "kind", "location", "ambiguous" }`: the
`anchor` is the token the editor inserts, `kind` drives the icon and the
`--kind` filter, `location` feeds the preview and a jump, and `ambiguous` is
`true` when the anchor has more than one definition (the plugin then calls
`rr search` to show which). The cases a plugin must handle, request and response
written out in full:

**Default (text): one insertable anchor per line.**

```
$ rr complete my_module::han
my_module::handle_retry
my_module::handler
```

**JSON: the `rr-json` envelope a plugin builds against** (emitted as one line per
invocation, like every `--format json` document; pretty-printed here for
reading).

```
$ rr complete my_module::han --format json
{
  "format": "rr-json",
  "version": 1,
  "command": "complete",
  "data": {
    "prefix": "my_module::han",
    "matches": [
      {
        "anchor": "my_module::handle_retry",
        "kind": "symbol",
        "location": { "file": "src/handlers.py", "start_line": 31, "end_line": 40 },
        "ambiguous": false
      },
      {
        "anchor": "my_module::handler",
        "kind": "symbol",
        "location": { "file": "src/handlers.py", "start_line": 8, "end_line": 26 },
        "ambiguous": false
      }
    ]
  }
}
```

**Kind-filtered: only one sort of anchor, for context-aware completion** (here,
headings after a `#`).

```
$ rr complete --kind heading docs/guide --format json
{
  "format": "rr-json",
  "version": 1,
  "command": "complete",
  "data": {
    "prefix": "docs/guide",
    "matches": [
      {
        "anchor": "docs/guide.md#configuration",
        "kind": "heading",
        "location": { "file": "docs/guide.md", "start_line": 42, "end_line": 42 },
        "ambiguous": false
      }
    ]
  }
}
```

**No match: an empty list and exit `1`, never a guess.**

```
$ rr complete zzz --format json
{
  "format": "rr-json",
  "version": 1,
  "command": "complete",
  "data": { "prefix": "zzz", "matches": [] }
}
$ echo $?
1
```

**Collision: the anchor is offered once, flagged `ambiguous`** so the plugin can
fall back to `rr search` to show both definitions.

```
$ rr complete AD-90 --format json
{
  "format": "rr-json",
  "version": 1,
  "command": "complete",
  "data": {
    "prefix": "AD-90",
    "matches": [
      {
        "anchor": "AD-9001",
        "kind": "record",
        "location": { "file": "plan-a.md", "start_line": 5, "end_line": 5 },
        "ambiguous": true
      }
    ]
  }
}
```

**Empty prefix: the whole namespace** (capped with `--limit`). This is the
agent-facing map and the source a plugin can hold for its own fuzzy matching.

```
$ rr complete '' --limit 1000 --format json   # every anchor, up to the cap
```

**The preview the detail pane shows comes from `rr read`.** `--locate` gives the
location and works today; the body (bounded by `-C`/`--context`) and the `read`
JSON envelope below land with the source-body print being built for snapshot
recovery.

```
$ rr read my_module::handler --locate
src/handlers.py:8-26

$ rr read my_module::handler --format json
{
  "format": "rr-json",
  "version": 1,
  "command": "read",
  "data": {
    "anchor": "my_module::handler",
    "kind": "symbol",
    "resolved": true,
    "ambiguous": false,
    "definitions": [
      {
        "location": { "file": "src/handlers.py", "start_line": 8, "end_line": 26 },
        "body": "def handler(request):\n    ...\n"
      }
    ]
  }
}
```

Exit status across the command: `0` at least one match, `1` none, `2` usage
error, `3` stale.

## Driving it from a plugin

A plugin calls `rr complete <prefix> --format json` on the word under the cursor
and renders the matches; `rr read` fills the detail pane. This is the
read-into-the-buffer companion to [Editor integration: copy a stable
reference](editor-integration.md), which runs the other direction, producing an
anchor from the cursor to paste elsewhere.

- Vim / Neovim: an `omnifunc` or `completefunc`, or an `nvim-cmp` / `blink.cmp`
  source, that shells out to `rr complete` for the current word. Trigger on the
  anchor grammar (`::` for a symbol, `#` for a heading or scenario) so it fires
  mid-citation rather than on every word. The reserved `@` and `~` that begin a
  snapshot or tracking pin come after the anchor, so anchor completion fires
  before them; completing the commit half is a separate source (git revisions,
  or `rr cite` / `rr track` writing it for you).
- VSCode: a `CompletionItemProvider` registered for the languages the built-in
  provider skips (`markdown`, `plaintext`, `gherkin`, `toml`). Map each match to
  a `CompletionItem` whose `detail` is its `location` and whose `documentation`
  is the `rr read` body.
- Point the plugin at the index with `--index <path>` (or `REF_INDEX`) when the
  editor's working directory is not the repository root, pass `--no-color` so
  nothing but the anchor reaches the buffer, and pass `--no-freshness` so a query
  answers from the index as-is instead of refusing with exit `3` mid-keystroke.
  Re-run `rr index` on save (or rely on `--watch`, once it lands) to refresh.

## Limits to know today

- Nothing here runs end-to-end yet. The resolve side (`rr read`, `rr at`) works,
  but the candidate query (`rr complete`) is specified provisionally and not yet
  built, and no command emits the namespace to filter client-side, so a plugin
  has nothing to complete against until the work in
  [What rr must provide](#what-rr-must-provide) lands.
- Completion only produces references; auditing them is separate work. `rr verify`
  (the nearer-term gate) checks the snapshot and tracking references you have
  pinned, flagging drift and broken pins; `rr search` (find every citation of an
  anchor) is planned; broad prose enforcement is deferred. So rr would offer the
  correct anchors as you type and check the pinned references you committed, but
  not yet audit every hand-written citation.

## Why it is worth it

Jump-to-definition made reading code a keystroke. This would make citing it one
too, in the files the LSP never reached: the design doc, the spec, the comment.
You type the start of an anchor and the editor offers the real ones, so a
citation is correct when you write it and stays correct after the code moves,
instead of a hand-typed `file:line` that is wrong on arrival and rots from there.
The cost is one small reader method (`Reader::prefix_lookup`) and a thin command
over it.
