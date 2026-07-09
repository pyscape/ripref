# AD-1: The domain model: anchors, markers, locations

- Status: Accepted
- Date: 2026-07-01
- Tags: domain, references, public-api

## Context and problem statement

ripref (binary: `rr`) keeps written references honest. A reference by line
number, `parser.go:42`, is wrong the moment a line is inserted above it, and
nothing reports the break. The durable way to point at something is to name
what it is (the function, the heading, the decision, the entry), not where it
happens to sit. Making that the cheap default requires a model precise enough
that every later contract (the written form, the verbs, the output) derives
from it without restatement.

## Decision drivers

- One word names one concept, and one concept keeps one word.
- Closed over mechanisms, open over taxonomies: no file format, however
  exotic, may force an amendment to this record.
- Nothing depends on stored file content or on a version-control system.
- Deterministic: two readers of the same tree derive the same anchors.

## Considered options

- **A fixed taxonomy of anchor kinds.** Enumerate the kinds normatively in
  this record. Every format the taxonomy did not anticipate then requires an
  amendment, and this record does not amend. Rejected.
- **Paths as identities.** Let a file's path stand as its own anchor. A path
  is where a thing lives, not what it is: the reference dangles on the first
  rename, which is the disease rr treats. Rejected absolutely.
- **An open kind mechanism with fixed invariants.** This record fixes what a
  kind is and the rules every kind obeys; configuration declares the kinds
  themselves. Taken.

