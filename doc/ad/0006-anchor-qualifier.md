# AD-6: The anchor qualifier: an anchor may narrow an anchor

- Status: Accepted
- Date: 2026-08-22
- Tags: domain, references, public-api

## Context and problem statement

`[[rr:AD-1]]` gives a writer one way to disambiguate an identity that repeats
across files: the path qualifier, `path#identity`. The same record concedes
the cost, in that a path qualifier "trades move-stability for precision: the
qualified anchor dangles if the qualifying file moves". That trade is forced.
A writer who wants one section of one record has no move-stable way to say
so, because the section title repeats across every record and only the path
tells them apart.

The result is visible in this repository. Before this record, thirty-one
markers were path-qualified, thirty of them into doc/ad, and renaming any
record file would have dangled every marker into it at once, while the
record ID those files carry would not have moved. A tool built to retire
fragile references made its own most precise form the fragile one.

## Decision drivers

- Both halves of a reference should survive a file move when the tree gives
  them a stable name to survive by.
- One resolution rule, stated once, for every kind that can qualify; no kind
  is a special case.
- `[[rr:AD-1]]` does not amend. This record extends it and contradicts
  nothing in it.
- `at` prints the form a person pastes (`[[rr:AD-4]]`), so whatever form is
  legal must be the form `at` emits, or the move-stable form is only ever
  written by hand.

## Considered options

- **Leave the path as the only qualifier.** Every section reference into a
  record stays one rename from dangling. Rejected.
- **Intersect across a multi-file qualifier.** `Decision outcome#x` would
  resolve wherever any `Decision outcome` section holds an `x`. Friendlier
  on the surface, and the meaning then depends on how many files happen to
  match today. Rejected.
- **Restrict the qualifier to kinds that span a file.** A record usually
  does, a heading or a symbol does not, so the rule would need a kind
  taxonomy, which `[[rr:AD-1]]` refuses to fix. Rejected.
- **Let any anchor qualify, by span containment.** Every definition already
  has a span, so "inside the qualifier" is answerable for every kind from
  the index as it stands. Taken.

## Decision outcome

The qualifier of `[[rr:AD-1]]` may be an anchor as well as a path. The
written form is unchanged, `qualifier#identity`, split at the first `#`,
and the identity half is unchanged. Resolution, in order:

1. The whole token is tried as an identity, as before, so an identity that
   itself contains `#` resolves literally.
2. The qualifier is read as a path: the identity's definitions filter to
   that file. This is the rule of `[[rr:AD-1]]` and it comes first, so a
   file whose path is also spelled like some identity keeps winning.
3. Only when no definition lies in a file of that name is the qualifier read
   as an anchor. It must resolve to exactly one definition; the identity's
   definitions then filter to those whose span lies within that
   definition's span, in the same file. A qualifier that resolves to none or
   to many narrows to nothing, and the anchor dangles.

Containment is by span, so the rule is the same for a record, a heading, a
symbol, or any kind a profile adds: `AD-1#Decision outcome` is the section
inside the record, and `parse_reference#inner` would be a definition inside
that function, if the language declared one. A symbol that defines nothing
inside it simply qualifies nothing.

`at` prints the minimal unambiguous form, and this record fixes what minimal
means when the identity is not unique: the outermost enclosing anchor that is
itself unique and whose span holds exactly one definition of the identity.
Only when no enclosing anchor serves does `at` print the path. This
supersedes the single clause of `[[rr:AD-4]]` that read "path-qualified when
it does not"; every other word of that record stands.

`search` with a qualified argument matches exactly that written form, as
before. A marker qualified by path and one qualified by anchor are different
markers, even when they resolve to one definition.

### Consequences

- A marker that names a record and a section inside it survives the record
  file moving. The path-qualified form keeps working and keeps dangling on a
  move; it is the writer's choice, and `at` no longer chooses it while a
  stable qualifier exists.
- The gate gains no finding kind. A qualifier that fails to name one
  definition is a dangling marker (`[[rr:AD-3]]`), the same as any other
  anchor that resolves to nothing.
- A qualifier is one anchor, not a chain. `AD-1#Decision outcome#more`
  still reads as qualifier `AD-1` and identity `Decision outcome#more`.
- Two lookups instead of one for an anchor-qualified marker. The second is
  the same forward lookup as the first, against the same index.

Extension seams: a kind qualifier for cross-kind ambiguity, which
`[[rr:AD-1]]` reserves, would be a third reading of the same `#` position
and would slot after the path reading without disturbing this one.

## Dogfooding

- Run against the "Decision outcome" section of this record, `at` prints
  `[[rr:AD-6#Decision outcome]]`; `rr read` of that marker resolves to the
  section, and the marker survives this file being renamed.
- Every marker into a sibling record's section in this repository now
  carries the record ID rather than the path, minted by `at`.
