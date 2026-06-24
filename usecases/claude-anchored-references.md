# Anchored references with Claude Code

An AI coding agent (and people) constantly cite code by `file:line` in commit
messages, PR comments, docs, and chat. Those coordinates are wrong the moment a
line is inserted above them, and nothing flags the break. rr replaces that
fragile coordinate with a stable anchor (a symbol, a heading, a record) that
keeps pointing at the right thing across edits, moves, and refactors. This use
case wires rr into a Claude Code workflow so the agent cites anchors, not line
numbers.

## The pattern

Three commands, used as a loop:

```
rr index                      # build / refresh the index
rr at path/to/file.rs:42      # a line -> the anchor you should cite
rr read <anchor>              # an anchor -> its current location, when you need it
```

- To *write* a citation: run `rr at <file>:<line>`, take the anchor it prints,
  and put that anchor in your doc, comment, or message, never the `file:line`.
- To *follow* a citation: `rr read <anchor>` resolves it; add `--locate` for the
  current `file:start-end` to hand to an editor.
- The anchor is the durable reference; a line number is only ever a transient
  resolution. That is the whole point.

Worked example (this repo): cite a benchmark finding by anchor instead of line.

```
$ rr at BENCHMARKS.md:112
Index build: the writer
```

You then cite `Index build: the writer`, not `BENCHMARKS.md:112`.

## Enforcing it with Claude Code

Claude Code hooks act on tool calls, not on the model's prose, so enforcement is
two layers.

1. Block `file:line` from being written into files. A `PreToolUse` hook on
   `Edit` and `Write` scans the new content and rejects a `name.ext:line`
   pattern, returning a message that tells the agent to resolve the anchor with
   `rr at` and cite that instead. Model it on a content-blocking hook you already
   have. Sketch:

   ```
   # ~/.claude/hooks/block-line-refs.py   (PreToolUse: Edit, Write)
   # Read the tool input, scan the new content. If it contains a
   # filename.ext:line outside a fenced code block, and the line is not an
   # `rr at` / `rr read` command, exit non-zero with:
   #   "cite the rr anchor (run: rr at <file>:<line>), not a line number".
   ```

   ```
   # settings.json
   "hooks": { "PreToolUse": [ { "matcher": "Edit|Write",
     "hooks": [ { "type": "command",
       "command": "python ~/.claude/hooks/block-line-refs.py" } ] } ] }
   ```

   Scope it conservatively or it will fire on legitimate `file:line`: exempt
   fenced code blocks, lines that invoke `rr at` / `rr read` / `rr enforce`
   (those carry a `file:line` query or output on purpose), and test fixtures.

2. Shape the agent's chat citations. No hook can intercept the model's
   natural-language output, so put a standing instruction in the agent's memory
   or project rules: "never cite code by file:line; resolve via `rr at` and cite
   the anchor." That is what governs what the agent prints to the screen.

## Limits to know today

- A hook can block file *writes* but not what the agent *prints*; the memory
  rule covers the chat side, and only as guidance.
- Heading anchors are line-scoped, so `rr at` on a body line resolves to the
  file anchor, not the enclosing section. Cite a prose section by its heading
  anchor. (Section-spanning heading anchors are a planned improvement.)
- `rr search` (find everywhere an anchor is cited) is planned, not yet
  implemented, so today rr generates and resolves citations but does not yet
  find existing ones to audit.

## Why it is worth it

The references in your docs, PRs, and agent output stay correct across change,
and the check can run in a hook (or later in CI via `rr enforce`), not just
interactively in one editor.
