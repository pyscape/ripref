# Decision-record traceability

An architecture decision record (ADR) explains why the code looks the way it
does, but the link between the two rots fast. The ADR says "see the parser", the
parser moves, and now the record points at nothing; and a bare `AD-42` written
into the code carries no marker, so the next person rewriting it never learns
there was a decision to honor, and no scan can list where the decision is relied
on. This use case ties each record to the code that implements it in both
directions: the ADR cites the symbol anchors it constrains, and the code (or its
PR) cites the `AD-42` record back. Each citation is a findable marker,
`[[rr:...]]`, around a stable anchor, so it survives the next refactor and a scan
can always locate it.

## The pattern

Author the two-way link with the readers, keep it fresh with the writer:

```
rr index                  # build / refresh the index
rr at src/parser.rs:118   # a line -> the symbol anchor an ADR cites
rr read AD-42             # the record id -> where the decision lives
```

- ADR -> code: in the record, cite the symbol anchors it governs as markers,
  never a `file:line`. Get each anchor from `rr at <file>:<line>` on a line in
  the implementation, then write `[[rr:<anchor>]]` into the ADR's "implemented
  by" list.
- code -> ADR: in the implementing code (or the PR description) cite the decision
  as `[[rr:AD-42]]`. The marker is what a later scan finds and what tells the
  next editor a reference exists; a bare `AD-42` is indistinguishable from prose.
- The record anchor and the symbol anchor are the durable ends of the link, and
  the `[[rr:...]]` wrapper is the marker that makes each end findable. A line
  number is only ever a transient resolution of one of them.

Worked example. You are editing the parser and want to record why it is
hand-rolled rather than generated:

```
$ rr at src/parser.rs:118
parser::Parser::parse_record
```

The ADR's "implemented by" line then carries
`[[rr:parser::Parser::parse_record]]`, not `src/parser.rs:118`. In the function
itself (and in the PR that adds it) you cite `[[rr:AD-42]]`, and anyone who lands
there resolves the decision with:

```
$ rr read AD-42
docs/adr/0042-hand-rolled-parser.md:1-37
# AD-42: Hand-rolled record parser
...
```

## What makes the link real

Both ends are anchors, so both move with the code. Rename `parse_record` and
the ADR's `[[rr:parser::Parser::parse_record]]` marker still resolves; relocate
the ADR file and the code's `[[rr:AD-42]]` reference still resolves, because a
record anchor is keyed on its identifier, not its path. The marker adds the
property the bare form lacked: the reference is locatable, so a scan finds every
citation without mistaking an incidental mention of the id for one. Freshness is
enforced on resolution: if the parser changed after the last `rr index`,
`rr read` and `rr at` exit 3 rather than hand you a stale location, so a citation
is either correct or loudly out of date, never quietly wrong. Record anchors live
in the same namespace as symbols, headings and scenarios, so the same two
commands author every kind of cross-reference.

## Limits to know today

- rr writes and follows the marker one anchor at a time. The readers do not strip
  a pasted `[[rr:...]]` wrapper yet, so pass the bare anchor on the CLI
  (`rr read AD-42`, not `rr read [[rr:AD-42]]`) for now.
- rr does not yet list all the places `AD-42` is cited, or check that every
  "implemented by" marker in an ADR still resolves. `rr search AD-42` (find every
  citation of a record) is planned, not yet implemented.
- The audit half of traceability is also planned: `rr enforce` would flag a
  dangling `[[rr:AD-42]]` marker, an ADR marker to a symbol that no longer
  exists, and a bare `file:line` used where a marker belongs. Today those checks
  are manual.
- `rr read AD-42` resolves the record to its file anchor span; the record must be
  in a file rr is configured to scan as records (see Configuration in the
  README). Which patterns count as a record is configuration, not hardcoded.

## Why it is worth it

A decision and its implementation stay connected through every rename, move and
rewrite, in both directions, with no manual bookkeeping, and each connection is
now a marker a scan can find rather than a bare id hiding in prose. The next
person to touch `parse_record` can find `[[rr:AD-42]]` from the code, and the
next person to read `AD-42` can find the code from the record, and neither link
silently breaks the way a `file:line` would.
