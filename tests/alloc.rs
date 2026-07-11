//! Allocation pin for the index read path.
//!
//! [`Reader::forward_lookup`] and [`Reader::covering`] allocate O(n) in the
//! index size on every call; this test pins that behavior so it cannot
//! silently worsen, and BENCHMARKS.md carries the measured costs.
//!
//! Root cause: both methods call `split_records` (in the `refidx` module), which
//! `.collect()`s a `Vec` of record slices over the ENTIRE forward section on
//! every call. So each query allocates O(n) in the index size -- the same O(n)
//! the `query` bench attributes to that per-call rebuild. A real in-place bisect
//! (which the `forward_lookup` doc comment notes is possible) would allocate
//! O(1); when that lands these bounds should drop toward zero and the
//! scales-with-N assertions below should be inverted or deleted.
//!
//! Key invariant: a CLEAN allocation counter requires exactly ONE test thread.
//! Cargo runs the `#[test]` fns in a binary on parallel threads sharing this
//! process's global allocator, so a second test fn here would let its
//! allocations bleed into these counters. This file is therefore ONE test fn:
//! one fn, one thread, one uncontended odometer. (Other test binaries are
//! separate processes and never touch this counter.)

use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::sync::atomic::{AtomicUsize, Ordering};

use ripref::refidx::{serialize, ForwardEntry, IndexData, Reader};

/// A pass-through allocator that tallies what it hands out. `bytes` is a
/// monotonic "gross bytes requested" odometer -- it only ever climbs, because a
/// freed-then-reallocated buffer still cost a fresh allocation we want to count;
/// a net "currently live" gauge would read zero for exactly the transient `Vec`
/// this test exists to catch. `count` is the number of `alloc` calls.
struct Counting {
    bytes: AtomicUsize,
    count: AtomicUsize,
}

// SAFETY: every method forwards verbatim to `System`, which is a sound
// `GlobalAlloc`; the atomic tally has no bearing on the returned pointer or the
// memory's validity. The `#[allow]` keeps this the one conspicuous `unsafe` in
// the file, per the crate's lint posture (`#![warn(unsafe_code)]`).
#[allow(unsafe_code)]
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.bytes.fetch_add(layout.size(), Ordering::Relaxed);
        self.count.fetch_add(1, Ordering::Relaxed);
        System.alloc(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // Never subtract: `bytes` is a gross-allocated odometer, not a live gauge.
        System.dealloc(ptr, layout);
    }
}

#[global_allocator]
static A: Counting = Counting {
    bytes: AtomicUsize::new(0),
    count: AtomicUsize::new(0),
};

/// Gross bytes the allocator has handed out so far.
fn bytes() -> usize {
    A.bytes.load(Ordering::Relaxed)
}

/// `alloc` calls so far.
fn count() -> usize {
    A.count.load(Ordering::Relaxed)
}

/// A SORTED `IndexData` of `n` entries that round-trips the real format. Anchors
/// `mod{i}::item{i}` are emitted in `i` order, which is also lexical order here
/// (zero-padded), satisfying `forward`'s sorted-by-anchor invariant -- without
/// it `forward_lookup`'s binary search would be wrong. Every span is `:1-10`, so
/// line 5 of file `i` is covered by entry `i`.
fn sorted_index(n: usize) -> IndexData {
    let width = (n - 1).to_string().len();
    let forward = (0..n)
        .map(|i| ForwardEntry {
            anchor: format!("mod{i:0width$}::item{i:0width$}"),
            location: format!("crates/c{i:0width$}/src/mod{i:0width$}.rs:1-10"),
        })
        .collect();
    let paths = (0..n)
        .map(|i| format!("crates/c{i:0width$}/src/mod{i:0width$}.rs"))
        .collect();
    IndexData {
        mtime: 0,
        tree: String::new(),
        forward,
        mentions: Vec::new(),
        paths,
    }
}

/// Bytes and `alloc`-call delta a single `f()` invocation costs. The two
/// odometer reads bracket exactly one call with nothing else allocating between
/// them. Each `f` already `black_box`es the operation's input and result
/// internally, so the call cannot be elided as dead; nothing is needed here.
fn measure(f: impl FnOnce()) -> (usize, usize) {
    let (b0, c0) = (bytes(), count());
    f();
    (bytes() - b0, count() - c0)
}

