# AD-4: The output contract: text by default, rr-json for tools

- Status: Accepted
- Date: 2026-07-01
- Tags: cli, public-api, output

## Context and problem statement

Two readers consume rr's output: a person at a terminal, and a tool (an
editor, a CI step, a script). A person wants a marker to paste and a location
to read; a tool wants one stable, versioned record it can depend on across
versions. This record fixes what each verb of `[[rr:AD-3]]` prints as text,
what it prints as JSON, and how every result reaches the exit code.

## Decision drivers

- A person reads the default output with no flag.
- A tool parses one stable, versioned format across every verb.
- The result reaches the exit code, so a shell branches without parsing.
- `at` prints the form a person pastes.
- `--format` selects the shape for every verb alike; a verb may take flags
  that select what is reported, never how it is shaped.

## Considered options

- **`at` prints the bare anchor, and a flag adds the brackets.** The flag
  duplicates the one marker-producing verb (`[[rr:AD-3]]`), and the default
  hands a person the one form they never paste. Rejected.
- **`at` prints the marker; JSON carries the bare anchor.** A person pastes,
  a shell quotes, a tool reads the structured field. Taken.
- **Per-verb output flags.** Every verb grows its own shape switches and a
  tool learns five contracts. Rejected: one flag, every verb.

