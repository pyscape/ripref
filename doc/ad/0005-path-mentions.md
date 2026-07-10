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

