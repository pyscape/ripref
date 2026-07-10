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

