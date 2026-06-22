//! End-to-end walking-skeleton tests: `rr index` -> `rr read --locate`, plus the
//! `stat`-only freshness signal (exit 3) and the not-found signal (exit 1).
//!
//! Drives the real `rr` binary in a throwaway `Dir` so the whole pipeline
//! (walk -> serialize -> mmap -> binary-search -> freshness) is exercised
//! together. Scratch-dir setup and teardown live in `common`.

mod common;

use common::{code, Dir, TestCommand};

// Pins the `--locate` output format (`path:start-end`) and the index summary
// format (`indexed N anchors across N files`). Single-line fixture keeps the
// expected `1-1` range unambiguous.
rrtest!(index_then_read_locate_roundtrip, |mut dir: Dir, mut cmd: TestCommand| {
    dir.file("src/main.rs", "fn main() {}\n")
        .file("README.md", "# title\n\nbody\n");

    let idx = cmd.arg("index").run();
    assert_eq!(code(&idx), 0, "index should succeed: {idx:?}");
    let summary = String::from_utf8_lossy(&idx.stdout);
    assert!(
        summary.contains("indexed 2 anchors across 2 files"),
        "{summary}"
    );

    let loc = cmd.args(&["read", "src/main.rs", "--locate"]).stdout();
    assert_eq!(loc.trim(), "src/main.rs:1-1");
});

rrtest!(unknown_anchor_exits_one, |mut dir: Dir, mut cmd: TestCommand| {
    dir.file("a.txt", "hello\n");
    cmd.arg("index").assert_exit_code(0);
    cmd.args(&["read", "does/not/exist.rs", "--locate"]).assert_exit_code(1);
});

rrtest!(stale_index_exits_three, |mut dir: Dir, mut cmd: TestCommand| {
    dir.file("a.txt", "v1\n");
    cmd.arg("index").assert_exit_code(0);

    // The freshness check is second-granular and same-second writes are fresh
    // by design, so wait past the second boundary before modifying the file.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    dir.write("a.txt", "v2 changed\n");

    let out = cmd.args(&["read", "a.txt", "--locate"]).run();
    assert_eq!(code(&out), 3, "stale index should exit 3: {out:?}");
    assert!(String::from_utf8_lossy(&out.stderr).contains("stale"));
});

rrtest!(missing_index_exits_three, |mut dir: Dir, mut cmd: TestCommand| {
    dir.file("a.txt", "hi\n");
    // No `rr index` run first: the reader can't answer, so it asks for a rebuild.
    let out = cmd.args(&["read", "a.txt", "--locate"]).run();
    assert_eq!(code(&out), 3, "absent index should exit 3: {out:?}");
    assert!(String::from_utf8_lossy(&out.stderr).contains("run `rr index`"));
});

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

rrtest!(unknown_subcommand_exits_two, |_dir: Dir, mut cmd: TestCommand| {
    cmd.arg("frobnicate").assert_exit_code(2);
});
