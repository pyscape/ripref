//! End-to-end walking-skeleton tests: `rr index` -> `rr read --locate`, plus the
//! `stat`-only freshness signal (exit 3) and the not-found signal (exit 1).
//!
//! Drives the real `rr` binary in a throwaway `Dir` so the whole pipeline
//! (walk -> serialize -> mmap -> binary-search -> freshness) is exercised
//! together. Scratch-dir setup and teardown live in `common`.

mod common;

use std::ffi::OsString;
use std::process::{Command, Output};

use common::{code, Dir, TestCommand};

// Pins the `--locate` output format (`path:start-end`) and the index summary
// format (`indexed N anchors across N files`). Single-line fixture keeps the
// expected `1-1` range unambiguous.
rrtest!(
    index_then_read_locate_roundtrip,
    |mut dir: Dir, mut cmd: TestCommand| {
        dir.file("src/main.rs", "fn main() {}\n")
            .file("README.md", "# title\n\nbody\n");

        let idx = cmd.arg("index").run();
        assert_eq!(code(&idx), 0, "index should succeed: {idx:?}");
        // 4 anchors: path anchors for both files, the README heading, and the
        // `main` fn from src/main.rs (Rust extraction).
        let summary = String::from_utf8_lossy(&idx.stdout);
        assert!(
            summary.contains("indexed 4 anchors across 2 files"),
            "{summary}"
        );

        let loc = cmd.args(["read", "src/main.rs", "--locate"]).stdout();
        assert_eq!(loc.trim(), "src/main.rs:1-1");
    }
);

rrtest!(
    unknown_anchor_exits_one,
    |mut dir: Dir, mut cmd: TestCommand| {
        dir.file("a.txt", "hello\n");
        cmd.arg("index").assert_exit_code(0);
        cmd.args(["read", "does/not/exist.rs", "--locate"])
            .assert_exit_code(1);
    }
);

rrtest!(
    stale_index_exits_three,
    |mut dir: Dir, mut cmd: TestCommand| {
        dir.file("a.txt", "v1\n");
        cmd.arg("index").assert_exit_code(0);

        // The freshness check is second-granular and same-second writes are fresh
        // by design, so wait past the second boundary before modifying the file.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        dir.write("a.txt", "v2 changed\n");

        let out = cmd.args(["read", "a.txt", "--locate"]).run();
        assert_eq!(code(&out), 3, "stale index should exit 3: {out:?}");
        assert!(String::from_utf8_lossy(&out.stderr).contains("stale"));
    }
);

rrtest!(
    missing_index_exits_three,
    |mut dir: Dir, mut cmd: TestCommand| {
        dir.file("a.txt", "hi\n");
        // No `rr index` run first: the reader can't answer, so it asks for a rebuild.
        let out = cmd.args(["read", "a.txt", "--locate"]).run();
        assert_eq!(code(&out), 3, "absent index should exit 3: {out:?}");
        assert!(String::from_utf8_lossy(&out.stderr).contains("run `rr index`"));
    }
);

rrtest!(
    heading_anchor_resolves,
    |mut dir: Dir, mut cmd: TestCommand| {
        dir.copy_fixture("heading-anchors.md", "README.md");
        cmd.arg("index").assert_exit_code(0);
        let loc = cmd.args(["read", "Freshness", "--locate"]).stdout();
        assert_eq!(loc.trim(), "README.md:5-5");
    }
);

// `rr at <file>:<line>` — the inverse of `read`. A multi-line fixture makes the
// whole-file path anchor's span (`1-3`) genuinely wider than the single-line
// `main` symbol (`1-1`), so outermost-first lists the path anchor first.
rrtest!(
    at_returns_covering_anchors,
    |mut dir: Dir, mut cmd: TestCommand| {
        dir.file("src/main.rs", "fn main() {\n    let x = 1;\n}\n");
        cmd.arg("index").assert_exit_code(0);

        let out = cmd.args(["at", "src/main.rs:1"]).stdout();
        let lines: Vec<&str> = out.lines().collect();
        // Outermost (the whole-file path anchor) is first.
        assert_eq!(lines[0], "src/main.rs\tsrc/main.rs:1-3", "{out:?}");
        // The `main` symbol (single-line span at its definition) is also covered.
        assert!(out.contains("main\tsrc/main.rs:1-1"), "{out:?}");
    }
);

