# AD-4: The output contract: text by default, rr-json for tools

- Status: Accepted
- Date: 2026-07-01
- Tags: cli, public-api, output

## Context and problem statement

Two readers consume rr's output: a person at a terminal, and a tool (an
editor, a CI step, a script). A person wants a marker to paste and a location
to read; a tool wants one stable, versioned record it can depend on across
versions. This record fixes what each verb of `[[rr:AD-3]]` prints as text,
what it prints as JSON, and how every result reaches the exit code.

## Decision drivers

- A person reads the default output with no flag.
- A tool parses one stable, versioned format across every verb.
- The result reaches the exit code, so a shell branches without parsing.
- `at` prints the form a person pastes.
- `--format` selects the shape for every verb alike; a verb may take flags
  that select what is reported, never how it is shaped.

## Considered options

- **`at` prints the bare anchor, and a flag adds the brackets.** The flag
  duplicates the one marker-producing verb (`[[rr:AD-3]]`), and the default
  hands a person the one form they never paste. Rejected.
- **`at` prints the marker; JSON carries the bare anchor.** A person pastes,
  a shell quotes, a tool reads the structured field. Taken.
- **Per-verb output flags.** Every verb grows its own shape switches and a
  tool learns five contracts. Rejected: one flag, every verb.

## Decision outcome

`--format text|json` is global. `text` is the default; any other value is a
usage error.

Text, the default:

- `index` prints `indexed N anchors and P path mentions across M files`.
- `read` prints one definition location per line, `file:start-end`; an
  ambiguous anchor prints each.
- `at` prints its answer (`[[rr:AD-3]]` fixes which anchor answers) as a
  marker in the anchor's minimal unambiguous form: unqualified while the
  identity resolves uniquely, path-qualified when it does not. Anchors tied
  on the same innermost span print one marker per line; `--all` prints the
  whole nest of `[[rr:AD-3]]`, outermost first.
- `search` prints one line per marker (the file, the line, the marker) and a
  closing summary; under `--mentions`, the same shape for mentions.
- `verify` prints one line per finding (the file, the line, the rule) and a
  closing summary.

`at` prints the bracketed marker because that is what a person pastes. The
brackets are shell glob metacharacters, so a shell consumer quotes the marker
(single quotes usually, double when the anchor itself carries a single
quote); `read` accepts the bracketed form (`[[rr:AD-3]]`), so
`rr read "$(rr at file:line)"` resolves; a tool that wants a glob-free token
reads the `anchor` field under JSON.

JSON, under `--format json`: every verb prints one envelope,

```json
{"format":"rr-json","version":1,"command":"<verb>","data":{}}
```

- `version` is the contract version: a field may be added without a bump; a
  field that changes meaning or departs bumps it. A tool checks `format` and
  `version`, then reads `data`.
- `data`, per verb. `at` carries `anchors`, always a list (one entry
  normally, more on a tie or under `--all`), each entry an `anchor` (bare),
  a `marker` (composed), and the `location` of that definition. `read`
  carries the `anchor` and its `locations`. `index` carries the `anchors`,
  `mentions`, and `files` counts. `search` carries `matches`, each a `file`,
  `line`, `anchor`, and `marker`. `verify` carries `findings`, each a
  `file`, `line`, and `rule`. Selection flags change list membership, never
  shape. The field-level schema lives in the module docs beside the writers.

Exit codes: every verb asks a question, and the code reports how it was
answered, identically under text and JSON.

- `0`: the question got its answer.
- `1`: the adverse answer. A `read` or an `at` finds nothing or resolves
  ambiguously; a `search` finds no matching marker (the convention of rg,
  which scripts already assume of a search tool); a `verify` has findings.
- `2`: usage error: a malformed argument, an unknown flag, a value
  `--format` does not name.
- `3`: the index is stale; a reading verb refuses to answer from stale data.
  Rebuild with `rr index`, fall back to ripgrep (always fresh), or pass
  `--no-freshness` to accept the index as-is, which a completion popup
  mid-keystroke prefers to an error. `search` reads no index
  (`[[rr:AD-3]]`) and never returns 3.

### Consequences

- A person runs a verb and pastes what `at` prints; a tool parses one
  envelope and survives added fields; a CI step gates on `verify`'s exit
  code without parsing any output.
- The exit model is uniform, so a script learns one branch: nothing-found,
  ambiguity, no-match, and findings are all the same adverse shape.
- rr names two output shapes and one flag that selects them.

## Dogfooding

- The text and JSON writers for `at` are `[[rr:at_text]]` and
  `[[rr:at_json]]`, and `--format` parses into `[[rr:OutputFormat]]`; each
  marker names an identity, not a path (`[[rr:AD-1]]`), so each survives its
  file moving.
- Run against this record's opening line, `at` prints `[[rr:AD-4]]`, and its
  JSON data carries the bare anchor `AD-4`; the path under doc/ad is `at`'s
  input, never the anchor (`[[rr:AD-1]]`).
