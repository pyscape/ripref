//! Benchmark for the freshness stat-walk: the serial reduction vs the parallel
//! [`ripref::indexer::newest_mtime`] across file counts. At scale this walk
//! dominates query latency — on a ~2.3k-file tree it is ~75% of an `rr at` — so
//! this isolates it from the process start, mmap fault-in, and covering scan
//! that an end-to-end timing folds together.
//!
//!   cargo bench --bench freshness
//!
//! Each count gets a throwaway temp tree of empty files. The walk does one `stat`
//! per path, so file *count* is the only variable that matters (contents are
//! irrelevant). The counts bracket the realistic range; 256 is the parallel
//! threshold, so every measured count takes the threaded path. Results land in
//! `target/criterion/`.

use std::hint::black_box;
use std::path::Path;
use std::time::UNIX_EPOCH;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use ripref::indexer::newest_mtime;

// 256 is `PARALLEL_THRESHOLD`, so each count exercises the threaded reduction;
// 1024/4096 bracket clam-scale trees (~2.3k files) on either side.
const COUNTS: &[usize] = &[256, 1024, 4096];

/// Create `n` empty files under a fresh temp dir; return the dir (kept alive by
/// the caller for the bench's lifetime) and the relative paths to walk.
fn make_tree(n: usize) -> (std::path::PathBuf, Vec<String>) {
    let dir = std::env::temp_dir().join(format!("rr-fresh-bench-{}-{}", std::process::id(), n));
    std::fs::create_dir_all(&dir).unwrap();
    let paths: Vec<String> = (0..n).map(|i| format!("f{i}.txt")).collect();
    for p in &paths {
        std::fs::write(dir.join(p), b"x").unwrap();
    }
    (dir, paths)
}

/// The serial reduction, mirroring the private `indexer::newest_serial`: one
/// `stat` per path, newest wins, missing files contribute 0. Replicated here as
/// the baseline rather than exposed from the lib so the public API stays minimal
/// — the operation is fixed, so the two cannot drift in any way that matters.
fn newest_serial(paths: &[&str], root: &Path) -> u64 {
    let mut newest = 0u64;
    for p in paths {
        let full = root.join(p);
        if let Ok(secs) = std::fs::metadata(&full)
            .and_then(|m| m.modified())
            .map(|m| {
                m.duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0)
            })
        {
            newest = newest.max(secs);
        }
    }
    newest
}

fn bench_freshness(c: &mut Criterion) {
    let mut group = c.benchmark_group("freshness");
    for &n in COUNTS {
        let (dir, owned) = make_tree(n);
        let paths: Vec<&str> = owned.iter().map(String::as_str).collect();

        // Same input, same filesystem state — only the reduction differs.
        group.bench_with_input(BenchmarkId::new("serial", n), &paths, |b, paths| {
            b.iter(|| black_box(newest_serial(paths, &dir)));
        });
        group.bench_with_input(BenchmarkId::new("parallel", n), &paths, |b, paths| {
            b.iter(|| black_box(newest_mtime(paths, &dir)));
        });

        // Both measurements ran during the calls above; the temp tree can go.
        std::fs::remove_dir_all(&dir).ok();
    }
    group.finish();
}

criterion_group!(freshness, bench_freshness);
criterion_main!(freshness);
