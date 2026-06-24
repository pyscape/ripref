//! Tuning bench for the freshness stat-walk's parallel constants. The library's
//! [`ripref::indexer::newest_mtime`] hard-codes `PARALLEL_THRESHOLD = 256` (below
//! that count it runs serial) and otherwise fans across every core. Those two
//! numbers were picked by reasoning, not measurement. This bench measures them:
//! where parallel first beats serial (the empirically-correct threshold) and how
//! the parallel reduction scales with thread count.
//!
//!   cargo bench --bench freshness_scaling
//!
//! Why replicas instead of calling `newest_mtime` directly: the real function
//! cannot be forced parallel below 256 and always uses all cores, so it cannot
//! locate the crossover or sweep thread count. This file therefore carries its
//! OWN serial reducer and a PARAMETERIZED parallel reducer with no threshold
//! guard and an explicit `n_threads`. Both MIRROR the reduction in
//! `indexer::newest_mtime`: serial is one `stat` per path, newest wins, missing
//! files contribute 0; parallel is `chunk = len.div_ceil(n_threads)`, one
//! `thread::scope` thread per chunk each running the serial max-reduce, overall
//! max. The operation is fixed, so the replicas cannot drift in any way that
//! matters -- but the measured crossover only applies to the real function while
//! they match it. If `newest_mtime`'s reduction changes, update this bench in
//! step. (`freshness.rs` makes the same argument for its serial replica.)
//!
//! Caveats:
//!   - This is a MANUAL tuning bench. It is NOT wired into the CI bench-regression
//!     gate, which stays freshness + query. Run it by hand when retuning.
//!   - On Windows the %TEMP% stat cost is high and noisy (Defender taxes every
//!     metadata call), so the CROSSOVER POINT and the serial-vs-parallel RATIOS
//!     are the signal here, not the absolute microseconds. Run on Linux for clean
//!     absolutes and the real platform threshold: cheaper stats there shift the
//!     crossover higher, so the threshold is platform-dependent and the Linux run
//!     is the representative one.

use std::hint::black_box;
use std::path::Path;
use std::time::UNIX_EPOCH;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

// Spans the threshold region so the serial/parallel crossover is visible on
// either side of the guessed 256.
const CROSSOVER_COUNTS: &[usize] = &[16, 32, 64, 128, 256, 512, 1024, 2048, 4096];

// Large enough that per-thread work dominates spawn overhead, so the scaling
// curve reflects the reduction rather than thread startup.
const THREADS_COUNT: usize = 4096;
const THREAD_SWEEP: &[usize] = &[1, 2, 4, 8];

/// Create `n` empty files under a fresh temp dir; return the dir (kept alive by
/// the caller for the bench's lifetime) and the relative paths to walk.
fn make_tree(n: usize) -> (std::path::PathBuf, Vec<String>) {
    let dir =
        std::env::temp_dir().join(format!("rr-fresh-scale-bench-{}-{}", std::process::id(), n));
    std::fs::create_dir_all(&dir).unwrap();
    let paths: Vec<String> = (0..n).map(|i| format!("f{i}.txt")).collect();
    for p in &paths {
        std::fs::write(dir.join(p), b"x").unwrap();
    }
    (dir, paths)
}

/// One `stat` per path, newest wins, missing files contribute 0. Mirrors the
/// private `indexer::newest_serial`.
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

/// The chunked parallel max-reduce with no threshold guard and an explicit thread
/// count. Mirrors `indexer::newest_mtime`'s reduction exactly, minus the guard:
/// `chunk = len.div_ceil(n_threads)`, one `thread::scope` thread per chunk each
/// running `newest_serial`, overall max. `n_threads` is clamped to the path count
/// so empty chunks are never spawned.
fn newest_parallel(paths: &[&str], root: &Path, n_threads: usize) -> u64 {
    let n = n_threads.min(paths.len()).max(1);
    if n == 1 {
        return newest_serial(paths, root);
    }
    let chunk = paths.len().div_ceil(n);
    std::thread::scope(|s| {
        paths
            .chunks(chunk)
            .map(|c| s.spawn(move || newest_serial(c, root)))
            .collect::<Vec<_>>()
            .into_iter()
            .filter_map(|h| h.join().ok())
            .max()
            .unwrap_or(0)
    })
}

fn cores() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get())
}

/// Serial vs all-cores parallel at each count: the count where parallel first
/// wins is the empirical threshold to compare against the guessed 256.
fn bench_crossover(c: &mut Criterion) {
    let all = cores();
    let mut group = c.benchmark_group("freshness_scaling/crossover");
    for &n in CROSSOVER_COUNTS {
        let (dir, owned) = make_tree(n);
        let paths: Vec<&str> = owned.iter().map(String::as_str).collect();
        group.throughput(Throughput::Elements(n as u64));

        // Same input, same filesystem state -- only the reduction differs.
        group.bench_with_input(BenchmarkId::new("serial", n), &paths, |b, paths| {
            b.iter(|| black_box(newest_serial(paths, &dir)));
        });
        group.bench_with_input(BenchmarkId::new("parallel", n), &paths, |b, paths| {
            b.iter(|| black_box(newest_parallel(paths, &dir, all)));
        });

        std::fs::remove_dir_all(&dir).ok();
    }
    group.finish();
}

/// Parallel reduction at increasing thread counts on a fixed large tree: the
/// speedup at 2/4/8 vs 1 shows how the walk scales with cores.
fn bench_threads(c: &mut Criterion) {
    let all = cores();
    let (dir, owned) = make_tree(THREADS_COUNT);
    let paths: Vec<&str> = owned.iter().map(String::as_str).collect();

    let mut group = c.benchmark_group("freshness_scaling/threads");
    group.throughput(Throughput::Elements(THREADS_COUNT as u64));
    // Cap each requested count at the real core count (more threads than cores
    // only adds scheduling contention), then drop the duplicates that cap produces
    // on small machines -- 4 and 8 both become 4 on a 4-core box -- so criterion
    // never sees two benchmarks with the same id. THREAD_SWEEP is ascending, so
    // the capped values stay ordered and consecutive dedup suffices.
    let mut sweep: Vec<usize> = THREAD_SWEEP.iter().map(|&t| t.min(all)).collect();
    sweep.dedup();
    for &t in &sweep {
        group.bench_with_input(BenchmarkId::from_parameter(t), &paths, |b, paths| {
            b.iter(|| black_box(newest_parallel(paths, &dir, t)));
        });
    }
    group.finish();

    std::fs::remove_dir_all(&dir).ok();
}

criterion_group!(freshness_scaling, bench_crossover, bench_threads);
criterion_main!(freshness_scaling);
