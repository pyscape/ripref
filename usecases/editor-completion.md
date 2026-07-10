# Editor integration: complete a reference as you type

Markers are written exactly where editor completion is not: Markdown prose,
TOML manifests, gherkin features, code comments. A language server completes
the symbols it parses, in code, per language; the moment you reference one of
those symbols from a design doc or a `.feature` spec, you type an anchor from
memory with no completion at all, and a mistyped anchor is a reference that
dangles. `rr verify` reports the dangle after the fact; completion is how the
reference is right the moment it is written.

## The index is the completion source

Deliberately so: rr has no serve verb. Per `[[rr:AD-5]]`, an editor reads the
index, the anchor map and the mention table together, to offer real anchors
and paths while a reference is being written: identities to complete, and
path candidates for a qualifier. The index file (default .ref-cache/index) is
memory-mapped and flat, its byte layout is documented in the refidx module
(src/refidx.rs), and its forward section is sorted by anchor name, so prefix
enumeration is a bisect to the first anchor sharing the prefix and a walk
while the prefix holds: microseconds, well inside a completion popup's
budget. A plugin reads the index directly for candidate enumeration, and
shells out to the verbs' `--format json` envelopes (`[[rr:AD-4]]`) for
resolution.

## Preview

For the highlighted candidate, `rr read <anchor>` resolves to the definition
locations, and the detail pane renders the span from the live file; rr stores
no file content, so the location is the whole answer and the tree is the
body. Pass `--no-freshness` mid-keystroke: a reading verb otherwise refuses a
stale index with exit 3, correct for a one-shot resolve and wrong under a
cursor, where a slightly-old answer beats an error.

```
$ rr read my_module::handler --no-freshness
src/handlers.py:8-26
```

## Insertion

What lands in the buffer is the full marker, never the bare anchor. `rr at`
on the chosen definition's location prints it, in the anchor's minimal
unambiguous form and with the escaping of `[[rr:AD-2]]` already applied, so
the plugin never hand-wraps an anchor and never re-derives qualification;
under `--format json` the envelope carries the marker and the bare anchor
side by side.

```
$ rr at src/handlers.py:15 --no-freshness
[[rr:my_module::handler]]
```

## Refresh

Candidates are as fresh as the index. Re-run `rr index` on save: it is the
single writer, and a rebuild is one command. Between saves, `--no-freshness`
keeps every query answering.

## Vim and Neovim

An `omnifunc` or `completefunc`, or an `nvim-cmp` / `blink.cmp` source, that
enumerates anchors for the token under the cursor. Trigger it on the marker
opener rather than on every word; the opener is the five fixed bytes
`[[rr:AD-2]]` specifies, so it never fires mid-sentence. Insert the marker
`rr at` prints. Pass `--no-color` to keep escape codes out
of the buffer, and `--index <path>` (or `REF_INDEX`) when the editor's
working directory is not the project root.

## VS Code

A `CompletionItemProvider` registered for `markdown`, `plaintext`, `gherkin`,
and `toml`, the languages where the built-in providers stay silent. Map each
candidate to a `CompletionItem`: the label is the bare anchor, the detail is
the location `rr read` resolves, and the inserted text is the marker `rr at`
prints. The same `--index`, `--no-color`, and `--no-freshness` flags apply.

## Why it is worth it

Jump-to-definition made reading code a keystroke; this makes referencing it
one too, in the files no language server reaches: the design doc, the spec,
the comment, the manifest. You type the start of a name, the editor offers
the anchors that exist, and the marker in the buffer resolves the moment it
is written and keeps resolving as the code around it moves, while a
hand-typed `file:line` starts rotting at the next edit. The cost is a bisect
over an index the project already builds.
