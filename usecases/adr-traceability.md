# Decision-record traceability

A decision record explains why the code looks the way it does, and the link
between the two rots in both directions: the record names an implementation
that moves out from under it, and the code drifts from a record nobody
rereads. A bare `AD-42` dropped into a comment does not help, because it is
byte-identical to ordinary prose: no scan can list where the decision is
relied on, so nothing reports the break. rr makes each end of the link a
resolvable marker and gates the pair in CI.

## The record anchor

The record kind of `[[rr:AD-1]]` gives every decision record a stable
identity for free: a title opening with an ID of uppercase letters, one
hyphen, and digits, immediately followed by the title's first colon, defines
the record anchor. A title `AD-42: Hand-rolled record parser` defines
`AD-42`. The record is keyed by that ID, not by its file's path, so the
anchor survives the file moving or the directory renumbering; the records
under doc/ad each define their own anchor exactly this way.

## The two-way pattern

- Record to code: the record's "implemented by" list carries symbol markers.
  Get each one from `rr at` on a line inside the implementation; `at` prints
  the bracketed marker of the innermost anchor covering that line, ready to
  paste.
- Code to record: the implementing code, or the PR description that lands
  it, carries the record marker in a comment. It comes from the same verb:
  `rr at` on the record's title line prints the record's marker, ready to
  paste.

Worked example: the parser is hand-rolled, and a record says why.

```
$ rr at src/parser.rs:118
[[rr:Parser::parse_record]]
```

That marker goes into the record's "implemented by" list. Anyone who lands
on the comment in the parser resolves the decision:

```
$ rr read AD-42
doc/ad/0042-hand-rolled-parser.md:1-37
```

`at` and `read` invert each other: a location in, a marker out; a marker in,
the definition's location out.

## Impact analysis

Before changing or superseding a decision, list everything that leans on it.
`rr search AD-42` lists every marker of the record across docs, specs,
comments, and commit messages, each with the location it sits at:

```
$ rr search AD-42
src/parser.rs:114: [[rr:AD-42]]
doc/spec/parsing.md:31: [[rr:AD-42]]
tests/features/parse.feature:3: [[rr:AD-42]]
3 markers
```

## The gate

`rr verify` runs in CI and exits 1 on findings. For this pattern it reports:

- A dangling record marker: the record was deleted, and every marker of
  `AD-42` written into code resolves to nothing.
- A dangling symbol marker: the symbol was renamed, and the record's
  "implemented by" marker names an identity that no longer exists. A marker
  is findable, not rename-stable; the gate turns the dangle into a build
  failure instead of a quiet lie.
- An ambiguous marker: two records claim one ID, and resolution returns both
  definitions. The writer's fix is the path qualifier, `path#identity`,
  which narrows the marker to the record defined in one file.

## Both ends survive change

Both ends of the link are identities. The symbol marker names the function,
not the file, so the parser file moving changes nothing; the record marker
names the ID, not the path, so the record file moving changes nothing
either. rr stores no file content: a read resolves against the index of the
live tree, and freshness is checked at resolution, so a stale index exits 3
rather than answering wrong; `rr index` rebuilds it. The link is either
correct, loudly stale, or a gate finding, never quietly broken.

## Why it is worth it

A decision and its implementation stay attached through every rename, move,
and rewrite, and the attachment is machine-checked on every commit. The next
person to edit the parser finds the record from the comment; the next person
to reread the record finds the code from its list; and when either end
changes out from under the other, `rr verify` says so before the merge, in
an exit code a CI job reads without parsing any output.
