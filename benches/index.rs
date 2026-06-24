//! Benchmark for the index WRITER: the `rr index` build path. Building the index
//! on a real ~2,200-file tree takes ~14.5 s, while a warm query (`rr at` /
//! `rr read`) is ~30 ms, so the writer is roughly 500x a query and, until now,
//! had no benchmark at all. `indexer::build` is a single serial loop today (walk
//! with the `ignore` crate, then read + tree-sitter-parse + extract, one file at
//! a time), so this is the prerequisite for later parallelizing that loop and
//! proving the win.
//!
//!   cargo bench --bench index
//!
//! Two points are measured separately:
//!   index/build     - [`ripref::indexer::build`] over the corpus. This is the
//!                     parse-bound bulk: walk, read, parse, extract, sort.
//!   index/serialize - [`ripref::refidx::serialize`] over a pre-built IndexData.
//!                     This is the encode step only (no walk, no parse), so it
//!                     isolates the cheap tail from the expensive body.
//!
//! Each scale gets one throwaway temp tree of realistic, parseable source files
//! (roughly 80% Rust, 20% Markdown), generated once (build is read-only on the
//! tree, so it is reused across iterations) and removed after that scale is
//! measured. The corpus is fully deterministic (no rng): names and bodies are
//! derived from indices, so the numbers are reproducible run to run.
//!
//! The metric is [`Throughput::Elements`] over the file count, so criterion
//! reports files/second. That is the portable, machine-independent signal: raw
//! wall-clock is inflated by Windows Defender scanning each freshly written file,
//! whereas files/s (and the eventual serial-vs-parallel speedup ratio) is what
//! actually transfers between machines. Results land in `target/criterion/`.

use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::time::Duration;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use ripref::indexer;
use ripref::refidx::{self, IndexData};

// 128 brackets a small crate; 512 a mid-size one (clam is ~2,200 files). build
// is slow, so the build group caps at criterion's minimum sample size to keep
// the whole run in minutes rather than tens of minutes (see bench_build).
const SCALES: &[usize] = &[128, 512];

// Each generated `.rs` file emits this many extractable items, spread across the
// construct kinds the Rust anchors query matches (fn / struct / enum / trait /
// const / type). With the per-file path anchor that the indexer always adds,
// each Rust file yields roughly this many + 1 anchors; mixed with Markdown the
// corpus averages close to clam's observed ~12 anchors/file.
const ITEMS_PER_RS_FILE: usize = 12;

/// One realistic, parseable Rust source file whose item names are derived from
/// `i` (so the corpus is deterministic). It defines `ITEMS_PER_RS_FILE` items
/// spread across the construct kinds the Rust anchors query captures, so every
/// file contributes a realistic count of anchors rather than just its path.
fn rust_source(i: usize) -> String {
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
fn markdown_source(i: usize) -> String {
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
fn make_corpus(n: usize) -> PathBuf {
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
fn index_path_for(root: &Path) -> PathBuf {
    root.join(".ref-cache").join("index")
}

fn bench_build(c: &mut Criterion) {
    let mut group = c.benchmark_group("index");
    // build does real I/O + parsing per file, so it is slow; cap at criterion's
    // minimum sample count so the run stays in minutes. serialize (a separate
    // group below) is cheap and keeps the default sample size. The 512 scale needs
    // ~15 s to collect 10 samples, so widen the measurement window past criterion's
    // 5 s default (otherwise it warns and the slow estimate gets noisy).
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(16));

    for &n in SCALES {
        let root = make_corpus(n);
        let index_path = index_path_for(&root);

        // Correctness guard: build once up front and require that language
        // extraction actually fired, not just the per-file path anchor the indexer
        // always adds. The path-only floor is one anchor per file (exactly `n`), so
        // a dead grammar or unparseable corpus would still clear a `>= n` bar and
        // time nothing meaningful; a healthy corpus yields ~11 anchors/file, so a
        // `5 * n` threshold sits well above the floor and well below the expected
        // count, failing loudly on broken extraction. Mirrors grammar_loader's
        // native/wasm guard.
        let data = indexer::build(&root, &index_path).unwrap();
        assert!(
            data.forward.len() >= n * 5,
            "corpus at scale {n} extracted only {} anchors (< {}); language extraction is broken",
            data.forward.len(),
            n * 5
        );

        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::new("build", n), &n, |b, _| {
            b.iter(|| black_box(indexer::build(&root, &index_path).unwrap()));
        });

        // build is read-only on the tree, so the measurements above all reused
        // this corpus; it can go now.
        std::fs::remove_dir_all(&root).ok();
    }
    group.finish();
}

fn bench_serialize(c: &mut Criterion) {
    let mut group = c.benchmark_group("index");

    for &n in SCALES {
        // serialize is encode-only, so build the IndexData once (off the clock)
        // and reuse it; the temp tree exists only long enough to produce it.
        let root = make_corpus(n);
        let index_path = index_path_for(&root);
        let data: IndexData = indexer::build(&root, &index_path).unwrap();
        std::fs::remove_dir_all(&root).ok();

        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::new("serialize", n), &data, |b, data| {
            b.iter(|| black_box(refidx::serialize(data)));
        });
    }
    group.finish();
}

criterion_group!(index, bench_build, bench_serialize);
criterion_main!(index);
