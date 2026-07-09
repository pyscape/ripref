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

