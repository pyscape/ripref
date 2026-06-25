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
  wrap it as `[[rr:anchor]]` and write that marker into your doc, comment, or
  message, never a bare anchor or `file:line`.
- To *follow* a citation: `rr read <anchor>` resolves it; add `--locate` for the
  current `file:start-end` to hand to an editor.
- The anchor is the durable reference; a line number is only ever a transient
  resolution. That is the whole point.

Worked example (this repo): cite a benchmark finding by anchor instead of line.

```
$ rr at BENCHMARKS.md:112
Index build: the writer
```

You then cite `[[rr:Index build: the writer]]` in your doc, not the bare
anchor or `BENCHMARKS.md:112`.

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
   # Read the tool input, scan the new content outside fenced code blocks.
   #
   # Negative case (always block): a raw filename.ext:line pattern that is not
   # part of an `rr at` / `rr read` / `rr enforce` command line. Exit non-zero:
   #   "cite as [[rr:anchor]] (run: rr at <file>:<line> to get the anchor),
   #    never bare or as a file:line".
   #
   # Positive case (AD-1): a [[rr:...]] marker is the valid citation form;
   # pass it through without complaint. During the grace period, also flag a
   # bare token that exactly matches a live index anchor and tell the agent to
   # wrap it: "cite as [[rr:<anchor>]], not the bare form".
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
   or project rules: "cite as `[[rr:anchor]]`, never bare or as `file:line`;
   resolve via `rr at` to get the anchor, then wrap it." That is what governs
   what the agent prints to the screen.

## Limits to know today

- A hook can block file *writes* but not what the agent *prints*; the memory
  rule covers the chat side, and only as guidance.
- Heading anchors are line-scoped, so `rr at` on a body line resolves to the
  file anchor, not the enclosing section. Cite a prose section by its heading
  anchor. (Section-spanning heading anchors are a planned improvement.)
- `rr search` and `rr enforce` (find and audit every citation) are planned, not
  yet implemented. The `[[rr:...]]` marker is precisely what makes them
  implementable: one deterministic scan locates every citation without false
  positives. Today rr generates and resolves citations but does not yet find
  existing ones to audit.

## Why it is worth it

The references in your docs, PRs, and agent output stay correct across change,
and the check can run in a hook (or later in CI via `rr enforce`), not just
interactively in one editor.
