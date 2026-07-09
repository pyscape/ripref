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

## Decision drivers

- Findable: one deterministic scan locates every marker, with a clear
  boundary rule, even for anchors containing spaces, `:`, `#`, `::`, `[`,
  `]`, `\`, quotes, `@`, `~`.
- Distinguishable from an incidental mention of the same word.
- Readable from raw, unrendered text.
- Round-trips: a writer emits it, a person or agent pastes it, and the
  readers parse it back to the same anchor, byte for byte.
- Survives every host: rendered and raw Markdown, code comments, git commit
  messages, gherkin, TOML.
- House style: committed Markdown is ASCII only, and `--` is reserved for
  CLI flags, so the delimiter can rely on neither.

## Considered options

A six-way design study scored each option across six reviewer lenses
(raw-prose reader, parser author, Markdown renderer, cross-host portability,
AI-agent author, adoption cost). Aggregate scores in parentheses.

1. **Wiki-link wrapper (50).** A paired ASCII delimiter with an `rr:`
   sentinel; the grammar block below shows the form. Chosen.
2. **Inline code span (40).** Clean in Markdown, but backticks are literal
   in commit messages, gherkin, TOML, and comments, so it fails the
   cross-host driver.
3. **Scheme prefix `rr:anchor` (35).** No closing delimiter, so the boundary
   is undefined for space-bearing anchors.
4. **Typed wrapper with a kind segment (34).** A second field carries the
   kind, but the kind is derivable at resolution and the verbosity taxes
   every writer.
5. **Markdown-native link (21).** Only delimits where a Markdown renderer
   exists.
6. **Restricted anchor grammar, no wrapper (19).** Cannot close the leak for
   bare grammar-bearing tokens.

## Decision outcome

A **marker** is the anchor between a fixed opener and terminator:

```text
[[rr:<escaped-anchor>]]
```

Load-bearing rules (the exact scanner grammar lives in the module docs
beside its implementation):

- The opener is the five fixed bytes the grammar block above opens with;
  the terminator is `]]`; nothing follows the terminator. A marker resolves
  or it dangles, carrying no other state, and rr stores no file content
  (`[[rr:AD-1]]` fixes both), so a suffix would name behavior rr cannot
  deliver.
- Writers escape every literal `[`, `]`, and `\` in the anchor as `\[`,
  `\]`, and `\\`. Uniform per-byte escaping, not an escape for a `]]` run
  alone, is what lets an anchor containing `]`, such as the key identity
  `[tool.poetry] name`, round-trip instead of silently truncating. Escaping
  the backslash itself is equally load-bearing: unescaped, an anchor ending
  in `\` would emit a `\]]` tail whose first two bytes parse as an escaped
  bracket, leaving the marker unterminated. These three escapes are the
  whole set; a `\` before any other byte makes the token malformed, and the
  gate reports it (AD-3).
- The canonical extraction regex, run per line, is:

```text
\[\[rr:(?:\\[\\\[\]]|[^\\\]\[\t\n\r])*?\]\]
```

  The `rr:` sentinel after `[[` is what gives it no false positives in
  ordinary prose. Tab, CR, and newline cannot occur in an anchor, and the
  index never mints one, so a marker always sits on one line.

- The readers strip the wrapper and then unescape `\[`, `\]`, and `\\`. The
  unescape step is normative: stripping alone would leave escape bytes in
  the anchor and dangle every bracket-bearing lookup. Comparison then
  follows the invariant of `[[rr:AD-1]]`: byte-for-byte after NFC.
- Scan regions are declared per host in the profile's configuration, beside
  the kinds of `[[rr:AD-1]]`. The default profile declares: in a Markdown
  host, the scanners read prose and inline code spans whose content begins
  with the opener; fenced code blocks are invisible, and a match never
  crosses a region boundary. In a host with no declared structure (a
  comment, a commit message), the regex runs per raw line. A document that
  needs to show an illustrative marker without writing one puts it in a
  region its host does not scan, as the fenced grammar block above does.
- House style backtick-wraps a marker in Markdown to neutralize wiki-link
  and autolink rendering; the span rule above means the wrapping costs no
  findability.

### Consequences

- One scan, no lookahead past the terminator, no index required to locate
  markers; the form is greppable in every host (`rg '\[\[rr:'`).
- The wrapper makes a marker findable, not rename-stable: when an anchor's
  identity changes, every marker of it dangles until edited. The gate
  reports the dangle (AD-3), and the mention table of
  AD-5 gives a future rename workflow the occurrence data it
  needs.
- A malformed token is a defect, not a mystery: an unpaired opener outside
  a fence is findable by the same scan that finds markers, so a typo
  demotes loudly rather than silently becoming prose.

Extension seams: a host format with its own structure, and its own way to
fence an example, joins by declaring its scan regions in configuration; the
grammar itself never changes for a host.

## Dogfooding

- The grammar and regex blocks above are fenced and therefore invisible to
  the scanners; every marker this record writes in prose resolves.
- This record's title defines `AD-2` under the record kind of
  `[[rr:AD-1]]`.
