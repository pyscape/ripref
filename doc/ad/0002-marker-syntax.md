# AD-2: Marker syntax: the delimited written form of an anchor

- Status: Accepted
- Date: 2026-07-01
- Tags: format, references, public-api

## Context and problem statement

An anchor plays two roles with different requirements. Written bare as a CLI
argument, it only needs to be resolvable. Written into a document, it must
additionally be findable by a scanner and distinguishable from ordinary
prose. A bare `Decision outcome`, `handler`, or `AD-42` sitting in a sentence
is byte-identical to ordinary writing, so no scan can reliably find it: a
free-text title carries zero lexical signal, and a grammar-bearing token
(`::`, `#`, `AD-`) over-matches every incidental mention and has no boundary
rule. Everything that must find a written reference (the `search` and
`verify` verbs of AD-3, an editor hook, a completion trigger)
depends on this record.

