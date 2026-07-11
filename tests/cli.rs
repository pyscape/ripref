//! End-to-end tests of the five-verb contract: `rr index` builds the one
//! index, `rr read` and `rr at` invert each other over it, `rr search` lists
//! markers with no index at all, and `rr verify` judges the references scoped
//! text writes. Exit codes follow the one model of the output contract: 0 the
//! answer, 1 the adverse answer, 2 usage, 3 a stale index.
//!
//! Drives the real `rr` binary in a throwaway `Dir` so the whole pipeline
//! (walk -> extract -> serialize -> mmap -> resolve -> judge) is exercised
//! together. Scratch-dir setup and teardown live in `common`.

mod common;

use std::ffi::OsString;
use std::process::{Command, Output};

use common::{code, Dir, TestCommand};

// --- index + read: the forward path ------------------------------------------

// Pins the index summary format (anchors, mentions, files) and the read
// output (`file:start-end`, one definition per line). The heading anchor
// spans its whole section, not just the title line.
rrtest!(
    index_then_read_roundtrip,
    |mut dir: Dir, mut cmd: TestCommand| {
        dir.file("src/main.rs", "fn main() {}\n")
            .file("README.md", "# title\n\nsee src/main.rs here\n");

        let idx = cmd.arg("index").run();
        assert_eq!(code(&idx), 0, "index should succeed: {idx:?}");
        // 2 anchors: the README's heading (a section) and the `main` fn. One
        // mention: the prose path src/main.rs in the README.
        let summary = String::from_utf8_lossy(&idx.stdout);
        assert!(
            summary.contains("indexed 2 anchors and 1 path mentions across 2 files"),
            "{summary}"
        );

        let loc = cmd.args(["read", "main"]).stdout();
        assert_eq!(loc.trim(), "src/main.rs:1-1");
        let section = cmd.args(["read", "title"]).stdout();
        assert_eq!(section.trim(), "README.md:1-3", "headings span sections");
    }
);

rrtest!(
    unknown_anchor_exits_one,
    |mut dir: Dir, mut cmd: TestCommand| {
        dir.file("a.md", "# alpha\n");
        cmd.arg("index").assert_exit_code(0);
        cmd.args(["read", "does-not-exist"]).assert_exit_code(1);
    }
);

rrtest!(
    ambiguous_read_prints_each_definition_and_exits_one,
    |mut dir: Dir, mut cmd: TestCommand| {
        dir.file("a.md", "## Dup\n\nbody a\n")
            .file("b.md", "## Dup\n\nbody b\n");
        cmd.arg("index").assert_exit_code(0);

        let out = cmd.args(["read", "Dup"]).run();
        assert_eq!(code(&out), 1, "ambiguity is the adverse answer: {out:?}");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains("a.md:1-3"), "{stdout}");
        assert!(stdout.contains("b.md:1-3"), "{stdout}");

        // The path qualifier is the writer's fix; both spellings resolve.
        let one = cmd.args(["read", "a.md#Dup"]).stdout();
        assert_eq!(one.trim(), "a.md:1-3");
        let marker = cmd.args(["read", "[[rr:b.md#Dup]]"]).stdout();
        assert_eq!(marker.trim(), "b.md:1-3");
    }
);

// A record title defines the ID as the identity; the record spans the file.
rrtest!(
    record_title_defines_the_id,
    |mut dir: Dir, mut cmd: TestCommand| {
        dir.file(
            "doc/x.md",
            "# AD-7: A worked decision\n\nbody\n\n## Decision outcome\n\ntext\n",
        );
        cmd.arg("index").assert_exit_code(0);

        let loc = cmd.args(["read", "AD-7"]).stdout();
        assert_eq!(loc.trim(), "doc/x.md:1-7", "the record spans its region");

        // The full title is not an anchor; the ID is.
        cmd.args(["read", "AD-7: A worked decision"])
            .assert_exit_code(1);

        // The inverse direction: a body line answers with the record marker.
        let at = cmd.args(["at", "doc/x.md:3"]).stdout();
        assert_eq!(at.trim(), "[[rr:AD-7]]");
    }
);

