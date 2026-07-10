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

