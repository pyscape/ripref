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