rrtest!(
    stale_index_exits_three,
    |mut dir: Dir, mut cmd: TestCommand| {
        dir.file("a.md", "# alpha\n");
        cmd.arg("index").assert_exit_code(0);

        // The freshness check is second-granular and same-second writes are
        // fresh by design, so wait past the boundary before modifying.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        dir.write("a.md", "# alpha\n\nchanged\n");

        let out = cmd.args(["read", "alpha"]).run();
        assert_eq!(code(&out), 3, "stale index should exit 3: {out:?}");
        assert!(String::from_utf8_lossy(&out.stderr).contains("stale"));
    }
);

rrtest!(
    missing_index_exits_three,
    |mut dir: Dir, mut cmd: TestCommand| {
        dir.file("a.md", "# alpha\n");
        // No `rr index` run first: the reader can't answer, so it asks for a
        // rebuild.
        let out = cmd.args(["read", "alpha"]).run();
        assert_eq!(code(&out), 3, "absent index should exit 3: {out:?}");
        assert!(String::from_utf8_lossy(&out.stderr).contains("run `rr index`"));
    }
);

rrtest!(
    heading_sections_nest_by_rank,
    |mut dir: Dir, mut cmd: TestCommand| {
        dir.copy_fixture("heading-anchors.md", "README.md");
        cmd.arg("index").assert_exit_code(0);
        // The H4 runs to end of file; the H1 spans the whole document.
        let inner = cmd.args(["read", "Freshness"]).stdout();
        assert_eq!(inner.trim(), "README.md:5-7");
        let outer = cmd.args(["read", "ripref"]).stdout();
        assert_eq!(outer.trim(), "README.md:1-7");
    }
);

// An identity that carries grammar bytes (an email heading) reads literally.
rrtest!(
    email_anchor_reads_literally,
    |mut dir: Dir, mut cmd: TestCommand| {
        dir.file("README.md", "## support@example.com\n\nbody\n");
        cmd.arg("index").assert_exit_code(0);
        let out = cmd.args(["read", "support@example.com"]).stdout();
        assert_eq!(out.trim(), "README.md:1-3");
    }
);

// --- at: the inverse path -----------------------------------------------------

// `at` prints the marker (the form a person pastes), innermost by span;
// `--all` reports the whole nest, outermost first.
rrtest!(
    at_prints_marker_innermost_and_all_nest,
    |mut dir: Dir, mut cmd: TestCommand| {
        dir.file("guide.md", "# Guide\n\nintro\n\n## Config\n\nbody\n");
        cmd.arg("index").assert_exit_code(0);

        let tightest = cmd.args(["at", "guide.md:6"]).stdout();
        assert_eq!(tightest.trim(), "[[rr:Config]]");

        let all = cmd.args(["at", "guide.md:6", "--all"]).stdout();
        assert_eq!(
            all.lines().collect::<Vec<_>>(),
            ["[[rr:Guide]]", "[[rr:Config]]"],
            "{all:?}"
        );

        // The round trip: what `at` prints, `read` resolves.
        let marker = tightest.trim().to_string();
        let loc = cmd.args(["read", &marker]).stdout();
        assert_eq!(loc.trim(), "guide.md:5-7");
    }
);

// The minimal unambiguous form: an identity defined twice is emitted
// path-qualified, so the pasted marker resolves uniquely.
rrtest!(
    at_qualifies_an_ambiguous_identity,
    |mut dir: Dir, mut cmd: TestCommand| {
        dir.file("a.md", "## Dup\n\nbody a\n")
            .file("b.md", "## Dup\n\nbody b\n");
        cmd.arg("index").assert_exit_code(0);

        let marker = cmd.args(["at", "a.md:2"]).stdout();
        assert_eq!(marker.trim(), "[[rr:a.md#Dup]]");
        let loc = cmd.args(["read", marker.trim()]).stdout();
        assert_eq!(loc.trim(), "a.md:1-3");
    }
);

rrtest!(
    at_line_with_no_anchor_exits_one,
    |mut dir: Dir, mut cmd: TestCommand| {
        // A text file defines no anchors at all, so every line is uncovered
        // and `at` reports the adverse answer rather than minting a path.
        dir.file("solo.txt", "the only line\n");
        cmd.arg("index").assert_exit_code(0);
        let out = cmd.args(["at", "solo.txt:1"]).run();
        assert_eq!(code(&out), 1, "no covering anchor should exit 1: {out:?}");
        assert!(String::from_utf8_lossy(&out.stderr).contains("no anchor covers"));
    }
);

