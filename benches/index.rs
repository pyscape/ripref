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
use std::time::Duration;

use criterion::{
    criterion_group, criterion_main, BenchmarkId, Criterion, SamplingMode, Throughput,
};
use ripref::indexer;
use ripref::refidx::{self, IndexData};

mod corpus;
use corpus::{index_path_for, make_corpus};

// 128 brackets a small crate; 512 a mid-size one (clam is ~2,200 files). build
// is slow, so the build group caps at criterion's minimum sample size to keep
// the whole run in minutes rather than tens of minutes (see bench_build).
const SCALES: &[usize] = &[128, 512];

fn bench_build(c: &mut Criterion) {
    let mut group = c.benchmark_group("index");
    // Flat sampling is criterion's mode for long-running benchmarks: the default
    // linear ramp cannot fit 10 samples of a multi-second build into the window
    // and warns. measurement_time must clear 10 flat samples of the slowest
    // (512-file) scale (~2 s each), with headroom for filesystem jitter (Windows
    // Defender scanning each freshly written file).
    group.sample_size(10);
    group.sampling_mode(SamplingMode::Flat);
    group.measurement_time(Duration::from_secs(30));

    for &n in SCALES {
        let root = make_corpus(n);
        let index_path = index_path_for(&root);

        // Correctness guard: require that language extraction fired, not just the
        // per-file path anchor the indexer always adds. That path-only floor is
        // exactly `n`, so a dead grammar would still clear a `>= n` bar and time
        // nothing meaningful; a healthy corpus yields ~11 anchors/file, so `5 * n`
        // sits well above the floor and well below the expected count. Mirrors
        // grammar_loader's native/wasm guard.
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
