# Marker violations corpus

Fixture for the `rr verify` integration tests: each line under Violations
carries exactly one finding of the six kinds, and the Clean section must
produce none. The test supplies the tree this file is judged against: a
src/lib.rs file (so the src directory exists), two documents that each title
a section "Dup", and this file copied in as corpus.md.

## Violations

A dangling marker: [[rr:no-such-identity-xyzzy]] resolves to nothing.

An ambiguous marker: [[rr:Dup]] resolves to two definitions.

A path-only marker: [[rr:src/lib.rs]] wraps a bare path.

A malformed marker: the opener [[rr:unterminated never closes on this line.

Another malformed marker: [[rr:bad\zescape]] uses an escape that does not exist.

A bare path line reference: src/lib.rs:7 rots on the next edit.

A stale path mention: src/no-such-file.rs names nothing in the tree.

## Clean

A resolving marker: [[rr:corpus-anchor]] points at the heading below. A
qualified marker: [[rr:a.md#Dup]] resolves to exactly one definition. A prose
compound like and/or never reaches judgment, and neither does I/O or 24/7. A
path that exists, src/lib.rs, is honest prose and produces nothing. Inside a
fence, nothing is scanned, so an example cannot go stale by construction:

```text
[[rr:fenced-dangling]] and src/fenced-missing.rs and src/lib.rs:99
```

## corpus-anchor

This heading defines the identity the resolving marker above points at.