/// Pins that `forward_lookup` and `covering` allocate, and allocate MORE as the
/// index grows -- the O(n)-per-call behavior that contradicts the README's "no
/// allocation". Asserts only the robust ">0" and "scales-with-N" shape; exact
/// byte counts vary by allocator and platform, so they live in comments, not
/// assertions.
///
/// Measured on this machine (System allocator, 64-bit; bytes / `alloc` calls):
///
/// | op             | N = 1000      | N = 8000        |
/// | -------------- | ------------- | --------------- |
/// | forward_lookup | 32,830 B / 11 | 262,208 B / 14  |
/// | covering       | 33,000 B / 12 | 262,380 B / 15  |
///
/// The ~8x byte jump tracks the 8x entry count: `split_records` builds a
/// `Vec<&[u8]>` holding one fat pointer per record, so its backing buffer is
/// O(n) (the dominant term -- 8000 records times 16 bytes, plus growth
/// overshoot, is ~256 KB). The handful of extra small `alloc`s are the result
/// `Vec`s the operations return (`forward_lookup`'s one location, `covering`'s
/// hits) and do not scale with the index. Counts are recorded for context only;
/// the assertions below are on bytes.
#[test]
fn read_path_allocates_and_scales_with_index_size() {
    // SETUP -- everything that allocates happens here, BEFORE any measurement:
    // build both indexes, serialize, parse the readers, and materialize the
    // query arguments. The owned argument strings (anchor, cover file) must
    // exist before the snapshots so their own allocation is not charged to the
    // call under test.
    const SMALL: usize = 1_000;
    const LARGE: usize = 8_000;

    let small = sorted_index(SMALL);
    let large = sorted_index(LARGE);
    let small_bytes = serialize(&small);
    let large_bytes = serialize(&large);
    let small_reader = Reader::parse(&small_bytes).expect("small index must parse");
    let large_reader = Reader::parse(&large_bytes).expect("large index must parse");

    // A hit anchor in the middle of each index, and a `file:line` some entry's
    // span covers. Owned now so the measured call allocates nothing for them.
    let small_hit = small.forward[SMALL / 2].anchor.clone();
    let large_hit = large.forward[LARGE / 2].anchor.clone();
    let small_cover_file = location_file(&small.forward[SMALL / 2].location);
    let large_cover_file = location_file(&large.forward[LARGE / 2].location);
    let cover_line = 5u64; // inside every `:1-10` span

    // Guard the fixtures off the clock: a mis-synthesized index (unsorted,
    // un-covering) would otherwise make the deltas below measure nothing.
    assert_eq!(
        small_reader.forward_lookup(&small_hit).len(),
        1,
        "small hit must resolve to exactly one location"
    );
    assert_eq!(
        large_reader.forward_lookup(&large_hit).len(),
        1,
        "large hit must resolve to exactly one location"
    );
    assert!(
        !small_reader
            .covering(&small_cover_file, cover_line)
            .is_empty(),
        "small cover position must be covered"
    );
    assert!(
        !large_reader
            .covering(&large_cover_file, cover_line)
            .is_empty(),
        "large cover position must be covered"
    );

    // MEASURE -- one call per snapshot pair, nothing else allocating between.
    let (fwd_small_b, _fwd_small_c) = measure(|| {
        black_box(small_reader.forward_lookup(black_box(&small_hit)));
    });
    let (fwd_large_b, _fwd_large_c) = measure(|| {
        black_box(large_reader.forward_lookup(black_box(&large_hit)));
    });
    let (cov_small_b, _cov_small_c) = measure(|| {
        black_box(small_reader.covering(black_box(&small_cover_file), black_box(cover_line)));
    });
    let (cov_large_b, _cov_large_c) = measure(|| {
        black_box(large_reader.covering(black_box(&large_cover_file), black_box(cover_line)));
    });

    // forward_lookup: NOT allocation-free (direct contradiction of the README),
    // and it allocates strictly more as the index grows -- O(n) per call, not
    // the O(1)/O(log n) a true in-place bisect would cost.
    assert!(
        fwd_small_b > 0,
        "forward_lookup must allocate at N={SMALL} (README claims it must not); got {fwd_small_b} B"
    );
    assert!(
        fwd_large_b > fwd_small_b,
        "forward_lookup must allocate more at N={LARGE} than N={SMALL} \
         (scales with index size): {fwd_large_b} B vs {fwd_small_b} B"
    );

    // covering: same two properties.
    assert!(
        cov_small_b > 0,
        "covering must allocate at N={SMALL} (README claims it must not); got {cov_small_b} B"
    );
    assert!(
        cov_large_b > cov_small_b,
        "covering must allocate more at N={LARGE} than N={SMALL} \
         (scales with index size): {cov_large_b} B vs {cov_small_b} B"
    );
}

/// The `file` half of a `file:start-end` location, owned. Splits from the right
/// so a colon inside the path keeps its prefix, mirroring the reader's own
/// `parse_location`.
fn location_file(loc: &str) -> String {
    loc.rsplit_once(':')
        .map(|(f, _)| f)
        .unwrap_or(loc)
        .to_string()
}
