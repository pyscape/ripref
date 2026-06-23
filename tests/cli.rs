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

// `rr at <file>:<line>` — the inverse of `read`. The default answers with the one
// anchor you would cite: the tightest span covering the line. A multi-line fixture
// makes the `main` symbol (`1-1`) genuinely tighter than the whole-file path anchor
// (`1-3`), so the symbol is the default answer and `--all` adds the file around it.
rrtest!(
    at_returns_covering_anchors,
    |mut dir: Dir, mut cmd: TestCommand| {
        dir.file("src/main.rs", "fn main() {\n    let x = 1;\n}\n");
        cmd.arg("index").assert_exit_code(0);

        // Default: the tightest anchor, by name, no line numbers.
        let tightest = cmd.args(["at", "src/main.rs:1"]).stdout();
        assert_eq!(tightest.trim(), "main", "{tightest:?}");

        // `--all`: the whole nest, outermost-first, names only.
        let all = cmd.args(["at", "src/main.rs:1", "--all"]).stdout();
        let lines: Vec<&str> = all.lines().collect();
        assert_eq!(lines, ["src/main.rs", "main"], "{all:?}");
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

        // A real code line: the tightest cover is its symbol.
        let on_code = cmd.args(["at", "sections.rs:1"]).stdout();
        assert_eq!(on_code.trim(), "first", "{on_code:?}");

        // The blank line 4 falls between `first` and `second`: no symbol covers it,
        // so even `--all` lists the file anchor alone.
        let in_gap = cmd.args(["at", "sections.rs:4", "--all"]).stdout();
        assert_eq!(
            in_gap.trim(),
            "sections.rs",
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

        // Reverse lookup: feed the macro's own line back through `at --all`. Text
        // output is anchor identities, one per line (the references you would cite,
        // no line numbers — those live in `--format json`). Both the macro anchor
        // (the round-trip) and the file anchor that also covers the line appear.
        let at_out = run(&["at", &format!("{file}:{sym_start}"), "--all"]);
        assert_eq!(code(&at_out), 0, "reverse lookup: {at_out:?}");
        let at = String::from_utf8_lossy(&at_out.stdout);
        let names: Vec<&str> = at.lines().collect();
        assert!(
            names.contains(&"rrtest"),
            "macro anchor should round-trip through `at`: {at}"
        );
        assert!(
            names.contains(&file),
            "file anchor should also cover the macro's line: {at}"
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

// `--no-freshness` answers from the index without the staleness check. The same
// edit that makes a plain `read` exit 3 (stale) must still resolve under the
// flag — and the plain read must keep exiting 3, proving the flag, not a
// changed default, is what skips the gate.
rrtest!(
    no_freshness_skips_stale_check,
    |mut dir: Dir, mut cmd: TestCommand| {
        dir.file("a.txt", "v1\n");
        cmd.arg("index").assert_exit_code(0);

        // mtimes are whole seconds; cross the boundary before editing so the
        // stat-walk sees a newer file. (Mirrors `stale_index_exits_three`.)
        std::thread::sleep(std::time::Duration::from_millis(1100));
        dir.write("a.txt", "v2 changed\n");

        // Existing behavior preserved: the unflagged read still reports stale.
        cmd.args(["read", "a.txt", "--locate"]).assert_exit_code(3);
        // The flag answers anyway.
        cmd.args(["read", "a.txt", "--locate", "--no-freshness"])
            .assert_exit_code(0);
    }
);

// The git-tree short-circuit serves a clean checkout whose HEAD matches the
// stamp without stat-ing: a content-identical rewrite bumps the mtime (so the
// stat-walk alone would cry stale) but leaves the tree clean, so exit 0 proves
// the short-circuit fired. Dirtying the tree must then fall through to a real
// stale verdict (exit 3) — the short-circuit must not mask genuine staleness.
rrtest!(
    git_clean_tree_short_circuits_freshness,
    |mut dir: Dir, mut cmd: TestCommand| {
        if !git_available() {
            eprintln!("skipping: git not available");
            return;
        }
        dir.file("a.txt", "hello\n");
        // The index lives in the working tree; gitignore it (as a real repo
        // would) so `rr index` does not dirty the tree it just stamped clean.
        dir.file(".gitignore", ".ref-cache/\n");
        git(&dir, &["init", "-q"]);
        git(&dir, &["config", "user.email", "t@example.com"]);
        git(&dir, &["config", "user.name", "t"]);
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-q", "-m", "init"]);

        // Stamp a clean `tree` into the index.
        cmd.arg("index").assert_exit_code(0);

        // Cross the second boundary, then rewrite with identical content: mtime
        // bumps, tree stays clean.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        dir.write("a.txt", "hello\n");
        cmd.args(["read", "a.txt", "--locate"]).assert_exit_code(0);

        // Now actually change the content: the tree is dirty, the stamp no
        // longer matches, and the stat-walk sees a newer file -> stale.
        dir.write("a.txt", "changed\n");
        cmd.args(["read", "a.txt", "--locate"]).assert_exit_code(3);
    }
);

/// Whether a `git` binary is on PATH; the short-circuit test needs a real repo.
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
