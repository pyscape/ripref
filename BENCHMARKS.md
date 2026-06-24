# ripref benchmarks

Cross-platform performance of `rr`, measured 2026-06-24 on the two machines
below. Numbers are criterion medians (e2e numbers are means); where a measurement
is jittery its coefficient of variation (CV, the standard deviation over the
median) is given, because some of these benches are noisy and a bare median would
hide that.

Regenerate everything with [`scripts/bench_all.py`](scripts/bench_all.py) (it
gathers the host's specs and runs the whole suite); raw per-machine output lands
under `bench-results/` (gitignored). The individual commands are in
[Reproducing](#reproducing).

## Machines

These are deliberately different (a laptop and a desktop), so absolute times are
not an apples-to-apples hardware delta. Read the ratios and the structural
findings, not the raw milliseconds. (To remove the hardware variable entirely,
the plan is to re-run both OSes as matched VMs on one host; see
[Same-hardware comparison](#same-hardware-comparison-planned).)

| | Windows (laptop) | Linux (desktop) |
| --- | --- | --- |
| CPU | Intel i7-1255U, 10C/12T, 1.7 GHz base (~4.7 boost) | Intel i7-12700F, 12C/20T, 4.9 GHz |
| RAM | 16 GB | 14 GiB |
| Repo storage | NVMe SSD, NTFS | NVMe SSD, ext4 |
| Temp dir | `%TEMP%` on NVMe (NTFS) | `/tmp` is tmpfs (RAM); a disk `TMPDIR` measures the same warm |
| Anti-virus | Microsoft Defender real-time ON | none |
| OS | Windows 11 Home 26200 | Ubuntu 26.04 LTS, kernel 7.0 |
| Toolchain | rustc/cargo 1.95.0 (msvc) | rustc/cargo 1.95.0 (gnu) |

## Findings that hold on both platforms

These transfer across the two machines; they are the point of the report.

1. **The writer dominates.** `rr index` is a single serial loop and is the
   program's expensive operation: ~554 files/s on Linux, ~323 files/s on
   Windows. That is ~0.9 s to index 512 files on Linux (a ~2,200-file tree is
   ~4 s, ~5.4 s end to end), versus microsecond-to-millisecond reads.
   Parallelizing `indexer::build` is the open lever.
2. **Reads are O(n), not the README's "microsecond binary search".**
   `forward_lookup` and `covering` both grow linearly with index size on both
   platforms (Linux `forward_lookup_hit` 64.9 us at ~3k anchors -> 2.39 ms at
   ~98k; covering similar), because each rebuilds a record `Vec` over the whole
   forward section per call (`split_records`). They also allocate O(n)
   (~32.8 KB at N=1000 -> ~262 KB at N=8000, platform-independent; the
   `tests/alloc.rs` pin passes on both). The mmap and header parse are genuinely
   microseconds.
3. **Native language loading is free; WASM is not.** `language_init` is ~2 ns
   native (a function-pointer wrap) versus ~80 ms (Linux) / ~145 ms (Windows)
   for the WASM path (cranelift compiling the grammar, paid per process). Query
   compilation is at parity (~0.5 ms either backend) and parsing is ~1.5x slower
   under WASM. This is why first-class grammars are native and WASM is opt-in.
4. **The freshness walk's parallelism has a fixed ~140 us thread-spawn floor**
   on both platforms, so the serial-vs-parallel crossover is set entirely by
   per-`stat` cost: ~128 files on Windows, ~600-700 files on Linux. The guessed
   `PARALLEL_THRESHOLD = 256` therefore sits between the two platform optima,
   slightly high for Windows, slightly low for Linux. Raising it would help
   Linux by <100 us while costing Windows milliseconds, so 256 is a defensible,
   now-measured compromise rather than a number to change off either single
   platform.
5. **Parallel scaling is sublinear, and flat from 1 to 2 threads on both
   platforms.** Two threads give no speedup over serial; gains start at four
   (Linux 1.9x at 4 / 3.7x at 8 threads; Windows 1.9x at 4 / 2.4x at 8). The
   flat 1->2 step reproduces cross-platform and is currently unexplained, worth
   a look (SMT scheduling, or a serialization point such as the per-`stat`
   `PathBuf` join).
6. **The Windows-vs-Linux gap is Defender + CPU, not storage.** Disk-backed
   Linux equals tmpfs Linux for these warm benches (freshly-written files are
   cached either way), so storage backing is not the driver. Defender is: it
   taxes file *open* consistently (~24x: `query/open_mmap` 45.9 us on Windows
   versus 1.9 us on Linux) and metadata walks intermittently (the standalone
   freshness bench spiked to ~9 ms for 256 `stat`s during a Defender scan, ~90x
   the calm ~96 us). So Windows read latency is both higher and far noisier;
   pure in-memory CPU work (parse, lookup, covering) is a steadier ~2x Linux
   advantage from the faster desktop chip.
7. **rr's edge over ripgrep widens cold** (measured on Linux, where the cache can
   be dropped; the mechanism is platform-independent). Warm, ripgrep is fast
   (`rg` 9.8 ms/query), so rr's prebuilt index only amortizes after ~874 lookups.
   Cold, ripgrep must re-scan the whole corpus from disk (90.6 ms) while rr reads
   one small index, so the per-query saving jumps from ~6 ms to ~52 ms and rr
   amortizes after only ~118 queries. The index is most valuable exactly when the
   cache is cold: the first query, CI, or a corpus too big to stay resident.

## Numbers

### Freshness walk: serial vs parallel (warm, synthetic temp tree)

From `freshness_scaling`. Medians in us. The crossover is where parallel first
beats serial.

| files | Win serial | Win parallel | Linux serial | Linux parallel |
| --- | --- | --- | --- | --- |
| 256 | 95.7 | 185.8 | 105.9 | 189.4 |
| 512 | 192.5 | 202.4 | 210.3 | 209.6 |
| 1024 | 395.9 | 289.0 | 423.6 | 288.9 |
| 4096 | 1601 | 678 | 1802 | 764 |

Crossover: Windows ~128 files, Linux ~600-700 files. Serial `stat` cost is about
equal on the two machines in a calm run (~96 vs ~106 us at 256); the large gap
seen elsewhere on Windows is the intermittent Defender tax, not steady-state.

### Thread scaling (freshness walk, 4096 files)

| threads | Windows | Linux |
| --- | --- | --- |
| 1 | 1.62 ms (1.0x) | 1.73 ms (1.0x) |
| 2 | 1.62 ms (1.0x) | 1.72 ms (1.0x) |
| 4 | 849 us (1.9x) | 893 us (1.9x) |
| 8 | 452 us (2.4x) | 470 us (3.7x) |

### Index build: the writer

| | Windows | Linux |
| --- | --- | --- |
| build, 128 files | ~396 ms (~323 files/s) | 231 ms (~554 files/s) |
| build, 512 files | ~1.59 s (~323 files/s) | 919 ms (~557 files/s) |
| serialize, 128 | 16.3 us | 9.6 us |
| serialize, 512 | 59 us | 35.7 us |

Both serial; build (walk + read + tree-sitter parse) is the cost, serialize is
negligible. The index-build medians carry a large CV (filesystem jitter, larger
on Windows under Defender), so treat them as order-of-magnitude.

### Query: the reader (medians)

| operation | Windows | Linux |
| --- | --- | --- |
| open + mmap (any N) | ~45.9 us | ~1.9 us |
| parse, 8192 files | 265 us | 119.8 us |
| forward_lookup hit, 256 | 166 us | 64.9 us |
| forward_lookup hit, 8192 | 4.61 ms | 2.39 ms |
| covering, 8192 | 10.3 ms | 6.55 ms |

`open + mmap` is flat in N (it establishes the mapping); its ~24x Windows cost is
Defender scanning the opened index file. `forward_lookup` and `covering` are O(n)
on both (see finding 2).

### Grammar loading: native vs WASM

| operation | Win native | Win WASM | Linux native | Linux WASM |
| --- | --- | --- | --- | --- |
| language_init | 1.4 ns | ~145 ms | 2.28 ns | 79.5 ms |
| query_compile | 754 us | ~0.83 ms | 489 us | 483 us |
| parse, small doc | 28.8 us | ~49 us | 17.4 us | 26.4 us |
| parse, fixture doc | 3.2 ms | (not captured) | 2.16 ms | 3.40 ms |

Both native runs now parse the committed `tests/data/grammar_bench.md`, so the
fixture-doc row is comparable; the small-doc and `language_init`/`query_compile`
rows are input-independent. The Windows WASM run was the pre-fixture one, so its
fixture-doc parse is not shown (re-run `grammar_loader --features wasm` via
`bench_all.py` to fill it).

### End-to-end: rr vs ripgrep

Process-level, from `scripts/bench_e2e.py` on the clam corpus (~2,200 files /
~26k anchors): locate one symbol's definition via `rr read` (prebuilt index)
versus `rg` (scan). Means shown; the equivalence guard confirmed both resolve the
same location.

| metric | Windows warm | Linux warm | Linux cold |
| --- | --- | --- | --- |
| `rr index` (one-time) | ~9-14 s | 5.4 s | 6.1 s |
| `rr read` (per query) | ~22 ms | 3.6 ms | 38.9 ms |
| `rg` scan (per query) | ~59 ms | 9.8 ms | 90.6 ms |
| crossover (queries to amortize) | ~289 | 874 | 118 |

"Cold" drops the page cache before every sample (Linux `drop_caches`); it is the
first-touch cost (CI, a fresh shell, a working set larger than RAM). Two things
stand out. Cold `rr read` is ~11x its warm self (38.9 vs 3.6 ms), and that cost
is the freshness stat-walk going cold (re-statting every in-scope file from
disk), not the index mmap (2.7 MB faults in under a millisecond on NVMe), so
`--no-freshness` and the clean-tree git short-circuit matter most when cold. And
the crossover collapses from 874 queries warm to 118 cold (finding 7).

## Same-hardware comparison (planned)

The two machines above differ in CPU, so the cross-platform numbers are
confounded by hardware. To isolate the OS itself, the plan is to run a Windows
guest and a Linux guest as matched VMs on one host (same vCPU/RAM/virtual disk,
pinned cores). That cleanly answers two questions the bare-metal runs cannot:
Windows vs Linux on identical silicon, and a controlled Defender on/off pair on
the same Windows VM. Caveat: VM I/O is not native I/O, and rr's hot paths (`stat`,
file open, per-file build I/O) are the most virtualization-sensitive syscalls, so
these will be an OS-delta lens, not native absolutes; the bare-metal rows stay as
the native reference. `bench_all.py` records virtualization and Defender state in
its spec block so a VM run self-documents.

## Methodology and caveats

- **Warm unless noted.** A warmup primes the page cache before sampling. Cold
  first-touch is out of scope on Windows (no portable user-space cache drop); on
  Linux it is captured via `bench_e2e.py --cold-prepare "sync && sudo sh -c
  'echo 3 > /proc/sys/vm/drop_caches'"` (see the end-to-end table).
- **Hardware is not comparable.** Laptop U-series vs desktop F-series (~3x clock,
  12 vs 20 threads). Cross-platform claims here are about ratios and structure,
  not raw milliseconds; the VM plan above addresses this.
- **Windows synthetic-`stat` absolutes are unreliable** (Defender swings them up
  to ~90x). The Windows freshness numbers shown are from a calm run; the
  steady signal is the serial-vs-parallel ratio, with Linux for clean absolutes.
- **Storage backing is not a factor here** (disk == tmpfs for warm benches); see
  finding 6.
- **Standard deviation is reported as CV%** wherever a bench is jittery (the
  index build, the e2e harness, the parallel reductions). The regression gate
  (`scripts/bench_regression.py`) is noise-aware for the same reason.
- **Scope.** `search` and `enforce` are documented in the README but not yet
  implemented, so there is nothing to bench for them.

## Reproducing

Whole suite, per machine (writes specs + results under `bench-results/`):

```
python3 scripts/bench_all.py
python3 scripts/bench_all.py --e2e-corpus /path/to/clam --rg "$(command -v rg)"
```

Cold end-to-end (Linux, needs sudo for `drop_caches`):

```
python3 scripts/bench_all.py --e2e-corpus /path/to/clam --rg "$(command -v rg)" \
  --cold "sync && sudo sh -c 'echo 3 > /proc/sys/vm/drop_caches'"
```

Individual pieces:

```
cargo bench --bench freshness
cargo bench --bench index
cargo bench --bench query
cargo bench --bench freshness_scaling
cargo bench --bench grammar_loader
cargo bench --bench grammar_loader --features wasm
cargo test  --test alloc
python3 scripts/bench_e2e.py --corpus /path/to/clam --rg "$(command -v rg)"
```

On Linux, force a disk-backed temp dir (`export TMPDIR=$HOME/bench-tmp`) if you
want the temp-tree benches off tmpfs; in practice it matches the tmpfs numbers
because the files are warm in cache either way.
