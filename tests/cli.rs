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

// --- AD-1 citation markers: emit (`--cite`) and accept (readers strip) --------

// `rr at --cite` emits the document citation marker instead of the bare address;
// the bare form stays the default so `rr read "$(rr at f:l)"` keeps working.
rrtest!(
    at_cite_emits_marker,
    |mut dir: Dir, mut cmd: TestCommand| {
        dir.file("src/main.rs", "fn main() {\n    let x = 1;\n}\n");
        cmd.arg("index").assert_exit_code(0);

        let bare = cmd.args(["at", "src/main.rs:1"]).stdout();
        assert_eq!(bare.trim(), "main", "default stays bare: {bare:?}");

        let cited = cmd.args(["at", "src/main.rs:1", "--cite"]).stdout();
        assert_eq!(cited.trim(), "[[rr:main]]", "{cited:?}");

        let all = cmd
            .args(["at", "src/main.rs:1", "--all", "--cite"])
            .stdout();
        assert_eq!(
            all.lines().collect::<Vec<_>>(),
            ["[[rr:src/main.rs]]", "[[rr:main]]"],
            "{all:?}"
        );
    }
);

// The JSON envelope always carries the pre-composed `citation` marker beside the
// bare `anchor`, at envelope version 1 (an additive field, not a breaking bump).
rrtest!(
    at_json_carries_citation_field,
    |mut dir: Dir, mut cmd: TestCommand| {
        dir.file("src/main.rs", "fn main() {\n    let x = 1;\n}\n");
        cmd.arg("index").assert_exit_code(0);
        let out = cmd
            .args(["at", "src/main.rs:1", "--format", "json"])
            .stdout();
        assert!(out.contains(r#""version":1"#), "envelope stays v1: {out}");
        assert!(out.contains(r#""anchor":"main""#), "{out}");
        assert!(out.contains(r#""citation":"[[rr:main]]""#), "{out}");
    }
);

// A reader strips a pasted `[[rr:...]]` marker before resolving, so a copied
// citation works as a CLI argument (single-quoted in a shell because `[` globs).
rrtest!(
    read_strips_pasted_marker,
    |mut dir: Dir, mut cmd: TestCommand| {
        dir.file("src/main.rs", "fn main() {\n    let x = 1;\n}\n");
        cmd.arg("index").assert_exit_code(0);
        let loc = cmd.args(["read", "[[rr:main]]", "--locate"]).stdout();
        assert_eq!(loc.trim(), "src/main.rs:1-1", "{loc:?}");
    }
);

// A `[[rr:` token that is not a well-formed citation (here, a non-hex pin) is a
// usage error (exit 2), never a silent fall-through to bare parsing. Rejected up
// front, so no index is consulted.
rrtest!(
    read_malformed_marker_exits_two,
    |_dir: Dir, mut cmd: TestCommand| {
        let out = cmd.args(["read", "[[rr:a]]@zzz"]).run();
        assert_eq!(code(&out), 2, "malformed marker is a usage error: {out:?}");
    }
);

// A marker pin that resolves to no snapshot is BROKEN (exit 5). With a fresh
// index this matches the bare path too; the offline-dispatch distinction is
// proven separately under a stale index by the test below.
rrtest!(
    read_broken_marker_pin_exits_five,
    |mut dir: Dir, mut cmd: TestCommand| {
        dir.file("src/main.rs", "fn main() {\n    let x = 1;\n}\n");
        cmd.arg("index").assert_exit_code(0);
        let out = cmd.args(["read", "[[rr:main]]@deadbeef"]).run();
        assert_eq!(code(&out), 5, "missing marker pin is BROKEN: {out:?}");
    }
);

// THE offline-dispatch regression guard. Under a STALE index the two forms must
// diverge: a marker pin resolves offline (no index consulted) so a missing pin is
// honestly BROKEN (5), while the bare `anchor@commit` form reports STALE (3)
// because a stale index cannot confirm whether the whole token is a live anchor.
// The v1 bug (re-dispatching markers through run_read_pinned) would make the
// marker also report STALE here — this pair is what catches it.
rrtest!(
    marker_pin_is_offline_even_when_index_is_stale,
    |mut dir: Dir, mut cmd: TestCommand| {
        dir.file("a.txt", "v1\n");
        cmd.arg("index").assert_exit_code(0);
        std::thread::sleep(std::time::Duration::from_millis(1100));
        dir.write("a.txt", "v2 changed\n"); // index is now provably stale

        let marker = cmd.args(["read", "[[rr:a.txt]]@deadbeef"]).run();
        assert_eq!(
            code(&marker),
            5,
            "marker pin resolves offline -> BROKEN even when stale: {marker:?}"
        );

        let bare = cmd.args(["read", "a.txt@deadbeef"]).run();
        assert_eq!(
            code(&bare),
            3,
            "bare pin under a stale index -> STALE: {bare:?}"
        );
    }
);

// The BARE `anchor@<short>` form still resolves a snapshot through the
// known-anchor-wins-then-pin path (run_read_pinned), now that producers emit
// markers. Guards the unchanged bare-token path (v2 plan Constraints).
rrtest!(
    bare_pinned_read_still_resolves_snapshot,
    |mut dir: Dir, mut cmd: TestCommand| {
        if !git_available() {
            eprintln!("skipping: git not available");
            return;
        }
        dir.file("doc.md", "# Title\n\nbody\n");
        init_repo(&dir);
        commit_all(&dir, "init");
        cmd.arg("index").assert_exit_code(0);

        let marker = cmd.args(["cite", "doc.md"]).stdout().trim().to_string();
        let short = marker.rsplit('@').next().unwrap();
        let bare = format!("doc.md@{short}");

        let out = cmd.args(["read", &bare]).run();
        assert_eq!(code(&out), 0, "bare anchor@short snapshot read: {out:?}");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("doc.md@") && stdout.contains("# Title"),
            "frozen snapshot via the bare pinned form: {stdout:?}"
        );
    }
);

// `require_pin` decodes a pasted pinned marker for verify / uncite / untrack: a
// real `[[rr:anchor]]@<short>` from `rr cite` round-trips through verify and
// uncite, a cross-kind marker is rejected (exit 2), and a malformed marker is a
// usage error (exit 2).
rrtest!(
    pinned_marker_round_trips_through_verify_and_tomb,
    |mut dir: Dir, mut cmd: TestCommand| {
        if !git_available() {
            eprintln!("skipping: git not available");
            return;
        }
        dir.file("src/main.rs", "fn main() {}\n");
        init_repo(&dir);
        commit_all(&dir, "init");
        cmd.arg("index").assert_exit_code(0);

        let marker = cmd
            .args(["cite", "src/main.rs"])
            .stdout()
            .trim()
            .to_string();
        assert!(marker.starts_with("[[rr:src/main.rs]]@"), "{marker:?}");

        // verify accepts the pasted marker.
        let v = cmd.args(["verify", &marker]).run();
        assert_eq!(code(&v), 0, "verify of a pasted marker: {v:?}");

        // Cross-kind: untrack of a snapshot (`@`) marker is a usage error.
        let xk = cmd.args(["untrack", &marker]).run();
        assert_eq!(code(&xk), 2, "untrack of an @ marker is cross-kind: {xk:?}");

        // A malformed marker is a usage error, not a silent miss.
        let bad = cmd.args(["uncite", "[[rr:src/main.rs]]@zzz"]).run();
        assert_eq!(code(&bad), 2, "malformed marker to uncite: {bad:?}");

        // uncite retires the snapshot via the pasted marker.
        let u = cmd.args(["uncite", &marker]).run();
        assert_eq!(code(&u), 0, "uncite of a pasted marker: {u:?}");
    }
);

// `rr cite` accepts a pasted bare `[[rr:anchor]]` (stripping it to the anchor) and
// emits the same pinned marker as the bare form; a pinned or malformed marker is
// a usage error.
rrtest!(
    cite_accepts_bare_marker_rejects_pinned,
    |mut dir: Dir, mut cmd: TestCommand| {
        if !git_available() {
            eprintln!("skipping: git not available");
            return;
        }
        dir.file("src/main.rs", "fn main() {}\n");
        init_repo(&dir);
        commit_all(&dir, "init");
        cmd.arg("index").assert_exit_code(0);

        let from_bare = cmd
            .args(["cite", "src/main.rs"])
            .stdout()
            .trim()
            .to_string();
        let from_marker = cmd
            .args(["cite", "[[rr:src/main.rs]]"])
            .stdout()
            .trim()
            .to_string();
        assert_eq!(
            from_bare, from_marker,
            "bare and [[rr:...]] cite the same anchor"
        );

        // A pinned marker is nonsensical for cite -> usage error.
        let pinned = cmd.args(["cite", "[[rr:src/main.rs]]@a1b2c3d"]).run();
        assert_eq!(code(&pinned), 2, "cite of a pinned citation: {pinned:?}");
        assert!(
            String::from_utf8_lossy(&pinned.stderr).contains("pinned citation"),
            "{pinned:?}"
        );

        // A malformed marker is a usage error.
        let bad = cmd.args(["cite", "[[rr:a]]@zzz"]).run();
        assert_eq!(code(&bad), 2, "malformed marker to cite: {bad:?}");
    }
);

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

// --- Phase 1: writer/reader hardening ---------------------------------------

/// The default index path within a test [`Dir`].
fn index_file(dir: &Dir) -> std::path::PathBuf {
    dir.path().join(".ref-cache").join("index")
}

/// The bytes of an index after its header (everything from the blank line that
/// terminates the section table onward): the `forward`/`reverse`/`paths` bodies,
/// with the build-time `mtime` stamp in the header excluded.
fn index_body(dir: &Dir) -> Vec<u8> {
    let bytes = std::fs::read(index_file(dir)).expect("index file exists");
    let header_end = bytes
        .windows(2)
        .position(|w| w == b"\n\n")
        .map(|p| p + 2)
        .expect("index header ends in a blank line");
    bytes[header_end..].to_vec()
}

// The on-disk format is unchanged by the snapshot/tracking feature: the magic is
// still `refidx v1` and the header carries no content hash / blob OID / pin
// field. Locks "no format change" so the index stays a pure locations file.
rrtest!(
    index_is_refidx_v1_after_feature,
    |mut dir: Dir, mut cmd: TestCommand| {
        dir.file("src/main.rs", "fn main() {}\n")
            .file("README.md", "# title\n\nbody\n");
        cmd.arg("index").assert_exit_code(0);

        let bytes = std::fs::read(index_file(&dir)).unwrap();
        let header_end = bytes.windows(2).position(|w| w == b"\n\n").unwrap();
        let header = String::from_utf8(bytes[..header_end].to_vec()).unwrap();

        assert!(header.starts_with("refidx v1\n"), "magic line: {header:?}");
        for section in ["section:forward:", "section:reverse:", "section:paths:"] {
            assert!(header.contains(section), "missing {section} in {header:?}");
        }
        // No content-addressing leaked into the index format.
        for forbidden in ["blob", "oid", "pin", "sha", "snapshot", "track"] {
            assert!(
                !header.contains(forbidden),
                "header must not carry a {forbidden:?} field: {header:?}"
            );
        }
    }
);

// Indexing the same tree twice yields a byte-identical body. The parallel walk
// hands records back in a scheduling-dependent order, so this only holds because
// serialize sorts to a total order; a non-total sort (anchor only, with colliding
// anchors) would let the two builds differ. Stamp (mtime) aside, the bodies match.
rrtest!(
    index_unchanged_tree_is_byte_stable,
    |mut dir: Dir, mut cmd: TestCommand| {
        // Several files, several anchors each, so the parallel walk has real
        // freedom to reorder between the two builds.
        for i in 0..12 {
            dir.file(
                &format!("src/mod{i}.rs"),
                &format!("pub fn f{i}() {{}}\npub struct S{i};\n"),
            );
        }
        dir.file("a.md", "# Alpha\n\n## Beta\n");

        cmd.arg("index").assert_exit_code(0);
        let first = index_body(&dir);
        cmd.arg("index").assert_exit_code(0);
        let second = index_body(&dir);

        assert_eq!(
            first, second,
            "index body must be deterministic across builds"
        );
        assert!(
            !first.is_empty(),
            "body should carry the forward/paths records"
        );
    }
);

// A truncated index whose header still parses but whose section bytes are gone
// must be reported as a corrupt index, never crash with an out-of-bounds panic
// (the historical refidx slicing bug). Reads the built index, cuts it to the end
// of the header, and confirms a clean usage error (exit 2), not a panic (101).
rrtest!(
    truncated_index_is_corrupt_not_panic,
    |mut dir: Dir, mut cmd: TestCommand| {
        dir.file("a.txt", "one\ntwo\nthree\n");
        cmd.arg("index").assert_exit_code(0);

        let path = index_file(&dir);
        let bytes = std::fs::read(&path).unwrap();
        let header_end = bytes
            .windows(2)
            .position(|w| w == b"\n\n")
            .map(|p| p + 2)
            .unwrap();
        assert!(
            header_end < bytes.len(),
            "the built index has a non-empty body"
        );
        std::fs::write(&path, &bytes[..header_end]).unwrap();

        let out = cmd
            .args(["read", "a.txt", "--locate", "--no-freshness"])
            .run();
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

// --- Phase 2: cite / track producers + .rr/ sidecar -------------------------

// `rr cite` writes the committed sidecar (`.rr/objects/<oid>` + a `.rr/refs`
// snapshot line) and leaves the index untouched; the stored object re-hashes to
// the recorded oid (self-verifying, git-blob addressed).
rrtest!(
    cite_writes_sidecar_not_index,
    |mut dir: Dir, mut cmd: TestCommand| {
        if !git_available() {
            eprintln!("skipping: git not available");
            return;
        }
        dir.file("src/main.rs", "fn main() {}\n");
        init_repo(&dir);
        commit_all(&dir, "init");
        cmd.arg("index").assert_exit_code(0);

        let index_before = std::fs::read(index_file(&dir)).unwrap();

        let out = cmd.args(["cite", "src/main.rs"]).stdout();
        assert!(
            out.trim().starts_with("[[rr:src/main.rs]]@"),
            "cite prints the citation marker [[rr:anchor]]@<short>: {out:?}"
        );

        // The manifest records the snapshot, and the object exists + round-trips.
        let fields = pin_fields(&dir, "snapshot");
        assert_eq!(fields[1], "src/main.rs", "anchor field");
        assert_eq!(fields[3], "src/main.rs", "path-at-pin field");
        assert_eq!(fields[4], "1-1", "span field");
        let oid = &fields[5];
        let obj = object_path(&dir, oid);
        assert!(obj.exists(), "object {obj:?} should exist");
        assert_eq!(std::fs::read(&obj).unwrap(), b"fn main() {}\n");
        assert_eq!(
            git_hash_object(&dir, &obj),
            *oid,
            "stored object must re-hash to its recorded oid"
        );

        // cite never touches the index.
        let index_after = std::fs::read(index_file(&dir)).unwrap();
        assert_eq!(index_before, index_after, "cite must not modify the index");
    }
);

// An anchor whose file is not committed as-is cannot be cited (exit 2): there is
// no durable commit for the frozen evidence to correspond to.
rrtest!(
    cite_refuses_uncommitted_file,
    |mut dir: Dir, mut cmd: TestCommand| {
        if !git_available() {
            eprintln!("skipping: git not available");
            return;
        }
        init_repo(&dir);
        commit_all(&dir, "init"); // commit .gitignore so HEAD exists
        dir.file("new.txt", "uncommitted\n"); // untracked, never committed
        cmd.arg("index").assert_exit_code(0);

        let out = cmd.args(["cite", "new.txt"]).run();
        assert_eq!(code(&out), 2, "uncommitted cite should exit 2: {out:?}");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("not committed"),
            "stderr: {:?}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            !manifest(&dir).contains("new.txt"),
            "a refused cite writes no pin"
        );
    }
);

// The commit gate is content-based (`git hash-object` vs `HEAD:<path>`), so it
// catches an edit hidden from `git status` by `--skip-worktree`. Locks GITGATE-1.
rrtest!(
    cite_gate_is_content_based_under_skip_worktree,
    |mut dir: Dir, mut cmd: TestCommand| {
        if !git_available() {
            eprintln!("skipping: git not available");
            return;
        }
        dir.file("f.txt", "good\n");
        init_repo(&dir);
        commit_all(&dir, "init");
        cmd.arg("index").assert_exit_code(0); // stamps a clean tree

        // Weaken the file, then hide it from git's index view. `git status` now
        // reports clean (so freshness short-circuits), but the content differs.
        dir.write("f.txt", "weakened!\n");
        git(&dir, &["update-index", "--skip-worktree", "f.txt"]);

        let out = cmd.args(["cite", "f.txt"]).run();
        assert_eq!(
            code(&out),
            2,
            "a skip-worktree edit must still be refused by the content gate: {out:?}"
        );
        assert!(String::from_utf8_lossy(&out.stderr).contains("not committed"));
    }
);

// A clean-filter / Git-LFS path is refused (exit 2): LFS content is off-repo and
// a clean filter's pre-clean bytes can re-leak secrets, so neither is safe to
// freeze verbatim. Locks N2-LFS / N2-CLEAN-FILTER.
rrtest!(
    cite_refuses_lfs_or_clean_filter_path,
    |mut dir: Dir, mut cmd: TestCommand| {
        if !git_available() {
            eprintln!("skipping: git not available");
            return;
        }
        dir.file(".gitattributes", "secret.txt filter=lfs\n");
        dir.file("secret.txt", "data\n");
        init_repo(&dir);
        commit_all(&dir, "init");
        cmd.arg("index").assert_exit_code(0);

        let out = cmd.args(["cite", "secret.txt"]).run();
        assert_eq!(
            code(&out),
            2,
            "an LFS / clean-filter path must be refused: {out:?}"
        );
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("filter")
                || String::from_utf8_lossy(&out.stderr).contains("LFS"),
            "stderr: {:?}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            !manifest(&dir).contains("secret.txt"),
            "a refused cite writes no pin"
        );
    }
);

// --- Phase 3: grammar / known-anchor-wins -----------------------------------

// An anchor that legitimately contains `@` (an email heading) reads literally:
// known-anchor-wins resolves the whole token before any `@` split. Green today.
rrtest!(
    email_anchor_reads_literally,
    |mut dir: Dir, mut cmd: TestCommand| {
        dir.file("README.md", "## support@example.com\n\nbody\n");
        cmd.arg("index").assert_exit_code(0);
        let out = cmd.args(["read", "support@example.com", "--locate"]).run();
        assert_eq!(code(&out), 0, "email anchor should read literally: {out:?}");
        assert!(
            String::from_utf8_lossy(&out.stdout).contains("README.md:1-1"),
            "stdout: {:?}",
            String::from_utf8_lossy(&out.stdout)
        );
    }
);

// A path anchor containing `@scope` reads literally, too (the `@` is not a pin).
rrtest!(
    scope_path_anchor_reads_literally,
    |mut dir: Dir, mut cmd: TestCommand| {
        dir.file("node_modules/@scope/p.js", "export const x = 1;\n");
        cmd.arg("index").assert_exit_code(0);
        let out = cmd
            .args(["read", "node_modules/@scope/p.js", "--locate"])
            .run();
        assert_eq!(code(&out), 0, "@scope path should read literally: {out:?}");
        assert!(String::from_utf8_lossy(&out.stdout).contains("node_modules/@scope/p.js:1-1"));
    }
);

// `cite` emits a pinned citation marker `[[rr:support@example.com]]@<short>`.
// Reading it back resolves the *snapshot* offline: the marker delimits the
// anchor and the pin sits outside `]]`, so known-anchor-wins never fires even
// though `support@example.com` is itself a live anchor. This is AD-1's payoff,
// and the regression guard against routing a marker pin through the bare path.
rrtest!(
    email_anchor_snapshot_resolves_offline_from_marker,
    |mut dir: Dir, mut cmd: TestCommand| {
        if !git_available() {
            eprintln!("skipping: git not available");
            return;
        }
        dir.file("README.md", "## support@example.com\n\nbody\n");
        init_repo(&dir);
        commit_all(&dir, "init");
        cmd.arg("index").assert_exit_code(0);

        let pin_ref = cmd.args(["cite", "support@example.com"]).stdout();
        let pin_ref = pin_ref.trim();
        assert!(
            pin_ref.starts_with("[[rr:support@example.com]]@"),
            "cite emits the pinned marker: {pin_ref:?}"
        );

        let out = cmd.args(["read", pin_ref]).run();
        assert_eq!(code(&out), 0, "snapshot of the email anchor: {out:?}");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("README.md@") && stdout.contains("## support@example.com"),
            "snapshot output should be the frozen email-heading line: {stdout:?}"
        );
    }
);

// A pinned reference whose commit cannot be resolved is BROKEN (exit 5), not a
// plain not-found: the token names a pin, but no such pin exists.
rrtest!(
    missing_or_ambiguous_commit_is_broken,
    |mut dir: Dir, mut cmd: TestCommand| {
        dir.file("README.md", "# Title\n");
        cmd.arg("index").assert_exit_code(0);
        let out = cmd.args(["read", "README.md@zzzzzzz"]).run();
        assert_eq!(code(&out), 5, "unresolvable pin should be BROKEN: {out:?}");
        assert!(String::from_utf8_lossy(&out.stderr).contains("broken reference"));
    }
);

// --- Phase 3: snapshot (cite + read @) --------------------------------------

// After citing then rewriting + committing, `rr read anchor@<short>` prints the
// OLD frozen source from `.rr/objects`, under a `path@<short>:<range>` header.
rrtest!(
    snapshot_read_prints_frozen_source,
    |mut dir: Dir, mut cmd: TestCommand| {
        if !git_available() {
            eprintln!("skipping: git not available");
            return;
        }
        dir.file("doc.md", "# Title\n\noriginal line\n");
        init_repo(&dir);
        commit_all(&dir, "init");
        cmd.arg("index").assert_exit_code(0);

        let pin_ref = cmd.args(["cite", "doc.md"]).stdout().trim().to_string();

        // Rewrite and commit, so the live content diverges from the snapshot.
        dir.write("doc.md", "# Title\n\nREWRITTEN\n");
        commit_all(&dir, "rewrite");

        let out = cmd.args(["read", &pin_ref]).run();
        assert_eq!(code(&out), 0, "snapshot read should succeed: {out:?}");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("original line"),
            "frozen source: {stdout:?}"
        );
        assert!(
            !stdout.contains("REWRITTEN"),
            "must not show live content: {stdout:?}"
        );
        assert!(stdout.contains("doc.md@"), "header present: {stdout:?}");
    }
);

// A snapshot recovers even after its commit is pruned from git (rewrite history,
// expire reflog, gc). The recovery reads `.rr/objects`, never git. Locks GC-independence.
rrtest!(
    snapshot_recovers_after_commit_pruned,
    |mut dir: Dir, mut cmd: TestCommand| {
        if !git_available() {
            eprintln!("skipping: git not available");
            return;
        }
        dir.file("doc.md", "frozen evidence\n");
        init_repo(&dir);
        commit_all(&dir, "init");
        cmd.arg("index").assert_exit_code(0);

        let pin_ref = cmd.args(["cite", "doc.md"]).stdout().trim().to_string();
        let short = pin_ref.rsplit('@').next().unwrap().to_string();

        // Orphan the cited commit and prune it from the object store.
        dir.write("doc.md", "different now\n");
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-q", "--amend", "-m", "rewritten"]);
        git(
            &dir,
            &["reflog", "expire", "--expire-unreachable=now", "--all"],
        );
        git(&dir, &["gc", "-q", "--prune=now"]);

        // The original commit is genuinely gone from git...
        let gone = Command::new("git")
            .args(["cat-file", "-e", &short])
            .current_dir(dir.path())
            .output()
            .unwrap();
        assert!(!gone.status.success(), "commit {short} should be pruned");

        // ...yet the snapshot still recovers the frozen bytes.
        let out = cmd.args(["read", &pin_ref]).run();
        assert_eq!(
            code(&out),
            0,
            "pruned-commit snapshot must still read: {out:?}"
        );
        assert!(String::from_utf8_lossy(&out.stdout).contains("frozen evidence"));
    }
);

// A snapshot survives a rename of its file: recovery reads the stored bytes, so
// `git mv` of the cited path does not break it. Locks GIT-1.
rrtest!(
    snapshot_survives_rename,
    |mut dir: Dir, mut cmd: TestCommand| {
        if !git_available() {
            eprintln!("skipping: git not available");
            return;
        }
        dir.file("old.md", "frozen\n");
        init_repo(&dir);
        commit_all(&dir, "init");
        cmd.arg("index").assert_exit_code(0);

        let pin_ref = cmd.args(["cite", "old.md"]).stdout().trim().to_string();

        git(&dir, &["mv", "old.md", "new.md"]);
        commit_all(&dir, "rename");

        let out = cmd.args(["read", &pin_ref]).run();
        assert_eq!(
            code(&out),
            0,
            "renamed-file snapshot must still read: {out:?}"
        );
        assert!(String::from_utf8_lossy(&out.stdout).contains("frozen"));
    }
);

// CRLF is recovered faithfully: a `-text` file stored with CRLF comes back as
// CRLF (the working-tree form), not normalized to LF. Locks N2-EOL.
rrtest!(
    snapshot_recovers_crlf_faithfully,
    |mut dir: Dir, mut cmd: TestCommand| {
        if !git_available() {
            eprintln!("skipping: git not available");
            return;
        }
        dir.file(".gitattributes", "crlf.txt -text\n");
        dir.write("crlf.txt", "alpha\r\nbeta\r\ngamma\r\n");
        init_repo(&dir);
        commit_all(&dir, "init");
        cmd.arg("index").assert_exit_code(0);

        let pin_ref = cmd.args(["cite", "crlf.txt"]).stdout().trim().to_string();
        let out = cmd.args(["read", &pin_ref]).run();
        assert_eq!(code(&out), 0, "crlf snapshot read: {out:?}");
        // The recovered body preserves CRLF terminators verbatim.
        assert!(
            out.stdout.windows(2).any(|w| w == b"\r\n"),
            "CRLF must be preserved, got: {:?}",
            String::from_utf8_lossy(&out.stdout)
        );
        assert!(String::from_utf8_lossy(&out.stdout).contains("alpha"));
    }
);

// --- Phase 3: tracking (track + read ~) -------------------------------------

// A tracked anchor whose file is unchanged reads OK (exit 0).
rrtest!(
    tracking_clean_is_ok,
    |mut dir: Dir, mut cmd: TestCommand| {
        if !git_available() {
            eprintln!("skipping: git not available");
            return;
        }
        dir.file("t.txt", "baseline\n");
        init_repo(&dir);
        commit_all(&dir, "init");
        cmd.arg("index").assert_exit_code(0);

        let pin_ref = cmd.args(["track", "t.txt"]).stdout().trim().to_string();
        assert!(pin_ref.starts_with("[[rr:t.txt]]~"), "{pin_ref:?}");
        let out = cmd.args(["read", &pin_ref]).run();
        assert_eq!(code(&out), 0, "clean tracked ref is OK: {out:?}");
    }
);

// A tracked anchor whose content changed reads DRIFTED (exit 4).
rrtest!(
    tracking_drift_exits_4,
    |mut dir: Dir, mut cmd: TestCommand| {
        if !git_available() {
            eprintln!("skipping: git not available");
            return;
        }
        dir.file("t.txt", "baseline\n");
        init_repo(&dir);
        commit_all(&dir, "init");
        cmd.arg("index").assert_exit_code(0);
        let pin_ref = cmd.args(["track", "t.txt"]).stdout().trim().to_string();

        dir.write("t.txt", "changed substantially\n");
        let out = cmd.args(["read", &pin_ref]).run();
        assert_eq!(code(&out), 4, "drifted tracked ref exits 4: {out:?}");
        assert!(String::from_utf8_lossy(&out.stderr).contains("DRIFTED"));
    }
);

// Drift is content-based, so a same-length, same-second edit is still caught
// (exit 4) where a size/mtime or `git diff` check would miss it. Locks GIT-2/CI-3.
rrtest!(
    tracking_detects_same_size_same_second_edit,
    |mut dir: Dir, mut cmd: TestCommand| {
        if !git_available() {
            eprintln!("skipping: git not available");
            return;
        }
        dir.file("t.txt", "baseline\n");
        init_repo(&dir);
        commit_all(&dir, "init");
        cmd.arg("index").assert_exit_code(0);
        let pin_ref = cmd.args(["track", "t.txt"]).stdout().trim().to_string();

        // Same byte length (9), no sleep (same second): only a content hash sees it.
        dir.write("t.txt", "baseLine\n");
        let out = cmd.args(["read", &pin_ref]).run();
        assert_eq!(
            code(&out),
            4,
            "same-size same-second edit must drift: {out:?}"
        );
    }
);

// Drift is read from the working tree, so an edit hidden by `--skip-worktree` is
// still caught (exit 4). Locks GITGATE-1 on the tracking side.
rrtest!(
    tracking_detects_skip_worktree_edit,
    |mut dir: Dir, mut cmd: TestCommand| {
        if !git_available() {
            eprintln!("skipping: git not available");
            return;
        }
        dir.file("t.txt", "baseline\n");
        init_repo(&dir);
        commit_all(&dir, "init");
        cmd.arg("index").assert_exit_code(0);
        let pin_ref = cmd.args(["track", "t.txt"]).stdout().trim().to_string();

        dir.write("t.txt", "secretly changed\n");
        git(&dir, &["update-index", "--skip-worktree", "t.txt"]);
        let out = cmd.args(["read", &pin_ref]).run();
        assert_eq!(
            code(&out),
            4,
            "skip-worktree edit must still drift: {out:?}"
        );
    }
);

// A renamed-but-identical file is moved, not drifted: tracking re-finds it by
// content. Locks GIT-1 on the tracking side.
rrtest!(
    tracking_rename_unchanged_is_moved_not_drift,
    |mut dir: Dir, mut cmd: TestCommand| {
        if !git_available() {
            eprintln!("skipping: git not available");
            return;
        }
        dir.file("old.txt", "stable content\n");
        init_repo(&dir);
        commit_all(&dir, "init");
        cmd.arg("index").assert_exit_code(0);
        let pin_ref = cmd.args(["track", "old.txt"]).stdout().trim().to_string();

        git(&dir, &["mv", "old.txt", "new.txt"]);
        commit_all(&dir, "rename");
        let out = cmd.args(["read", &pin_ref]).run();
        assert_eq!(
            code(&out),
            0,
            "rename of identical content is OK (moved): {out:?}"
        );
        assert!(String::from_utf8_lossy(&out.stdout).contains("moved"));
    }
);

// --- Phase 4: verify + durability -------------------------------------------

// A committed manifest line silently removed (no tomb) is tampering: `rr verify`
// cross-checks the working `.rr/refs` against its committed form and fails
// closed. Locks sec-LEDGER-1.
rrtest!(
    verify_fails_closed_on_manifest_deletion,
    |mut dir: Dir, mut cmd: TestCommand| {
        if !git_available() {
            eprintln!("skipping: git not available");
            return;
        }
        dir.file("src/main.rs", "fn main() {}\n");
        init_repo(&dir);
        commit_all(&dir, "init");
        cmd.arg("index").assert_exit_code(0);
        cmd.args(["cite", "src/main.rs"]).assert_exit_code(0);

        // Commit the sidecar, then hand-delete the snapshot line from the working
        // copy without adding a tomb.
        commit_all(&dir, "add sidecar");
        delete_manifest_lines(&dir, "snapshot\t");

        let out = cmd.arg("verify").run();
        assert!(
            !out.status.success(),
            "a silent manifest deletion must fail verify: {out:?}"
        );
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("tampered"),
            "stderr: {:?}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
);

// `rr verify` is manifest-scoped, not a prose scanner: pin-like text in the
// README is never treated as a reference, so a repo whose README shows example
// refs still verifies clean (the real pin is OK).
rrtest!(
    verify_over_own_readme_exits_zero,
    |mut dir: Dir, mut cmd: TestCommand| {
        if !git_available() {
            eprintln!("skipping: git not available");
            return;
        }
        dir.file("src/main.rs", "fn main() {}\n");
        // Prose that *looks* like a broken pin but is just documentation.
        dir.file(
            "README.md",
            "Example: read `src/main.rs@deadbee` (this is prose, not a real pin).\n",
        );
        init_repo(&dir);
        commit_all(&dir, "init");
        cmd.arg("index").assert_exit_code(0);
        cmd.args(["cite", "src/main.rs"]).assert_exit_code(0);

        // The real pin is OK and the README example is ignored.
        cmd.arg("verify").assert_exit_code(0);
    }
);

// `rr verify` over K refs issues O(refs) git calls (one content hash per tracked
// ref plus the tamper-check), not a fan-out of forks per ref. Counted with
// GIT_TRACE2. Locks PERF/NOVEL4.
rrtest!(
    verify_git_calls_are_bounded,
    |mut dir: Dir, mut cmd: TestCommand| {
        if !git_available() {
            eprintln!("skipping: git not available");
            return;
        }
        const K: usize = 4;
        for i in 0..K {
            dir.file(&format!("f{i}.txt"), &format!("content {i}\n"));
        }
        init_repo(&dir);
        commit_all(&dir, "init");
        cmd.arg("index").assert_exit_code(0);
        for i in 0..K {
            cmd.args(["track", &format!("f{i}.txt")])
                .assert_exit_code(0);
        }

        let (out, git_calls) = run_rr_counting_git(&dir, &["verify"]);
        assert_eq!(code(&out), 0, "all tracked refs clean: {out:?}");
        // One hash per tracked ref, plus a single tamper-check git call; certainly
        // not several forks per ref.
        assert!(
            (K..=K + 2).contains(&git_calls),
            "expected ~{K} git calls (one per tracked ref), got {git_calls}"
        );
    }
);

// --- Phase 5: exit codes + truthfulness -------------------------------------

// The four non-trivial outcomes are each reachable and use distinct codes:
// 1 not-found, 3 stale, 4 drifted, 5 broken.
rrtest!(
    exit_codes_are_distinct,
    |mut dir: Dir, mut cmd: TestCommand| {
        if !git_available() {
            eprintln!("skipping: git not available");
            return;
        }
        dir.file("a.txt", "baseline\n");
        init_repo(&dir);
        commit_all(&dir, "init");
        cmd.arg("index").assert_exit_code(0);

        // 1: a plain unknown anchor (fresh index).
        let c1 = code(&cmd.args(["read", "nope-no-such-anchor"]).run());
        // 5: a pinned reference whose commit cannot be resolved (fresh index).
        let c5 = code(&cmd.args(["read", "a.txt@zzzzzzz"]).run());

        // 4: track, then edit the file -> drift (works even as the index goes stale).
        let pin = cmd.args(["track", "a.txt"]).stdout().trim().to_string();

        // Cross the second boundary so the edit also makes a plain read stale.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        dir.write("a.txt", "changed\n");
        let c4 = code(&cmd.args(["read", &pin]).run());
        // 3: a plain live read of the now-stale index.
        let c3 = code(&cmd.args(["read", "a.txt"]).run());

        let mut codes = [c1, c3, c4, c5];
        codes.sort_unstable();
        assert_eq!(
            codes,
            [1, 3, 4, 5],
            "expected distinct codes 1/3/4/5, got 1={c1} 3={c3} 4={c4} 5={c5}"
        );
    }
);

// A live read on a clean tree DOES run git (the freshness short-circuit), so the
// docs must not claim "no git on the read path". Locks GITREAD-1.
rrtest!(
    live_read_clean_tree_invokes_git,
    |mut dir: Dir, mut cmd: TestCommand| {
        if !git_available() {
            eprintln!("skipping: git not available");
            return;
        }
        dir.file("a.txt", "hello\n");
        init_repo(&dir);
        commit_all(&dir, "init");
        cmd.arg("index").assert_exit_code(0);

        // A bare clean-tree read invokes git at least once (status / rev-parse).
        let (out, git_calls) = run_rr_counting_git(&dir, &["read", "a.txt", "--locate"]);
        assert_eq!(code(&out), 0, "clean-tree read should succeed: {out:?}");
        assert!(
            git_calls >= 1,
            "a clean-tree read must invoke git for the freshness short-circuit, got {git_calls}"
        );

        // And the documentation must not contain the false "no git" claim.
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

/// The `.rr/refs` manifest text, or empty when the sidecar does not exist.
fn manifest(dir: &Dir) -> String {
    std::fs::read_to_string(dir.path().join(".rr").join("refs")).unwrap_or_default()
}

/// The first manifest line of the given record kind (`"snapshot"` / `"track"`),
/// split into its TAB fields.
fn pin_fields(dir: &Dir, kind: &str) -> Vec<String> {
    let refs = manifest(dir);
    let prefix = format!("{kind}\t");
    let line = refs
        .lines()
        .find(|l| l.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no {kind} line in manifest:\n{refs}"));
    line.split('\t').map(str::to_string).collect()
}

/// The sharded sidecar object path for an oid.
fn object_path(dir: &Dir, oid: &str) -> std::path::PathBuf {
    dir.path()
        .join(".rr")
        .join("objects")
        .join(&oid[..2])
        .join(&oid[2..])
}

/// `git hash-object <path>` run in the test dir (the self-verifying re-hash).
fn git_hash_object(dir: &Dir, path: &std::path::Path) -> String {
    let out = Command::new("git")
        .arg("hash-object")
        .arg(path)
        .current_dir(dir.path())
        .output()
        .expect("failed to spawn git");
    assert!(out.status.success(), "git hash-object failed");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Run `rr <args>` in `dir` while counting the top-level git processes it spawns,
/// via `GIT_TRACE2_EVENT` (each git process emits exactly one `version` event).
/// The test repos disable git's own fan-out (no hooks, and rr's git calls never
/// trigger gc/maintenance), so the `version` count equals rr's direct git calls.
/// The trace lands outside the repo so it is never walked.
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

/// Rewrite `.rr/refs`, dropping every line that begins with `prefix` (used to
/// simulate a hand-deletion of a manifest record).
fn delete_manifest_lines(dir: &Dir, prefix: &str) {
    let path = dir.path().join(".rr").join("refs");
    let text = std::fs::read_to_string(&path).unwrap();
    let kept: String = text
        .lines()
        .filter(|l| !l.starts_with(prefix))
        .map(|l| format!("{l}\n"))
        .collect();
    std::fs::write(&path, kept).unwrap();
}
