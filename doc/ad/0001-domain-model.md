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

## Decision outcome

Five nouns.

- **anchor**: a stable identity, written bare on the CLI. An identity is the
  name a definition bears: the record ID `AD-1`, the symbol
  `parse_reference`, the heading text `Decision outcome`. An anchor may carry
  a path qualifier, `path#identity`, split at the first `#`, which narrows
  resolution to identities defined in that file. A bare path is never an
  anchor.
- **marker**: the written use of an anchor, embedded in a document so that a
  scan can find it. AD-2 fixes the grammar. A marker resolves or it
  dangles; it carries no other state.
- **location**: a place in the tree: `file:start-end`, with 1-based inclusive
  line numbers, and `file:line` abbreviating a single-line span. The span is
  the numeric suffix after the last colon, so a Windows drive colon stays
  inside the path.
- **reference**: anything a document writes that points into the tree: a
  marker, a path written as prose, or the fragile bare `path:line` form.
- **index**: rr's one artifact, derived and rebuildable: the map from each
  anchor to its definition locations, beside which it records where prose
  writes paths (AD-5). rr writes nothing else and stores no file
  content.

A **kind** is a named class of anchors declared in configuration. A kind
declaration names three things: which files it reads (a host matcher), what
defines an anchor there (an identity rule), and how far a definition extends
(a span rule). The default profile ships in rr.toml and declares six kinds;
a project's own `.rr.toml` merges over it to narrow, widen, or add kinds.

Invariants every kind obeys, fixed here and not configurable:

- An identity is never a bare path. A kind whose identity rule yields the
  file's own path is not a kind; it is the fragile reference wearing a
  costume.
- Every definition has a span, so a location always answers "how far".
- Identities compare byte-for-byte after Unicode NFC normalization: no case
  folding, no slugging, no zero-stripping, no other rewriting. What you read
  in the source is the identity, exactly.
- `/` is the path separator in every written form (qualifiers, locations,
  prose). A `\` separator is accepted as CLI input and is normalized to `/`
  in everything rr prints; the escape bytes of AD-2 are not
  separators.
- An anchor resolves to zero, one, or many definitions. Many is ambiguity:
  resolution returns every definition, and the path qualifier is the
  writer's fix. The same title in two files coexists as one many-definition
  anchor, not an error.

The default profile's six kinds:

- **record**: a titled region whose title opens with an ID of uppercase
  ASCII letters, one hyphen, and digits, immediately followed by the title's
  first colon; the ID is the identity. This record's title defines `AD-1`.
- **heading**: any other titled region in a document; the identity is the
  full title text.
- **scenario**: a titled region in a gherkin feature; the identity is the
  title text.
- **symbol**: a code definition (a function, a type, a constant); the
  identity is the name the language query captures, qualified as the
  language itself qualifies names.
- **key**: a manifest entry; the identity is the entry's name as the format
  writes it, for TOML `[table] key`.
- **operation**: an API operation; for OpenAPI, the operationId value.

Spans, by the same declarations: a titled region (record, heading, scenario)
spans from its title line to the next title of the same or higher rank, or
the end of its region; a symbol spans its whole definition; a key or
operation spans its entry. The innermost span covering a line is that line's
tightest anchor.

### Consequences

- Any format joins by configuration; this record never amends for one.
- A file move dangles no unqualified reference, because no kind may name a
  path. A path qualifier, written by hand or emitted for an ambiguous
  identity (AD-4), trades move-stability for precision: the
  qualified anchor dangles if the qualifying file moves, and the gate
  reports it (AD-3).
- Identical titles are legal and ambiguous, not errors; the qualifier
  resolves them. A cross-kind collision (a heading and a symbol sharing an
  identity) is the same ambiguity.
- Comparison is unforgiving by design: a marker that differs from its
  definition by case dangles, and the gate reports it (AD-3).
  Forgiveness would make resolution nondeterministic across profiles.

Extension seams: a future record may add kinds through configuration, and
may introduce a kind qualifier for cross-kind ambiguity, without
contradicting this record.

## Dogfooding

- This record's title defines the record anchor `AD-1`, and `[[rr:AD-1]]`
  resolves to it.
- The qualified anchor `src/cli.rs#parse_reference` narrows the symbol
  `parse_reference` to one file; unqualified, `[[rr:parse_reference]]`
  resolves the same symbol while its name is unique.
- Each sibling record titles a region "Decision outcome"; the unqualified
  identity is ambiguous across them, exactly as this record requires, and
  the qualifier picks one.
