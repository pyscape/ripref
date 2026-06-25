# AD-1: Anchors are addresses; citations are a distinct, delimited form

- Status: Proposed
- Date: 2026-06-24
- Tags: format, references, public-api

## Context and problem statement

ripref lets you cite code and prose by stable anchors. Today an anchor plays two
roles that have different requirements, and conflating them blocks half the
planned feature set.

- An anchor used as an **address** (the argument to `rr read`, `rr cite`,
  `rr track`) only needs to be resolvable. This works.
- The same anchor used as a **citation** (a token written into a document) must
  additionally be locatable by a scanner and distinguishable from ordinary text.
  This does not work: the citation is a bare, undelimited string.

`rr at` prints a bare anchor and the docs instruct pasting it verbatim into
prose. A bare `Index build: the writer`, `handler`, or `AD-42` sitting in a
sentence is byte-identical to ordinary writing, so no scan can reliably find it.
Two flavors of the gap:

1. Free-text heading anchors (for example `Index build: the writer`) carry zero
   lexical signal.
2. Grammar-bearing anchors (`::`, `#`, `AD-`) carry partial signal but over-match
   every incidental mention and have no boundary rule.

Every planned feature that must *find* a citation, as opposed to generating or
resolving one, is therefore blocked: `rr search`, `rr enforce`, the editor and
agent PreToolUse hook positive case, completion-trigger detection, and
resolution caching.

## Decision drivers

- Findable: one deterministic scan locates every citation, with a clear boundary
  rule, even for anchors containing spaces, `:`, `#`, `::`, `[`, `]`, quotes,
  `@`, `~`.
- Distinguishable from an incidental mention of the same word.
- Readable and reason-able from raw, unrendered text.
- Round-trips: `rr at` can emit it, a human or agent pastes it, the readers parse
  it, and the `@commit` / `~commit` pins still compose.
- Survives every host: rendered and raw Markdown, code comments, git commit
  messages, gherkin, TOML.
- House style: committed Markdown is ASCII only, and `--` is reserved for CLI
  flags, so a delimiter cannot rely on either.

## Considered options

A six-way design study scored each option across six reviewer lenses (raw-prose
reader, parser author, Markdown renderer, cross-host portability, AI-agent
author, migration). Aggregate scores in parentheses.

1. **Wiki-link wrapper `[[rr:anchor]]` (50).** A paired ASCII delimiter with an
   `rr:` sentinel. Chosen.
2. **Inline code span (40).** Clean in Markdown, but backticks are literal in
   commit messages, gherkin, TOML, and comments, so it fails the cross-host
   driver.
3. **Scheme prefix `rr:anchor` (35).** No closing delimiter, so the boundary is
   undefined for space-bearing anchors.
4. **Typed wrapper `[[rr:symbol:anchor]]` (34).** Carries the kind and
   disambiguates, but the verbosity and the not-yet-stored kind made it
   premature.
5. **Markdown-native link (21).** Only delimits where a Markdown renderer exists.
6. **Restricted anchor grammar, no wrapper (19).** Cannot close the leak for bare
   grammar-bearing tokens.

## Decision outcome

Adopt a clear split:

- An **anchor (address)** is the stable identity, exactly as it lives in the
  index and as `rr at` prints by default. It is the CLI argument to the readers
  and writers.
- A **citation** is the delimited embedded form that lives inside a host document
  and is what the scanners find:

```text
[[rr:<escaped-anchor>]]            live reference
[[rr:<escaped-anchor>]]@<commit>   snapshot pin
[[rr:<escaped-anchor>]]~<commit>   tracking pin
```

Load-bearing rules (the full normative grammar belongs in the parser module
docs, not this record):

- The opener is the fixed five bytes `[[rr:`; the terminator is `]]`; any pin
  sits outside `]]`, so it parses offline with no fresh index.
- `rr at` and `rr cite` escape every literal `[` and `]` in the anchor as `\[`
  and `\]`. Uniform per-byte escaping (not just escaping a `]]` run) is required
  so that anchors ending in `]` (the manifest-key kind, such as
  `pyproject.toml#[tool.poetry] name`) round-trip instead of silently
  truncating.
- The canonical extraction regex is
  `\[\[rr:(?:\\.|[^\\\]\[\t\n\r])*?\]\](?:[@~][0-9a-fA-F]{7,40})?`, run per line.
  The `rr:` sentinel after `[[` is what gives it no false positives in ordinary
  prose.
- `rr at` keeps printing the bare address by default, so the
  `rr read "$(rr at file:line)"` pipe and the existing tests stay valid;
  `rr at --cite` prints the document form. The bracketed form is an active shell
  glob, so emitting it by default would break terminal paste.
- The readers strip a pasted `[[rr:...]]` wrapper before resolving, so a copied
  citation works as a CLI argument.
- Markdown house style backtick-wraps a citation to neutralize GitHub wiki-link
  and autolink behavior, but it is recommended, not enforced (see consequence
  R1).

### Consequences

Good:

- The scanners `rr search` and `rr enforce`, the hook positive case, and
  completion triggers become implementable; they were blocked, not merely
  unscheduled.
- Pins parse deterministically offline, removing today's dependence on a fresh
  index plus known-anchor-wins.
- The form is readable and greppable in every host (`rg '\[\[rr:'`).

Carried forward, not solved here:

- R1: a backtick-wrapped citation lives in a Markdown code span, which the
  structural prose scanner currently skips. The scanner must treat a code span
  beginning with `[[rr:` as eligible. This record uses the bare form in prose so
  the future scanner finds its own citations directly.
- R3 and R4: the wrapper makes citations findable but not rename-stable, and
  path-less kinds (record, operation) still cannot disambiguate two same-named
  definitions. These motivate a future `rr rename` and an ambiguous-reference
  lint.
- R6: nothing normalizes Unicode form, so an NFD heading versus an NFC citation
  reads as dangling.

## Dogfooding

This record is written the way the tool intends:

- Its heading defines the record anchor `AD-1`. Until the record extractor lands
  (see follow-up), `rr index` already indexes the heading text as a heading
  anchor, so the decision is resolvable today.
- Every reference to code below uses the citation form, so a single scan finds
  them all. The dispatch path is [[rr:run_read]] in [[rr:src/commands.rs]]; the
  reference parser is [[rr:parse_reference]] in [[rr:src/cli.rs]]; the
  bare-anchor emitter this record changes is [[rr:at_text]]; the JSON envelope is
  [[rr:at_json]]; the forward-map lookup is [[rr:forward_lookup]] in
  [[rr:src/refidx.rs]].
- `rr at doc/adr/0001-citation-syntax.md:1` names the anchor a reader should cite
  to point back at this decision.

## Follow-up work

Tracked separately, each a discrete change rather than part of this record:

- A record extractor so `AD-1` resolves as a record, not only as a heading.
- A `kind` field in the index (`refidx v2`) so cross-kind collisions are
  resolvable and the JSON envelope can carry `kind`.
- Implement `rr search` and `rr enforce` over the canonical regex, with
  normalize-before-lookup.
- Teach the readers to strip a pasted citation wrapper.
- Fix [[rr:parse_position]] for column and Windows-drive inputs, a separate
  defect surfaced during this study.
