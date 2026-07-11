#![allow(dead_code)]
//! Shared deterministic corpus generators for the index and query benches.
//!
//! Both `benches/index.rs` (the writer path) and `benches/query.rs` (the reader
//! path) need realistic, reproducible inputs, and previously each carried its own
//! near-duplicate generator. This module is the single source of truth so the two
//! cannot drift: a change to anchor density or naming lands in one place and both
//! benches see it.
//!
//! Two independent generators live here, one per bench:
//!   - the on-disk file-tree side ([`make_corpus`] + [`rust_source`],
//!     [`markdown_source`], [`index_path_for`], [`ITEMS_PER_RS_FILE`]) feeds the
//!     writer bench, which walks and parses real files;
//!   - the in-memory side ([`make_index`] + [`hit_anchor`], [`miss_anchor`],
//!     [`ITEMS_PER_FILE`]) feeds the reader bench, which operates on a serialized
//!     index and never touches tree-sitter.
//!
//! Each bench crate compiles the whole module but uses only its half, so the
//! file-level `#![allow(dead_code)]` above is load-bearing: without it the unused
//! half trips clippy's `-D warnings`.
//!
//! Invariants a plausible edit could break, most-violable first:
//!   - Both generators are fully deterministic (no rng): names and bodies derive
//!     from indices, so bench numbers are reproducible run to run. Introducing any
//!     randomness here silently makes every dependent bench noisy.
//!   - [`make_index`] returns `forward` sorted by `anchor` and `paths` sorted;
//!     `refidx::serialize` does NOT enforce this, and `Reader::forward_lookup`'s
//!     binary search is wrong without it.
//!   - The two anchor-density consts are intentionally different (the on-disk side
//!     counts every extracted item toward its total, the in-memory side adds a
//!     file-spanning module anchor on top), but both are tuned so each corpus
//!     averages ~12 anchors/file like the real clam tree. Keep that ~12 average
//!     if you retune either.

use std::path::{Path, PathBuf};

use ripref::refidx::{ForwardEntry, IndexData};

// ---------------------------------------------------------------------------
// On-disk file-tree corpus (the writer / index bench)
// ---------------------------------------------------------------------------

// Each generated `.rs` file emits this many extractable items, spread across the
// construct kinds the Rust anchors query matches (fn / struct / enum / trait /
// const / type), so each Rust file yields roughly this many anchors; mixed with
// Markdown the corpus averages close to clam's observed ~12 anchors/file.
pub const ITEMS_PER_RS_FILE: usize = 12;

/// One realistic, parseable Rust source file whose item names are derived from
/// `i` (so the corpus is deterministic). It defines `ITEMS_PER_RS_FILE` items
/// spread across the construct kinds the Rust anchors query captures, so every
/// file contributes a realistic count of anchors rather than just its path.
pub fn rust_source(i: usize) -> String {
    let mut s = String::new();
    s.push_str(&format!("//! Generated module {i}.\n\n"));
    // A const and a type alias (both are anchor kinds in the Rust query).
    s.push_str(&format!("pub const LIMIT_{i}: usize = {i};\n"));
    s.push_str(&format!("pub type Alias{i} = u64;\n\n"));
    // A struct plus an impl with a method (the method is a `function_item` nested
    // in the impl, so it is captured too).
    s.push_str(&format!(
        "pub struct Config{i} {{\n    pub value: u64,\n}}\n\n"
    ));
    s.push_str(&format!(
        "impl Config{i} {{\n    pub fn value(&self) -> u64 {{\n        self.value\n    }}\n}}\n\n"
    ));
    // An enum and a trait.
    s.push_str(&format!(
        "pub enum State{i} {{\n    Idle,\n    Running,\n    Done,\n}}\n\n"
    ));
    s.push_str(&format!(
        "pub trait Handler{i} {{\n    fn handle(&self) -> usize;\n}}\n\n"
    ));
    // Free functions to top the file up to ITEMS_PER_RS_FILE items. Items so far:
    // const, type, struct, method, enum, trait = 6.
    let already = 6;
    for f in 0..ITEMS_PER_RS_FILE.saturating_sub(already) {
        s.push_str(&format!(
            "pub fn compute_{i}_{f}(x: u64) -> u64 {{\n    x.wrapping_add({f})\n}}\n\n"
        ));
    }
    s
}

/// One realistic Markdown file with a handful of ATX headings (the Markdown
/// anchor kind). Deterministic in `i`.
pub fn markdown_source(i: usize) -> String {
    format!(
        "# Document {i}\n\nIntro paragraph for document {i}.\n\n\
         ## Overview\n\nSome overview text.\n\n\
         ## Details\n\nMore detail here.\n\n\
         ### Notes\n\nClosing notes.\n"
    )
}

