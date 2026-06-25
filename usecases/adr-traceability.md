# Decision-record traceability

An architecture decision record (ADR) explains why the code looks the way it
does, but the link between the two rots fast. The ADR says "see the parser", the
parser moves, and now the record points at nothing; the code that implements
`AD-42` carries no marker, so the next person rewriting it never learns there was
a decision to honor. This use case ties each record to the code that implements
it in both directions: the ADR cites the symbol anchors it constrains, and the
code (or its PR) cites the `AD-42` record anchor back. Both citations are anchors,
so both survive the next refactor.

## The pattern

Author the two-way link with the readers, keep it fresh with the writer:

```
rr index                       # build / refresh the index
rr at src/parser.rs:118        # a line -> the symbol anchor an ADR should cite
rr read AD-42                  # the record anchor -> where the decision lives
```

- ADR -> code: in the record, cite the symbol anchors it governs, never a
  `file:line`. Get each anchor from `rr at <file>:<line>` on a line in the
  implementation, then write that anchor into the ADR's "implemented by" list.
- code -> ADR: in the implementing code (or the PR description) cite the record
  anchor `AD-42`. A reader follows it with `rr read AD-42` to land on the
  decision; add `--locate` for the bare `file:start-end` to open in an editor.
- The record anchor and the symbol anchor are the durable ends of the link. A
  line number is only ever a transient resolution of one of them.

Worked example. You are editing the parser and want to record why it is
hand-rolled rather than generated:

```
$ rr at src/parser.rs:118
parser::Parser::parse_record
```

The ADR's "implemented by" line then reads `parser::Parser::parse_record`, not
`src/parser.rs:118`. In `parse_record` itself (and in the PR that adds it) you
cite `AD-42`, and anyone who lands there resolves the decision with:

```
$ rr read AD-42
docs/adr/0042-hand-rolled-parser.md:1-37
# AD-42: Hand-rolled record parser
...
```

## What makes the link real

Both ends are anchors, so both move with the code. Rename `parse_record` and the
ADR's citation still resolves; relocate the ADR file and the code's `AD-42`
reference still resolves, because a record anchor is keyed on its identifier, not
its path. Freshness is enforced on resolution: if the parser changed after the
last `rr index`, `rr read` and `rr at` exit 3 rather than hand you a stale
location, so a citation is either correct or loudly out of date, never quietly
wrong. Record anchors live in the same namespace as symbols, headings and
scenarios, so the same two commands author every kind of cross-reference.

## Limits to know today

- rr authors and follows the link one anchor at a time. It does not yet list all
  the places `AD-42` is cited, or check that every "implemented by" anchor in an
  ADR still resolves. `rr search AD-42` (find every citation of a record) is
  planned, not yet implemented.
- The audit half of traceability is also planned: `rr enforce` would flag a
  dangling `AD-42` reference, an ADR citation to a symbol that no longer exists,
  and a bare `file:line` used where an anchor belongs. Today those checks are
  manual.
- `rr read AD-42` resolves the record to its file anchor span; the record must be
  in a file rr is configured to scan as records (see Configuration in the
  README). Which patterns count as a record is configuration, not hardcoded.

## Why it is worth it

A decision and its implementation stay connected through every rename, move and
rewrite, in both directions, with no manual bookkeeping. The next person to touch
`parse_record` can find `AD-42` from the code, and the next person to read
`AD-42` can find the code from the record, and neither link silently breaks the
way a `file:line` would.
