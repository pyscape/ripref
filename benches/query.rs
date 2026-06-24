//! Benchmark for the index READER: the query work `rr read` and `rr at` do once
//! the index exists on disk. The only end-to-end query numbers we have otherwise
//! come from spawning the `rr` binary under a shell timer, where roughly half of
//! every sample is the Windows process spawn plus Defender scanning the ~3.9 MB
//! exe, which drowns the actual lookup. This bench isolates the lookup, but
//! against the REAL on-disk index: it serializes a synthesized index, writes it to
//! a temp file, and memory-maps that file exactly as `src/commands.rs` does
//! (`File::open` then `Mmap::map`), so parse and lookup run over an mmap of an
//! actual file rather than bytes that only ever lived on the heap. Only the
//! process spawn is stripped.
//!
//!   cargo bench --bench query
//!
//! The mapping is warm: the file is page-cached right after it is written, and
//! criterion's warmup faults every page in. That is the common case (a build or CI
//! loop reads the index repeatedly while it stays resident). The cold first-touch
//! cost (a major fault from disk) and the Windows-vs-Linux delta are deliberately
//! out of scope here: cold belongs to a process-level harness that can drop the
//! page cache, and the platform delta comes from re-running this same bench on
//! Linux. Defender interference on Windows is left in, since that is what a Windows
//! user actually pays.
//!
//! Four operations are measured:
//!
//!   query/parse               - [`ripref::refidx::Reader::parse`]: header plus
//!                               section table. It UTF-8-validates the whole image
//!                               first (`str::from_utf8` over every byte), so its
//!                               cost grows with total index size, not just header
//!                               size.
//!   query/forward_lookup_hit  - [`Reader::forward_lookup`] of an anchor that
//!                               exists. This is the README's microsecond claim,
//!                               and the bench exists to check it. The lookup IS a
//!                               binary search (O(log n) comparisons), but today it
//!                               first materializes a record index over the entire
//!                               forward section (an O(n) `split_records` pass) on
//!                               every call before bisecting that vec, so the real
//!                               cost is dominated by the O(n) preamble. The code's
//!                               own doc comment flags this ("a future version can
//!                               bisect the raw bytes in place without materializing
//!                               it"); this bench quantifies what that preamble
//!                               costs and how it scales.
//!   query/forward_lookup_miss - the same lookup for an anchor guaranteed absent
//!                               (the bisect lands on a partition point and finds
//!                               no match), to confirm a miss is no cheaper or
//!                               dearer than a hit.
//!   query/covering            - [`Reader::covering`]: the work `rr at` does. A
//!                               LINEAR scan of the whole forward section, O(n) in
//!                               total anchor count, so its throughput is reported
//!                               as anchors/second (the scan rate). The honest
//!                               finding to surface: neither read/forward_lookup
//!                               nor at/covering is constant-time today; both walk
//!                               the whole forward section per query, so query cost
//!                               grows with index size (covering by construction,
//!                               forward_lookup via the index it rebuilds each
//!                               call). The microsecond claim holds at small index
//!                               sizes but degrades as the corpus grows.
//!
//! The index is synthesized (no `indexer::build`, no tree-sitter): the query path
//! operates purely on the serialized index, so synthesizing gives precise control
//! over anchor count and a fast, deterministic setup. Each scale is written to its
//! own temp file, mapped once, queried, then the file is removed. Results land in
//! `target/criterion/`.

use std::fs::{self, File};
use std::hint::black_box;
use std::path::PathBuf;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use memmap2::Mmap;
use ripref::refidx::{self, ForwardEntry, IndexData, Reader};

// File counts bracketing clam (~2,200 files / ~26k anchors). At ~12 anchors per
// file (one path anchor plus ITEMS_PER_FILE symbol anchors) these land near ~3k
// and ~25k anchors; the third, larger scale makes each operation's scaling with
// index size unmistakable (see the module header: covering and forward_lookup
// both grow with the corpus today).
const SCALES: &[usize] = &[256, 2048, 8192];

// Symbol anchors emitted per file, on top of the single whole-file path anchor.
// Total anchors per file is therefore ITEMS_PER_FILE + 1, averaging ~12 like the
// clam corpus.
const ITEMS_PER_FILE: usize = 11;

