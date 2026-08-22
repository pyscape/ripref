# Anchored references with Claude Code

Agents and people reference code the same fragile way: `file:line` in a
commit message, a PR comment, a doc, a chat reply. The coordinate is wrong
the moment a line is inserted above it, and nothing reports the break. rr
replaces the coordinate with a marker wrapping a stable anchor: the name the
definition bears (a symbol, a heading, a record ID), which stays correct
across edits, moves, and refactors. This use case wires rr into Claude Code
so the agent writes markers, not line numbers.

## The loop

```
rr index                    # build or refresh the index
rr at path/to/file.rs:42    # location -> marker, printed ready to paste
rr read '[[rr:anchor]]'     # marker (or bare anchor) -> location
```

`rr at` takes the `file:line` you are looking at and prints the marker of
the innermost anchor whose definition covers that line. Paste it exactly as
printed, and quote it in a shell: the brackets are glob metacharacters.
`rr read` inverts it, resolving the marker (or the bare anchor) back to the
definition's current `file:start-end`. Nothing ever follows a marker's
closing `]]`, and rr stores no file content: a marker resolves or it
dangles, and the gate below reports the dangle.

A heading anchor spans its whole section, from the title line to the next
title of the same or higher rank, so `rr at` answers from any body line
inside the section. BENCHMARKS.md titles a section "Index build: the
writer"; ask about a line in its table:

```
$ rr at BENCHMARKS.md:118
[[rr:Index build: the writer]]
```

That marker goes into the commit message or doc as printed, and it still
resolves after the table grows and every line number under it shifts.

## Two guardrails in Claude Code

Hooks act on tool calls, not on the model's prose, so the guardrails come
in two layers.

**At write time.** A `PreToolUse` hook on `Edit` and `Write` judges the
markers in the content before it lands. The content is not on disk yet, so
the hook cannot run `rr verify` on it; it does two things instead. First,
find the markers with the canonical extraction regex of `[[rr:AD-2]]`,
which has no false positives in prose because of the `rr:` sentinel:

```python
# ~/.claude/hooks/check-markers.py   (PreToolUse: Edit, Write)
import re, subprocess, sys

MARKER = re.compile(r'\[\[rr:(?:\\[\\\[\]]|[^\\\]\[\t\n\r])*?\]\]')
```

Then resolve each one with `rr read`, and read the exit code. `rr read`
answers 0 for a marker that resolves to exactly one definition, 1 for the
adverse answer (dangling, or ambiguous across several definitions), and 3
when the index is stale, which is not the writer's fault:

```python
for marker in MARKER.findall(new_content):
    code = subprocess.run(['rr', 'read', marker],
                          capture_output=True).returncode
    if code == 3:
        sys.exit('index is stale: run `rr index`, then retry')
    if code == 1:
        sys.exit(f'{marker} does not resolve to one definition: run '
                 f'`rr at <file>:<line>` and paste the marker it prints')
```

Pass the marker to `rr read` exactly as written, and quote it in a shell:
the brackets are glob metacharacters. Wire the hook up with:

```json
{ "hooks": { "PreToolUse": [ { "matcher": "Edit|Write",
  "hooks": [ { "type": "command",
    "command": "python ~/.claude/hooks/check-markers.py" } ] } ] } }
```

After the write lands, the file is on disk and the whole gate applies to
it: `rr verify <path>` judges exactly the paths named, without walking the
tree. CI runs bare `rr verify` over the whole scope.

**In chat.** No hook intercepts what the model prints, so a standing memory
rule shapes the prose side: reference with markers, never a bare anchor and
never `file:line`; get the marker from `rr at` and paste it as printed.

## The gate

`rr verify` runs in a pre-commit hook or CI and exits 1 on findings, so a
bad reference fails the build instead of rotting. It judges every reference
scoped text writes and reports findings of six kinds: malformed, dangling,
and ambiguous markers, a marker wrapping a bare path, a bare `path:line`
reference, and a stale path mention (`[[rr:AD-3]]`). Exit codes are uniform
across the five verbs: 0 is the answer, 1 the adverse answer, 2 a usage
error, and 3 a stale index, which `rr index` rebuilds.

An existing codebase starts with a backlog of bare `file:line` references
and stale prose paths, which would leave CI red until it is all cleaned up.
`[verify] rules` is the way in: name the marker kinds on day one so the
gate is green, and add the mention kinds as the backlog clears.

```toml
# .rr.toml, day one
[verify]
rules = ["malformed-marker", "dangling-marker",
         "ambiguous-marker", "path-only-marker"]
```

```
$ rr verify        # CI, over the whole scope
0 findings
```

## Why it is worth it

A reference that names what it means outlives every edit that would have
broken a line number, and the loop that produces it costs one command. The
hook makes the marker the cheap default at write time, the memory rule
carries the habit into chat, and `rr verify` turns whatever slips through
into a failing check instead of silent rot: the references in your docs,
commit messages, and agent output stay honest without anyone rereading
them.
