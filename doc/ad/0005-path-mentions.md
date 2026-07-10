# AD-5: Path mentions: the index tracks where prose names files

- Status: Accepted
- Date: 2026-07-01
- Tags: index, references, public-api

## Context and problem statement

Documentation names files. A README says which module owns a feature, a
decision record names the files it constrains, a comment points a reader at a
neighbor. Under `[[rr:AD-1]]` none of those paths is an anchor, and under
`[[rr:AD-3]]` a marker wrapping only a path is a finding, so the paths stay
in prose. Prose paths rot silently: rename a directory and every document
that named it keeps naming it, and nothing notices. The index of
`[[rr:AD-1]]` knows where every anchor is defined; for the gate to keep
prose paths honest, it must also know where paths are written.

## Decision drivers

- A path written in prose is a reference (`[[rr:AD-1]]`); the index should
  record it and where it sits.
- No verb beyond the five of `[[rr:AD-3]]`, no artifact beyond the one
  index of `[[rr:AD-1]]`.
- Detection is lexical and deterministic: a documented grammar, not
  guesswork.
- The gate stays quiet: a finding is almost always real, because a noisy
  gate is a disabled gate.

## Considered options

- **Do nothing.** Prose paths keep rotting silently. Rejected.
- **Wrap paths in markers.** The wrapper adds no identity the path does not
  already carry, and the marker dangles on the first rename; `[[rr:AD-1]]`
  forbids the form and `[[rr:AD-3]]` reports it. Rejected.
- **Record mentions at index time, judge them at verify time.** Taken.

## Decision outcome

A **path mention** is a delimited token of two or more nonempty segments
separated by `/`, containing no colon. src/cli.rs in this sentence is a
mention; a lone filename or a bare separator is not. The exact token charset
is pinned in the module docs beside the marker regex of `[[rr:AD-2]]`.

Load-bearing rules:

- Mention scanning reads the regions marker scanning reads (`[[rr:AD-2]]`),
  over the scope of `[[rr:AD-3]]`; since an inline code span is read just
  when its content begins with the marker opener, which no mention does, a
  mention qualifies only in prose. The interior of a marker is excluded, so
  a path-qualified marker never also counts as a mention, and a moved file
  reports once, not twice.
- `index` records every mention with the location it sits at, beside the
  anchor map. The anchor map of `[[rr:AD-1]]` answers where an identity is
  defined; the mention table answers where a path is written.
- `index` records a mention whether or not the path exists. Existence is the
  gate's judgment, at the gate's time, against the live tree.
- `verify` judges a mention only when its first segment names a directory
  that exists in the tree or a configured scope root; a judged mention whose
  full path names nothing is the stale path finding of `[[rr:AD-3]]`. The
  guard is what keeps the gate quiet: and/or, I/O, and 24/7 are two-segment
  tokens, but "and", "I", and "24" name no directory, so prose compounds
  never reach judgment. The cost is recall on a renamed top-level directory,
  and a missed mention costs only a missed warning.
- A judged mention immediately followed by `:` and digits is the bare
  `path:line` finding of `[[rr:AD-3]]`.
- `search --mentions` (`[[rr:AD-3]]`) lists mentions exactly as `search`
  lists markers, by this record's grammar.
- Consumers, stated exactly: `search` and `verify` scan live text
  (`[[rr:AD-3]]`) and take nothing from the mention table. The table serves
  completion (with the
  anchor map: an editor reads the index, or any verb's JSON, to offer real
  anchors and paths while a reference is being written), and it is the
  occurrence input a rename workflow needs: when a file moves, the table
  names every document that wrote the old path.