/// Write a fresh corpus of `n` parseable files under an isolated temp dir and
/// return that dir. Roughly 80% Rust, 20% Markdown (every fifth file is
/// Markdown). The dir name carries the process id and scale (mirroring
/// freshness.rs `make_tree`) so concurrent or repeated runs never collide.
/// Files are NOT dot-prefixed, since the walker skips hidden entries.
pub fn make_corpus(n: usize) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rr-index-bench-{}-{}", std::process::id(), n));
    std::fs::create_dir_all(&dir).unwrap();
    for i in 0..n {
        if i % 5 == 0 {
            std::fs::write(dir.join(format!("doc{i}.md")), markdown_source(i)).unwrap();
        } else {
            std::fs::write(dir.join(format!("mod{i}.rs")), rust_source(i)).unwrap();
        }
    }
    dir
}

/// The index path passed to `build` only excludes the index file itself from the
/// walk; it is dot-prefixed, so it is naturally skipped and need not exist.
pub fn index_path_for(root: &Path) -> PathBuf {
    root.join(".ref-cache").join("index")
}

// ---------------------------------------------------------------------------
// In-memory index corpus (the reader / query bench)
// ---------------------------------------------------------------------------

// Symbol anchors emitted per file, on top of one file-spanning module anchor.
// Total anchors per file is therefore ITEMS_PER_FILE + 1, averaging ~12 like the
// clam corpus.
pub const ITEMS_PER_FILE: usize = 11;

/// Build a synthesized-but-realistic [`IndexData`] for `files` files, fully in
/// memory. Per file `i`: one file-spanning module anchor (lines 1-200), plus
/// `ITEMS_PER_FILE` symbol anchors with realistic-length names and locations in
/// that same file. Byte length affects parse and scan cost, so the names are the
/// length of real Rust paths and symbols, not `a0`/`a1`.
///
/// File 0 is given a genuine NEST so [`ripref::refidx::Reader::covering`] on a
/// mid-line returns depth >= 3: the wide module anchor (1-200), a
/// struct/impl-width span (40-120), and an inner method span (60-80) all cover
/// line 70.
///
/// `forward` is returned sorted by `anchor` (the writer's invariant, which
/// `refidx::serialize` does NOT enforce) and `paths` sorted too.
pub fn make_index(files: usize) -> IndexData {
    let mut forward = Vec::with_capacity(files * (ITEMS_PER_FILE + 1));
    let mut paths = Vec::with_capacity(files);

    for i in 0..files {
        // A file-spanning module anchor (and the file's entry in `paths`).
        let path = format!("crates/c{i}/src/mod{i}.rs");
        forward.push(ForwardEntry {
            anchor: format!("mod{i}"),
            location: format!("{path}:1-200"),
        });
        paths.push(path.clone());

        if i == 0 {
            // A genuine nest for `covering`: line 70 sits inside the module
            // anchor (1-200), this struct/impl-width span (40-120), and the
            // inner method span (60-80), so covering returns depth >= 3.
            forward.push(ForwardEntry {
                anchor: format!("mod{i}::Type{i}"),
                location: format!("{path}:40-120"),
            });
            forward.push(ForwardEntry {
                anchor: format!("mod{i}::Type{i}::run"),
                location: format!("{path}:60-80"),
            });
            // Top this file up to ITEMS_PER_FILE symbol anchors with non-nesting
            // methods elsewhere in the file, so file 0 has the same anchor count
            // as the rest.
            for j in 2..ITEMS_PER_FILE {
                forward.push(ForwardEntry {
                    anchor: format!("mod{i}::Type{i}::method{j}"),
                    location: format!("{path}:{}-{}", 130 + j, 131 + j),
                });
            }
        } else {
            for j in 0..ITEMS_PER_FILE {
                let start = 1 + j * 15;
                forward.push(ForwardEntry {
                    anchor: format!("mod{i}::Type{i}::method{j}"),
                    location: format!("{path}:{}-{}", start, start + 10),
                });
            }
        }
    }

    // serialize does not sort, so the writer's "sorted by anchor" invariant is the
    // caller's job; forward_lookup's binary search is wrong without it.
    forward.sort_by(|a, b| a.anchor.cmp(&b.anchor));
    paths.sort();

    IndexData {
        mtime: 1_718_660_000,
        tree: "0123456789abcdef0123456789abcdef01234567".to_string(),
        forward,
        mentions: Vec::new(),
        paths,
    }
}

/// An anchor known to exist in the corpus (a middle file's first method), to
/// drive the forward_lookup hit case.
pub fn hit_anchor(files: usize) -> String {
    let mid = files / 2;
    format!("mod{mid}::Type{mid}::method0")
}

/// An anchor guaranteed absent from any corpus this generator produces (its file
/// index is past `files`, and the `zz_` prefix matches nothing emitted).
pub fn miss_anchor(files: usize) -> String {
    format!("zz_missing::mod{files}::nope")
}