rrtest!(
    at_malformed_position_exits_two,
    |_dir: Dir, mut cmd: TestCommand| {
        // Position syntax is validated before dispatch, so no index is
        // needed.
        cmd.args(["at", "a.txt"]).assert_exit_code(2);
        cmd.args(["at", "a.txt:nope"]).assert_exit_code(2);
    }
);

rrtest!(
    at_missing_or_stale_index_exits_three,
    |mut dir: Dir, mut cmd: TestCommand| {
        dir.file("a.md", "# alpha\n");
        let out = cmd.args(["at", "a.md:1"]).run();
        assert_eq!(code(&out), 3, "absent index should exit 3: {out:?}");

        cmd.arg("index").assert_exit_code(0);
        std::thread::sleep(std::time::Duration::from_millis(1100));
        dir.write("a.md", "# alpha\n\nchanged\n");
        let out = cmd.args(["at", "a.md:1"]).run();
        assert_eq!(code(&out), 3, "stale index should exit 3: {out:?}");
    }
);

// The at envelope: data.anchors is always a list, each entry the bare anchor,
// the composed marker, and the definition's location.
rrtest!(at_json_envelope, |mut dir: Dir, mut cmd: TestCommand| {
    dir.file("guide.md", "# Guide\n\nbody\n");
    cmd.arg("index").assert_exit_code(0);
    let out = cmd.args(["at", "guide.md:2", "--format", "json"]).stdout();
    assert!(out.contains(r#""format":"rr-json""#), "{out}");
    assert!(out.contains(r#""version":1"#), "{out}");
    assert!(out.contains(r#""command":"at""#), "{out}");
    assert!(out.contains(r#""anchors":[{"anchor":"Guide""#), "{out}");
    assert!(out.contains(r#""marker":"[[rr:Guide]]""#), "{out}");
    assert!(
        out.contains(r#""location":{"file":"guide.md","start_line":1,"end_line":3}"#),
        "{out}"
    );
});

rrtest!(
    at_json_not_found_still_emits_envelope,
    |mut dir: Dir, mut cmd: TestCommand| {
        dir.file("solo.txt", "text\n");
        cmd.arg("index").assert_exit_code(0);
        let out = cmd.args(["at", "solo.txt:1", "--format", "json"]).run();
        assert_eq!(code(&out), 1, "{out:?}");
        let s = String::from_utf8_lossy(&out.stdout);
        assert!(s.contains(r#""anchors":[]"#), "{s}");
    }
);

// --- read input hygiene -------------------------------------------------------

// A pasted marker is stripped and unescaped before resolving.
rrtest!(
    read_strips_pasted_marker,
    |mut dir: Dir, mut cmd: TestCommand| {
        dir.file("src/main.rs", "fn main() {}\n");
        cmd.arg("index").assert_exit_code(0);
        let loc = cmd.args(["read", "[[rr:main]]"]).stdout();
        assert_eq!(loc.trim(), "src/main.rs:1-1", "{loc:?}");
    }
);

// A token that opens like a marker but is not one is a usage error (exit 2),
// never a silent fall-through to bare parsing. Nothing follows a terminator.
rrtest!(
    read_malformed_marker_exits_two,
    |_dir: Dir, mut cmd: TestCommand| {
        cmd.args(["read", "[[rr:a]]@a1b2c3d"]).assert_exit_code(2);
        cmd.args(["read", "[[rr:a"]).assert_exit_code(2);
        cmd.args(["read", r"[[rr:a\zb]]"]).assert_exit_code(2);
    }
);

// --- search: lexical, index-free ----------------------------------------------

// `search` lists the markers scoped text writes and needs no index at all;
// the anchor argument filters, matching qualified markers by identity.
rrtest!(
    search_lists_and_filters_without_an_index,
    |mut dir: Dir, mut cmd: TestCommand| {
        dir.file(
            "notes.md",
            "see [[rr:main]] and [[rr:src/x.md#main]] and [[rr:other]]\n\n\
             ```\n[[rr:fenced]] is invisible\n```\n",
        );
        // Deliberately no `rr index`: search is purely lexical.
        let all = cmd.arg("search").stdout();
        assert!(all.contains("notes.md:1: [[rr:main]]"), "{all}");
        assert!(all.contains("notes.md:1: [[rr:src/x.md#main]]"), "{all}");
        assert!(all.contains("3 markers"), "fenced marker excluded: {all}");

        let filtered = cmd.args(["search", "main"]).stdout();
        assert!(
            filtered.contains("2 markers"),
            "unqualified filter matches the qualified marker too: {filtered}"
        );
        let exact = cmd.args(["search", "src/x.md#main"]).stdout();
        assert!(exact.contains("1 markers"), "{exact}");

        let none = cmd.args(["search", "zzz-absent"]).run();
        assert_eq!(code(&none), 1, "no match is the adverse answer: {none:?}");
    }
);

rrtest!(
    search_mentions_lists_prose_paths,
    |mut dir: Dir, mut cmd: TestCommand| {
        dir.file(
            "notes.md",
            "the parser in src/cli.rs and a compound and/or here\n",
        );
        let out = cmd.args(["search", "--mentions"]).stdout();
        assert!(out.contains("notes.md:1: src/cli.rs"), "{out}");
        assert!(
            out.contains("notes.md:1: and/or"),
            "search lists every mention; judgment is verify's: {out}"
        );
        assert!(out.contains("2 mentions"), "{out}");
    }
);

// --- verify: the gate ----------------------------------------------------------

// The six finding kinds, one fixture: the corpus under tests/data carries one
// violation per line plus a clean section that must produce nothing.
rrtest!(
    verify_reports_the_six_finding_kinds,
    |mut dir: Dir, mut cmd: TestCommand| {
        dir.copy_fixture("marker-violations.md", "corpus.md");
        dir.file("src/lib.rs", "fn lib() {}\n")
            .file("a.md", "## Dup\n\nbody a\n")
            .file("b.md", "## Dup\n\nbody b\n");
        cmd.arg("index").assert_exit_code(0);

        let out = cmd.arg("verify").run();
        assert_eq!(code(&out), 1, "findings are the adverse answer: {out:?}");
        let s = String::from_utf8_lossy(&out.stdout);
        for rule in [
            "malformed marker",
            "dangling marker",
            "ambiguous marker",
            "path-only marker",
            "bare path:line reference",
            "stale path mention",
        ] {
            assert!(s.contains(rule), "missing {rule:?} in:\n{s}");
        }
        assert!(s.contains("7 findings"), "{s}");
    }
);

rrtest!(
    verify_clean_tree_exits_zero,
    |mut dir: Dir, mut cmd: TestCommand| {
        dir.file("a.md", "# Alpha\n\nsee [[rr:Alpha]] and src/ok.rs here\n")
            .file("src/ok.rs", "fn ok() {}\n");
        cmd.arg("index").assert_exit_code(0);
        let out = cmd.arg("verify").run();
        assert_eq!(code(&out), 0, "clean tree: {out:?}");
        assert!(String::from_utf8_lossy(&out.stdout).contains("0 findings"));
    }
);

// The profile narrows scope: a project's .rr.toml excludes its fixtures, so
// deliberate violations there are never judged.
rrtest!(
    verify_honors_project_scope_excludes,
    |mut dir: Dir, mut cmd: TestCommand| {
        dir.file(".rr.toml", "[verify]\nexclude = [\"fixtures/**\"]\n")
            .file("fixtures/bad.md", "a dangling [[rr:nope]] here\n")
            .file("a.md", "# Alpha\n");
        cmd.arg("index").assert_exit_code(0);
        let out = cmd.arg("verify").run();
        assert_eq!(code(&out), 0, "excluded fixtures are not judged: {out:?}");
    }
);

rrtest!(
    verify_stale_index_exits_three,
    |mut dir: Dir, mut cmd: TestCommand| {
        dir.file("a.md", "# Alpha\n");
        cmd.arg("index").assert_exit_code(0);
        std::thread::sleep(std::time::Duration::from_millis(1100));
        dir.write("a.md", "# Alpha\n\nmore\n");
        let out = cmd.arg("verify").run();
        assert_eq!(
            code(&out),
            3,
            "verify refuses to judge from stale data: {out:?}"
        );
    }
);

rrtest!(
    verify_json_envelope,
    |mut dir: Dir, mut cmd: TestCommand| {
        dir.file("a.md", "a dangling [[rr:nope]] here\n");
        cmd.arg("index").assert_exit_code(0);
        let out = cmd.args(["verify", "--format", "json"]).run();
        assert_eq!(code(&out), 1, "{out:?}");
        let s = String::from_utf8_lossy(&out.stdout);
        assert!(s.contains(r#""command":"verify""#), "{s}");
        assert!(
            s.contains(r#""findings":[{"file":"a.md","line":1,"rule":"dangling marker"}]"#),
            "{s}"
        );
    }
);

// --- the index artifact ---------------------------------------------------------

/// The default index path within a test [`Dir`].
fn index_file(dir: &Dir) -> std::path::PathBuf {
    dir.path().join(".ref-cache").join("index")
}

/// The bytes of an index after its header (everything from the blank line
/// that terminates the section table onward), with the build-time `mtime`
/// stamp excluded.
fn index_body(dir: &Dir) -> Vec<u8> {
    let bytes = std::fs::read(index_file(dir)).expect("index file exists");
    let header_end = bytes
        .windows(2)
        .position(|w| w == b"\n\n")
        .map(|p| p + 2)
        .expect("index header ends in a blank line");
    bytes[header_end..].to_vec()
}

// The on-disk format: `refidx v2`, three sections, and no content-addressing
// fields — the index is a pure locations file and the only artifact rr
// writes.
rrtest!(index_is_refidx_v2, |mut dir: Dir, mut cmd: TestCommand| {
    dir.file("src/main.rs", "fn main() {}\n")
        .file("README.md", "# title\n\nsee src/main.rs here\n");
    cmd.arg("index").assert_exit_code(0);

    let bytes = std::fs::read(index_file(&dir)).unwrap();
    let header_end = bytes.windows(2).position(|w| w == b"\n\n").unwrap();
    let header = String::from_utf8(bytes[..header_end].to_vec()).unwrap();

    assert!(header.starts_with("refidx v2\n"), "magic line: {header:?}");
    for section in ["section:forward:", "section:mentions:", "section:paths:"] {
        assert!(header.contains(section), "missing {section} in {header:?}");
    }
    for forbidden in ["blob", "oid", "pin", "snapshot", "track"] {
        assert!(
            !header.contains(forbidden),
            "header must not carry a {forbidden:?} field: {header:?}"
        );
    }
});

// Indexing the same tree twice yields a byte-identical body: the parallel
// walk hands records back in a scheduling-dependent order, and serialize's
// total order recovers determinism.
rrtest!(
    index_unchanged_tree_is_byte_stable,
    |mut dir: Dir, mut cmd: TestCommand| {
        for i in 0..12 {
            dir.file(
                &format!("src/mod{i}.rs"),
                &format!("pub fn f{i}() {{}}\npub struct S{i};\n"),
            );
        }
        dir.file(
            "a.md",
            "# Alpha\n\nsee src/mod0.rs and src/mod1.rs\n\n## Beta\n",
        );

        cmd.arg("index").assert_exit_code(0);
        let first = index_body(&dir);
        cmd.arg("index").assert_exit_code(0);
        let second = index_body(&dir);

        assert_eq!(
            first, second,
            "index body must be deterministic across builds"
        );
        assert!(!first.is_empty(), "body should carry the records");
    }
);

// A truncated index whose header still parses but whose section bytes are
// gone must be reported as corrupt, never crash with an out-of-bounds panic.
rrtest!(
    truncated_index_is_corrupt_not_panic,
    |mut dir: Dir, mut cmd: TestCommand| {
        dir.file("a.md", "# Alpha\n\nbody\n");
        cmd.arg("index").assert_exit_code(0);

        let path = index_file(&dir);
        let bytes = std::fs::read(&path).unwrap();
        let header_end = bytes
            .windows(2)
            .position(|w| w == b"\n\n")
            .map(|p| p + 2)
            .unwrap();
        assert!(header_end < bytes.len(), "the built index has a body");
        std::fs::write(&path, &bytes[..header_end]).unwrap();

        let out = cmd.args(["read", "Alpha", "--no-freshness"]).run();
        assert_eq!(
            code(&out),
            2,
            "truncated index should be a clean usage error, not a panic: {out:?}"
        );
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("corrupt index"),
            "stderr should name the corrupt index: {:?}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
);

// --- freshness ------------------------------------------------------------------

// `--no-freshness` answers from the index as-is; the same edit that makes a
// plain read exit 3 must still resolve under the flag.
rrtest!(
    no_freshness_skips_stale_check,
    |mut dir: Dir, mut cmd: TestCommand| {
        dir.file("a.md", "# Alpha\n");
        cmd.arg("index").assert_exit_code(0);

        std::thread::sleep(std::time::Duration::from_millis(1100));
        dir.write("a.md", "# Alpha\n\nchanged\n");

        cmd.args(["read", "Alpha"]).assert_exit_code(3);
        cmd.args(["read", "Alpha", "--no-freshness"])
            .assert_exit_code(0);
    }
);

// The git-tree short-circuit serves a clean checkout whose HEAD matches the
// stamp without stat-ing; a dirty tree falls through to a real stale verdict.
rrtest!(
    git_clean_tree_short_circuits_freshness,
    |mut dir: Dir, mut cmd: TestCommand| {
        if !git_available() {
            eprintln!("skipping: git not available");
            return;
        }
        dir.file("a.md", "# Alpha\n");
        init_repo(&dir);
        commit_all(&dir, "init");

        cmd.arg("index").assert_exit_code(0);

        // Cross the second boundary, then rewrite with identical content:
        // mtime bumps, tree stays clean, the short-circuit answers.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        dir.write("a.md", "# Alpha\n");
        cmd.args(["read", "Alpha"]).assert_exit_code(0);

        // A real edit dirties the tree and the stat-walk sees it: stale.
        dir.write("a.md", "# Alpha\n\nchanged\n");
        cmd.args(["read", "Alpha"]).assert_exit_code(3);
    }
);

// A live read on a clean tree DOES run git (the freshness short-circuit), so
// the docs must not claim "no git on the read path".
rrtest!(
    live_read_clean_tree_invokes_git,
    |mut dir: Dir, mut cmd: TestCommand| {
        if !git_available() {
            eprintln!("skipping: git not available");
            return;
        }
        dir.file("a.md", "# Alpha\n");
        init_repo(&dir);
        commit_all(&dir, "init");
        cmd.arg("index").assert_exit_code(0);

        let (out, git_calls) = run_rr_counting_git(&dir, &["read", "Alpha"]);
        assert_eq!(code(&out), 0, "clean-tree read should succeed: {out:?}");
        assert!(
            git_calls >= 1,
            "a clean-tree read must invoke git for the freshness short-circuit, got {git_calls}"
        );

        let readme = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("README.md"),
        )
        .unwrap();
        assert!(
            !readme.contains("no `git` on the read path")
                && !readme.contains("no git on the read path"),
            "README must not claim there is no git on the read path"
        );
    }
);

// --- CLI surface ------------------------------------------------------------------

rrtest!(version_exits_zero, |_dir: Dir, mut cmd: TestCommand| {
    let v = cmd.arg("--version").run();
    assert_eq!(code(&v), 0);
    assert!(String::from_utf8_lossy(&v.stdout).starts_with("rr "));
});

rrtest!(
    help_advertises_exactly_five_verbs,
    |_dir: Dir, mut cmd: TestCommand| {
        let h = cmd.arg("--help").run();
        assert_eq!(code(&h), 0);
        let text = String::from_utf8_lossy(&h.stdout).into_owned();
        assert!(text.contains("USAGE"));
        for verb in ["index", "read", "at", "search", "verify"] {
            assert!(
                text.contains(&format!("\n    {verb} ")),
                "help missing {verb}:\n{text}"
            );
        }
        for gone in ["cite", "track", "uncite", "untrack", "enforce"] {
            assert!(
                !text.contains(&format!("\n    {gone} ")),
                "help must not advertise {gone}:\n{text}"
            );
        }
    }
);

rrtest!(
    unknown_and_dropped_subcommands_exit_two,
    |_dir: Dir, mut cmd: TestCommand| {
        cmd.arg("frobnicate").assert_exit_code(2);
        for gone in ["cite", "track", "uncite", "untrack", "enforce"] {
            let out = cmd.args([gone, "x"]).run();
            assert_eq!(code(&out), 2, "{gone} must be an unknown command");
            assert!(
                String::from_utf8_lossy(&out.stderr).contains("unknown command"),
                "{gone}: {out:?}"
            );
        }
    }
);

// --- dogfood ------------------------------------------------------------------------

// Drive `rr` against its own source tree: the records under doc/ad resolve
// by their IDs, the `rrtest!` macro that generates this very test resolves
// and round-trips through `at`, and `search` finds the markers the records
// write — all with the index in a throwaway location, never the repo's own
// cache.
rrtest!(
    dogfoods_own_records_and_source,
    |dir: Dir, _cmd: TestCommand| {
        let index = dir.path().join("index");
        let run = |args: &[&str]| -> Output {
            let mut full: Vec<OsString> = args.iter().map(|a| OsString::from(*a)).collect();
            full.push("--index".into());
            full.push(index.clone().into_os_string());
            Command::new(env!("CARGO_BIN_EXE_rr"))
                .args(&full)
                .current_dir(env!("CARGO_MANIFEST_DIR"))
                .output()
                .expect("failed to spawn rr")
        };

        let idx = run(&["index"]);
        assert_eq!(code(&idx), 0, "indexing our own tree: {idx:?}");

        // The domain record resolves by its ID and lives where it should.
        let ad1 = run(&["read", "AD-1"]);
        assert_eq!(code(&ad1), 0, "AD-1 must resolve: {ad1:?}");
        let loc = String::from_utf8_lossy(&ad1.stdout).trim().to_string();
        assert!(
            loc.starts_with("doc/ad/0001-domain-model.md:1-"),
            "AD-1 spans its record: {loc}"
        );

        // A pasted record marker resolves identically.
        let ad3 = run(&["read", "[[rr:AD-3]]"]);
        assert_eq!(code(&ad3), 0, "a pasted record marker resolves: {ad3:?}");

        // The macro that generates this test resolves, and its location
        // round-trips through `at --all`.
        let sym = run(&["read", "rrtest"]);
        assert_eq!(code(&sym), 0, "read macro anchor: {sym:?}");
        let sym_loc = String::from_utf8_lossy(&sym.stdout).trim().to_string();
        let (sym_file, sym_span) = sym_loc.rsplit_once(':').expect("file:span");
        assert_eq!(sym_file, "tests/common/mod.rs", "{sym_loc}");
        let (start, end) = sym_span.split_once('-').expect("start-end");
        assert!(
            start.parse::<u64>().unwrap() < end.parse::<u64>().unwrap(),
            "the macro definition spans multiple lines: {sym_loc}"
        );

        let at = run(&["at", &format!("{sym_file}:{start}"), "--all"]);
        assert_eq!(code(&at), 0, "reverse lookup: {at:?}");
        let nest = String::from_utf8_lossy(&at.stdout);
        assert!(
            nest.lines().any(|l| l == "[[rr:rrtest]]"),
            "the macro round-trips through at: {nest}"
        );

        // The records write markers of each other; search finds them with no
        // index at all.
        let search = run(&["search", "AD-1"]);
        assert_eq!(code(&search), 0, "search over own tree: {search:?}");
        let listing = String::from_utf8_lossy(&search.stdout);
        assert!(
            listing.lines().any(|l| l.starts_with("doc/ad/")),
            "the sibling records reference AD-1: {listing}"
        );
    }
);

// --- git helpers ----------------------------------------------------------------

/// Whether a `git` binary is on PATH; the short-circuit tests need a repo.
fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Run `git <args>` in the test dir, asserting success.
fn git(dir: &Dir, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir.path())
        .output()
        .expect("failed to spawn git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Initialize a git repo in the test dir with a deterministic identity, and
/// gitignore the index dir so `rr index` never dirties the tree it stamps.
fn init_repo(dir: &Dir) {
    dir.write(".gitignore", ".ref-cache/\n");
    git(dir, &["init", "-q"]);
    git(dir, &["config", "user.email", "t@example.com"]);
    git(dir, &["config", "user.name", "t"]);
}

/// `git add . && git commit -m <msg>` in the test dir.
fn commit_all(dir: &Dir, msg: &str) {
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", msg]);
}

/// Run `rr <args>` in `dir` while counting the top-level git processes it
/// spawns, via `GIT_TRACE2_EVENT` (each git process emits exactly one
/// `version` event). The trace lands outside the repo so it is never walked.
fn run_rr_counting_git(dir: &Dir, args: &[&str]) -> (Output, usize) {
    let tag = dir
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let trace = std::env::temp_dir().join(format!("rr-trace-{tag}.json"));
    std::fs::remove_file(&trace).ok();
    let out = Command::new(env!("CARGO_BIN_EXE_rr"))
        .args(args)
        .current_dir(dir.path())
        .env("GIT_TRACE2_EVENT", &trace)
        .output()
        .expect("failed to spawn rr");
    let count = std::fs::read_to_string(&trace)
        .map(|t| t.matches(r#""event":"version""#).count())
        .unwrap_or(0);
    std::fs::remove_file(&trace).ok();
    (out, count)
}
