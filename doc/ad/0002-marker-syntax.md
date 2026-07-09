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