/// Build a synthesized-but-realistic [`IndexData`] for `files` files, fully in
/// memory. Per file `i`: one whole-file path anchor spanning lines 1-200, plus
/// `ITEMS_PER_FILE` symbol anchors with realistic-length names and locations in
/// that same file. Byte length affects parse and scan cost, so the names are the
/// length of real Rust paths and symbols, not `a0`/`a1`.
///
/// File 0 is given a genuine NEST so [`Reader::covering`] on a mid-line returns
/// depth >= 3: the wide path anchor (1-200), a struct/impl-width span (40-120),
/// and an inner method span (60-80) all cover line 70.
///
/// `forward` is returned sorted by `anchor` (the writer's invariant, which
/// [`refidx::serialize`] does NOT enforce) and `paths` sorted too.
fn make_index(files: usize) -> IndexData {
    let mut forward = Vec::with_capacity(files * (ITEMS_PER_FILE + 1));
    let mut paths = Vec::with_capacity(files);

    for i in 0..files {
        // The whole-file path anchor (and a corresponding entry in `paths`).
        let path = format!("crates/c{i}/src/mod{i}.rs");
        forward.push(ForwardEntry {
            anchor: path.clone(),
            location: format!("{path}:1-200"),
        });
        paths.push(path.clone());

        if i == 0 {
            // A genuine nest for `covering`: line 70 sits inside the path anchor
            // (1-200), this struct/impl-width span (40-120), and the inner method
            // span (60-80), so covering returns depth >= 3.
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
        paths,
    }
}

/// An anchor known to exist in the corpus (a middle file's first method), to
/// drive the forward_lookup hit case.
fn hit_anchor(files: usize) -> String {
    let mid = files / 2;
    format!("mod{mid}::Type{mid}::method0")
}

/// An anchor guaranteed absent from any corpus this generator produces (its file
/// index is past `files`, and the `zz_` prefix matches nothing emitted).
fn miss_anchor(files: usize) -> String {
    format!("zz_missing::mod{files}::nope")
}

/// Serialize `data`, write it to a throwaway index file on disk, and memory-map
/// that file exactly as the reader commands do (`File::open` then `Mmap::map`).
/// Returns the live mapping plus its path so the caller can delete the file after
/// the scale is measured. The query benches run against this mapping, so what is
/// measured is the real on-disk read path (mmap plus page-fault-in), not bytes
/// that only ever lived on the heap.
fn write_and_map(data: &IndexData, files: usize) -> (Mmap, PathBuf) {
    let path =
        std::env::temp_dir().join(format!("rr-query-bench-{}-{}.idx", std::process::id(), files));
    fs::write(&path, refidx::serialize(data)).unwrap();
    let file = File::open(&path).unwrap();
    // SAFETY: we just wrote this file and nothing else mutates it for the bench's
    // lifetime; this mirrors the one justified mmap in src/commands.rs.
    let mmap = unsafe { Mmap::map(&file) }.unwrap();
    (mmap, path)
}

fn bench_query(c: &mut Criterion) {
    let mut group = c.benchmark_group("query");

    for &files in SCALES {
        let data = make_index(files);
        let total_anchors = data.forward.len();

        // Write the index to a real file and map it, then parse one Reader off the
        // clock and reuse it for the lookup/covering benches.
        let (mmap, idx_path) = write_and_map(&data, files);
        let reader = Reader::parse(&mmap).expect("synthesized index must parse");

        let hit = hit_anchor(files);
        let miss = miss_anchor(files);
        // The nest lives in file 0; line 70 is covered by all three of its spans.
        let cover_file = "crates/c0/src/mod0.rs";
        let cover_line = 70u64;

        // Correctness guards: a broken synthesis (unsorted forward, wrong location
        // format, miscounted nest) fails loudly here instead of timing garbage.
        // Mirrors the grammar_loader / index guards.
        assert_eq!(
            reader.forward_lookup(&hit).len(),
            1,
            "hit anchor {hit:?} must resolve to exactly one location at scale {files}"
        );
        assert!(
            reader.forward_lookup(&miss).is_empty(),
            "miss anchor {miss:?} must not resolve at scale {files}"
        );
        let depth = reader.covering(cover_file, cover_line).len();
        assert!(
            depth >= 3,
            "nest at {cover_file}:{cover_line} must cover depth >= 3, got {depth} at scale {files}"
        );

        // parse: header plus section table, over the mmap of the real file (the
        // same &[u8] the binary sees from disk).
        group.bench_with_input(BenchmarkId::new("parse", files), &mmap, |b, mmap| {
            b.iter(|| black_box(Reader::parse(black_box(&mmap[..])).unwrap()));
        });

        // forward_lookup hit: the README's microsecond claim. A binary search,
        // but preceded by an O(n) rebuild of the record index on every call (see
        // the module header), so this is where the claim is checked against scale.
        group.bench_with_input(
            BenchmarkId::new("forward_lookup_hit", files),
            &hit,
            |b, hit| {
                b.iter(|| black_box(reader.forward_lookup(black_box(hit))));
            },
        );

        // forward_lookup miss: same search, bisect lands on a partition point with
        // no match. Confirms a miss is no more expensive than a hit.
        group.bench_with_input(
            BenchmarkId::new("forward_lookup_miss", files),
            &miss,
            |b, miss| {
                b.iter(|| black_box(reader.forward_lookup(black_box(miss))));
            },
        );

        // covering: the `rr at` work, a linear scan of the whole forward section.
        // Throughput is anchors/second so criterion reports the scan rate; the
        // per-call time grows roughly linearly with total_anchors (the most
        // pronounced O(n) of the four operations).
        group.throughput(Throughput::Elements(total_anchors as u64));
        group.bench_with_input(
            BenchmarkId::new("covering", files),
            &(cover_file, cover_line),
            |b, &(file, line)| {
                b.iter(|| black_box(reader.covering(black_box(file), black_box(line))));
            },
        );

        // Done with this scale. Drop the reader (it borrows the mapping), then the
        // mapping, before deleting the file: on Windows an open mapping blocks the
        // delete.
        drop(reader);
        drop(mmap);
        fs::remove_file(&idx_path).ok();
    }

    group.finish();
}

criterion_group!(query, bench_query);
criterion_main!(query);