// A reference to the blank line *between* two top-level items. Symbol anchors are
// single-line spans at their definitions, so the gap between sections sits inside
// no symbol — only the whole-file path anchor covers it. The file's own anchor
// still answers, so this is a found (exit 0) result, not a miss. The fixture has
// `first` on line 1 and `second` on line 5, with line 4 blank between them.
rrtest!(
    at_blank_line_between_sections_resolves_to_file_only,
    |mut dir: Dir, mut cmd: TestCommand| {
        dir.copy_fixture("sectioned-code.rs", "sections.rs");
        cmd.arg("index").assert_exit_code(0);

        // A real code line is covered by both the file anchor and its symbol.
        let on_code = cmd.args(["at", "sections.rs:1"]).stdout();
        assert!(on_code.contains("first\tsections.rs:1-1"), "{on_code:?}");

        // The blank line 4 falls between `first` and `second`: the file anchor alone.
        let in_gap = cmd.args(["at", "sections.rs:4"]).stdout();
        assert_eq!(
            in_gap.trim(),
            "sections.rs\tsections.rs:1-7",
            "blank-line gap should resolve to the file anchor alone: {in_gap:?}"
        );
    }
);

rrtest!(
    at_line_with_no_anchor_exits_one,
    |mut dir: Dir, mut cmd: TestCommand| {
        // The whole-file path anchor covers every line *within* the file, so the
        // only uncovered lines are past its end. A one-line file's anchor spans
        // 1-1, making line 2 the tightest provably-uncovered query — robust as the
        // suite evolves, unlike a "large enough" magic line number that silently
        // assumes the fixture never grows that big.
        dir.file("solo.txt", "the only line\n");
        cmd.arg("index").assert_exit_code(0);
        let out = cmd.args(["at", "solo.txt:2"]).run();
        assert_eq!(code(&out), 1, "no covering anchor should exit 1: {out:?}");
        assert!(String::from_utf8_lossy(&out.stderr).contains("no anchor covers"));
    }
);

rrtest!(
    at_malformed_position_exits_two,
    |_dir: Dir, mut cmd: TestCommand| {
        // Position syntax is validated before dispatch, so no index is needed.
        cmd.args(["at", "a.txt"]).assert_exit_code(2);
        cmd.args(["at", "a.txt:nope"]).assert_exit_code(2);
    }
);

rrtest!(
    at_missing_index_exits_three,
    |mut dir: Dir, mut cmd: TestCommand| {
        dir.file("a.txt", "hi\n");
        let out = cmd.args(["at", "a.txt:1"]).run();
        assert_eq!(code(&out), 3, "absent index should exit 3: {out:?}");
        assert!(String::from_utf8_lossy(&out.stderr).contains("run `rr index`"));
    }
);

rrtest!(
    at_stale_index_exits_three,
    |mut dir: Dir, mut cmd: TestCommand| {
        dir.file("a.txt", "v1\n");
        cmd.arg("index").assert_exit_code(0);
        std::thread::sleep(std::time::Duration::from_millis(1100));
        dir.write("a.txt", "v2 changed\n");
        let out = cmd.args(["at", "a.txt:1"]).run();
        assert_eq!(code(&out), 3, "stale index should exit 3: {out:?}");
        assert!(String::from_utf8_lossy(&out.stderr).contains("stale"));
    }
);

