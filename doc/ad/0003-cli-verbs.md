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

## Decision outcome

The verbs:

- **`index`** builds or refreshes the index: every anchor's definitions and
  every path mention (AD-5) in scoped text. It is the only verb
  that writes, and it writes only the one artifact of `[[rr:AD-1]]`.
  **Scoped text** is
  the working tree, honoring `.gitignore` when the project provides one and
  the scan configuration of the profile, or exactly the paths a caller
  passes; `index`, `search`, and `verify` share this definition.
- **`read`** resolves a marker, or a bare anchor, to the anchor's definition
  locations.
- **`at`** takes a `file:line` location and answers with the innermost
  anchor whose definition covers that line (the span rule of `[[rr:AD-1]]`);
  under `--all` it reports the whole nest. AD-4 fixes the printed
  form. `at` is the one verb that produces a marker; no flag duplicates it.
- **`search`** lists the markers scoped text writes, each with the location
  it sits at. An optional anchor argument filters the listing: an
  unqualified argument matches every marker whose identity equals it,
  path-qualified or not; a qualified argument matches exactly. Under
  `--mentions` it lists path mentions instead of markers (AD-5).
  `search` is purely lexical: it reads no index, so it also runs on text
  outside any project.
- **`verify`** is the only gate. It judges the references scoped text
  writes and reports findings of exactly six kinds:
  1. **malformed marker**: an unpaired opener outside a fence, an undefined
     escape, an unterminated token (`[[rr:AD-2]]`).
  2. **dangling marker**: resolves to no definition.
  3. **ambiguous marker**: resolves to more than one definition; the
     qualifier of `[[rr:AD-1]]` is the writer's fix.
  4. **path-only marker**: wraps what is lexically a path and no identity. A
     marker that carries nothing beyond a path adds nothing over the path
     written as prose, which AD-5 already keeps honest.
  5. **bare `path:line` reference**: a line number rots faster than anything
     it points at; the marker is the fix.
  6. **stale path mention**: a prose path that names nothing in the tree.
     Detection and judgment grammar for this and for `path:line`:
     AD-5.

Load-bearing rules:

- `at` and `read` invert each other: `at` turns a location into a marker,
  and `read` turns a marker back into locations.
- A marker lives in the document that writes it; it is removed by editing
  that document. rr has no inverse verb.
- `search` locates, `verify` judges.
- Many definitions make a read ambiguous (the adverse answer,
  AD-4) and give `verify` a finding; they do not make the marker
  invalid (`[[rr:AD-1]]`).
- `index` is deliberately both the artifact and the verb that builds it; the
  pair shares one name because the verb does nothing else.

### Consequences

- `rr --help` reads as five verbs over one index, and a reader learns the
  whole tool from it.
- rr stores no content (`[[rr:AD-1]]`), so it raises no credential surface
  and owns no retention or garbage-collection policy. rr does not preserve a
  referenced definition against deletion and does not report when its
  content changes: a marker resolves or it dangles (`[[rr:AD-1]]`). That is
  the tool's identity, not a gap; content history belongs to version
  control.
- rr runs without git (`[[rr:AD-1]]`). It indexes the working tree; a
  clean-tree freshness fast-path may consult git and falls back to an mtime
  walk.
- A CI job gates on `verify` alone; the other verbs never judge.

Extension seams: a future record may add a verb only by extending the job
over the same nouns; the rename workflow over the mention table of
AD-5 is the anticipated case.

## Dogfooding

- This record's title defines `AD-3`; `[[rr:AD-3]]` resolves to it.
- The CLI lives across three files, each a path mention AD-5
  tracks: flags in src/cli.rs, per-verb handlers in src/commands.rs,
  dispatch in src/lib.rs.
- `[[rr:src/cli.rs#parse_reference]]` narrows the reference parser to its
  file, the qualified form of `[[rr:AD-1]]`.
