# Citation reference corpus (test data for `rr search` / `rr enforce`)

Fixture for the planned scanners. It catalogs references found while auditing the
`[[rr:...]]` citation work, split into what an enforcer should flag, what is only
an advisory candidate, and the look-alikes it must NOT flag (precision guards).

Each data line below lives inside a fenced block and has the shape:

    VERDICT:category   <example text>

- VERDICT is `FLAG` (reliably a violation), `ADVISORY` (a candidate that
  over-matches ordinary prose, so low precision), or `CLEAN` (must not be
  flagged). The prefix has no spaces; the example is everything after the first
  run of whitespace.
- The example is a realistic line; a pattern/scanner test extracts the example
  field and asserts whether it is flagged.

Key caveat (this is why the marker exists, per AD-1): the bare record/symbol
classes are byte-identical to ordinary writing, so they can only be ADVISORY. The
two *reliable* detections are line-number references and dangling/malformed
markers. Several CLEAN cases share a byte-identical token with a FLAG/ADVISORY
case and differ only by context — a fenced block, a CLI argument position, a
table cell. See "Context-dependent collisions" below. A line-level regex cannot
tell them apart; the scanner needs the surrounding structure.

## FLAG: reliably detectable violations

```text
FLAG:line-number-ref   The handler is defined at src/handlers.py:8-26.
FLAG:line-number-ref   See src/parser.rs:118 for where the record parser lives.
FLAG:line-number-ref   A bare line number such as http.go:42 is not an anchor.
FLAG:line-number-ref   point the next reader at parser.go:42 and it rots immediately
FLAG:bare-file-path   the envelope spec lives in usecases/editor-completion.md
FLAG:bare-file-path   the on-disk format is specified in the refidx module (src/refidx.rs)
FLAG:bare-file-path   the conformance oracle is scripts/citation_regex_oracle.py
FLAG:bare-file-path   lint posture: src/lib.rs
FLAG:bare-file-path   see doc/adr/0001-citation-syntax.md for the rationale
FLAG:dangling-marker   the account model [[rr:legacy.Account]] no longer exists
FLAG:dangling-marker   [[rr:does_not_exist]]
FLAG:dangling-marker   the AD-1 record miscited as [[rr:ADR-1]] resolves to nothing
FLAG:malformed-marker   [[rr:a]]@zzz
FLAG:malformed-marker   [[rr:a]]@a1b2c3
FLAG:malformed-marker   [[rr:unterminated
FLAG:malformed-marker   [[rr:bad[bracket]]
```

## ADVISORY: grammar-bearing, over-matches prose (low precision)

```text
ADVISORY:bare-record   implemented by AD-42
ADVISORY:bare-record   This is AD-1's payoff.
ADVISORY:bare-record   the canonical AD-1 regex
ADVISORY:bare-symbol   the hand-rolled citation::decode
ADVISORY:bare-symbol   re-dispatching markers through run_read_pinned
ADVISORY:bare-symbol   require_pin decodes a pasted marker
ADVISORY:bare-symbol   the request flow starts in my_module::handler
```

## CLEAN: must NOT be flagged (precision guards)

```text
CLEAN:cli-input   rr at src/handlers.py:15
CLEAN:cli-input   rr at src/parser.rs:118
CLEAN:cli-input   rr read AD-42
CLEAN:cli-input   rr read src/main.rs --locate
CLEAN:tool-output   src/handlers.py:8-26
CLEAN:tool-output   docs/architecture.md:14
CLEAN:tool-output   docs/adr/0042-hand-rolled-parser.md:1-37
CLEAN:valid-marker   [[rr:src/handlers.py]]
CLEAN:valid-marker   [[rr:my_module::handler]]
CLEAN:valid-marker   [[rr:AD-42]]
CLEAN:valid-marker   [[rr:doc/adr/0001-citation-syntax.md]]
CLEAN:valid-marker   [[rr:support@example.com]]@a1b2c3d
CLEAN:address-table   | record | AD-42 |
CLEAN:address-table   | scenario | tests/features/auth.feature#"User can log in" |
CLEAN:address-table   | manifest key | pyproject.toml#[tool.poetry] name |
CLEAN:marker-syntax   write the marker form [[rr:anchor]] in prose
CLEAN:marker-syntax   a pin attaches outside the brackets: [[rr:anchor]]@<commit>
CLEAN:incidental-prose   See AD-42 and pyproject.toml#[tool.poetry] name for config.
CLEAN:incidental-prose   The handler() calls my_module::handler and emails support@example.com.
CLEAN:incidental-prose   a bare AD-42 is indistinguishable from prose
CLEAN:code-identifier   use crate::cli::Sigil;
CLEAN:code-identifier   let s = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/citation_regex_oracle.py");
CLEAN:code-identifier   dir.file("src/main.rs", "fn main() {}\n");
CLEAN:rustdoc-link   the shared tail is [`run_read_pinned`] and [`run_read`]
```

## Context-dependent collisions (same token, different verdict)

These pairs prove a line-level regex over-matches; the scanner must use context.

```text
FLAG:line-number-ref   the handler at src/handlers.py:8-26 is the entry point
CLEAN:tool-output   src/handlers.py:8-26
ADVISORY:bare-record   implemented by AD-42
CLEAN:address-table   | record | AD-42 |
CLEAN:incidental-prose   See AD-42 and the configuration section.
```

The carve-outs the scanner needs: skip fenced code blocks; skip a `file:line`
that sits in a CLI argument position (after `$ rr `, `rr at `, `rr read `, and
the like); skip address-form table cells; treat an already-wrapped `[[rr:...]]`
as the citation it is, not a bare token; and never claim a bare grammar token in
running prose is a "missing citation", which is undecidable (the whole reason the
marker exists).

## Candidate rg patterns

```sh
# P1  line-number reference (high precision). Run per line; then drop matches
#     inside fenced code, after an `rr `/`$ ` CLI lead, and the `]]:` of a marker.
rg -n -e '[A-Za-z0-9_./-]+\.[A-Za-z]{1,6}:[0-9]+(-[0-9]+)?'

# P2  every marker, to validate (dangling/malformed) by decoding + resolving.
rg -n -e '\[\[rr:'

# P3  grammar-bearing bare-token CANDIDATES (advisory; expect false positives on
#     incidental prose). Records and `::`-bearing symbols:
rg -n -e '\bAD-[0-9]+\b' -e '\b[A-Za-z_][A-Za-z0-9_]*(::[A-Za-z0-9_]+)+\b'
```

(rg's Rust regex has no look-behind, so "not already inside `[[rr:...]]`" is a
second pass: find markers with P2, then subtract their spans from P1/P3 hits.)

## Provenance

Drawn from the `[[rr:...]]` citation branch. Representative real occurrences:

- line-number-ref / address examples: README.md (`rr at` examples, the kinds
  table, the `rr enforce` illustration), usecases/adr-traceability.md.
- bare-file-path: src/commands.rs doc-comments (the JSON envelope spec pointer),
  src/citation.rs module doc, tests/citation_oracle.rs, scripts/citation_regex_oracle.py.
- bare-record (`AD-1` / `AD-42`): src/commands.rs and tests/cli.rs comments,
  scripts/citation_regex_oracle.py, usecases/adr-traceability.md prose.
- valid-marker / marker-syntax: src/citation.rs, src/commands.rs doc-comments
  (dogfooded, backtick-wrapped per AD-1 rule R1).
- incidental-prose: scripts/citation_regex_oracle.py NO_FALSE_POSITIVES corpus.