rrtest!(at_json_envelope, |mut dir: Dir, mut cmd: TestCommand| {
    dir.file("src/main.rs", "fn main() {\n    let x = 1;\n}\n");
    cmd.arg("index").assert_exit_code(0);
    let out = cmd
        .args(["at", "src/main.rs:1", "--format", "json"])
        .stdout();
    assert!(out.contains(r#""command":"at""#), "{out}");
    assert!(out.contains(r#""found":true"#), "{out}");
    assert!(out.contains(r#""file":"src/main.rs""#), "{out}");
    assert!(out.contains(r#""line":1"#), "{out}");
    assert!(out.contains(r#""start_line":1"#), "{out}");
});

rrtest!(
    at_json_not_found_still_emits_envelope,
    |mut dir: Dir, mut cmd: TestCommand| {
        dir.file("src/main.rs", "fn main() {}\n");
        cmd.arg("index").assert_exit_code(0);
        let out = cmd.args(["at", "src/main.rs:99", "--format", "json"]).run();
        assert_eq!(code(&out), 1, "{out:?}");
        let s = String::from_utf8_lossy(&out.stdout);
        assert!(s.contains(r#""found":false"#), "{s}");
        assert!(s.contains(r#""anchors":[]"#), "{s}");
    }
);

// Dogfood: drive `rr` against its own source tree, exercising the forward lookup
// (`read`) and the reverse lookup (`at`) end to end on real code — specifically
// the `rrtest!` macro in tests/common/mod.rs that generates this very test. We
// index the crate root into a throwaway index file (never the repo's own
// `.ref-cache/`), `read` the macro anchor to its location, then feed that
// location back through `at` and require the anchor to round-trip, with the file
// anchor's span agreeing between the two commands. Line numbers are derived from
// `read`, never hardcoded, so ordinary edits to the harness don't break this.
rrtest!(
    dogfoods_forward_and_reverse_lookup_on_own_source,
    |dir: Dir, _cmd: TestCommand| {
        // The walk runs in the crate root; the index lives outside it (in `dir`),
        // so our own source tree never indexes the cache we are writing.
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
        assert_eq!(
            code(&idx),
            0,
            "indexing our own tree should succeed: {idx:?}"
        );

        // Forward lookup: the whole-file path anchor for the test harness file.
        let file = "tests/common/mod.rs";
        let file_out = run(&["read", file, "--locate"]);
        assert_eq!(code(&file_out), 0, "read path anchor: {file_out:?}");
        let file_loc = String::from_utf8_lossy(&file_out.stdout).trim().to_string();
        assert!(file_loc.starts_with(&format!("{file}:1-")), "{file_loc}");

        // Forward lookup: the `rrtest!` macro definition — the anchor that
        // generates this very test — resolved by name.
        let sym_out = run(&["read", "rrtest", "--locate"]);
        assert_eq!(code(&sym_out), 0, "read macro anchor: {sym_out:?}");
        let sym_loc = String::from_utf8_lossy(&sym_out.stdout).trim().to_string();
        let (sym_file, sym_span) = sym_loc.rsplit_once(':').expect("file:span in read output");
        assert_eq!(
            sym_file, file,
            "rrtest should live in the harness file: {sym_loc}"
        );
        let (sym_start, _) = sym_span
            .split_once('-')
            .expect("start-end span in read output");

        // Reverse lookup: feed the macro's own line back through `at`. It must
        // return the macro anchor (round-trip) plus the file anchor that also
        // covers that line, and the file anchor must match what `read` reported.
        let at_out = run(&["at", &format!("{file}:{sym_start}")]);
        assert_eq!(code(&at_out), 0, "reverse lookup: {at_out:?}");
        let at = String::from_utf8_lossy(&at_out.stdout);
        assert!(
            at.contains(&format!("rrtest\t{sym_loc}")),
            "macro anchor should round-trip through `at`: {at}"
        );
        assert!(
            at.contains(&format!("{file}\t{file_loc}")),
            "file anchor should agree between `read` and `at`: {at}"
        );
    }
);

rrtest!(version_exits_zero, |_dir: Dir, mut cmd: TestCommand| {
    let v = cmd.arg("--version").run();
    assert_eq!(code(&v), 0);
    assert!(String::from_utf8_lossy(&v.stdout).starts_with("rr "));
});

rrtest!(help_exits_zero, |_dir: Dir, mut cmd: TestCommand| {
    let h = cmd.arg("--help").run();
    assert_eq!(code(&h), 0);
    assert!(String::from_utf8_lossy(&h.stdout).contains("USAGE"));
});

rrtest!(
    unknown_subcommand_exits_two,
    |_dir: Dir, mut cmd: TestCommand| {
        cmd.arg("frobnicate").assert_exit_code(2);
    }
);
