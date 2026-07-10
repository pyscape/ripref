# AD-3: The CLI: five verbs over one index

- Status: Accepted
- Date: 2026-07-01
- Tags: cli, public-api, references

## Context and problem statement

`[[rr:AD-1]]` fixes the nouns and `[[rr:AD-2]]` the written form. This record
fixes the CLI: the verbs rr exposes and what each one does. rr's job is one
thing: the index of `[[rr:AD-1]]` maps each anchor to the locations where it
is defined, so a marker written anywhere resolves to its definition. The
verbs follow from the job and add nothing else; a CLI that names more than
the index also owns
storage, retention, and credential surface the index does not need.

## Decision drivers

- The vocabulary names the job: map an anchor to its definitions, turn a
  location back into a marker, and judge the references a project writes.
- One verb performs one operation.
- The CLI owns no storage, retention, or credential surface; the index of
  `[[rr:AD-1]]` is the only artifact written.
- `rr --help` teaches the whole tool.

## Considered options

- **A verb per capability, backed by a committed content store.** Freezing
  and tracking referenced content gives every reference more state to carry,
  and gives rr storage, garbage-collection, and credential concerns an index
  does not need. A reference that must survive content change is a job for
  the version-control system that already owns history. Rejected.
- **Readers without a gate.** Resolution alone leaves rot visible only to
  whoever happens to run a read; honesty that cannot run in CI is a
  suggestion, not a property. Rejected.
- **Five verbs over one index.** Build the map, resolve a marker, produce a
  marker, list the markers written, judge them. Taken.

