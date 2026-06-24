#!/usr/bin/env python3

'''
Compare two saved criterion baselines and fail on a gross benchmark regression.

This is the analysis half of the CI `bench-regression` gate (see
.github/workflows/ci.yml). The workflow runs the STABLE benches twice -- once on
the PR head saved as baseline `pr`, once on the PR base saved as baseline `base`
-- and then runs this script to compare them. Nothing here builds or runs a
benchmark; it only reads estimates criterion has already written under
target/criterion.

Why only the stable benches
---------------------------
Only `freshness` and `query` are gated. Both are pure in-process microbenches
(one `stat` per path; one mmap + parse + lookup over a synthesized index) with
no multi-second points and no per-iteration filesystem writes, so they are
reproducible enough to compare run to run. `index` is deliberately NOT gated: its
build points take SECONDS each and write a fresh tree to disk every iteration, so
on a shared CI runner (where Defender/AV and a noisy neighbor perturb every file
write) its wall-clock swings far more than any threshold we could set without
making the gate meaningless. `grammar_loader` is heavy/WASM and likewise excluded.
The workflow only ever saves baselines for the two stable benches, so in practice
this script only sees those; the `--bench` filter is a second belt-and-braces.

Standard deviation and noise-aware gating
-----------------------------------------
A median on its own hides how noisy a benchmark is. CI benchmark gating is
inherently noisy: shared GitHub runners have no perf isolation, variable CPU
frequency, and co-tenant load, so the SAME code commonly varies 10-30% run to run
on a microbench (and far more on the slow points, which is why those are not
gated). So this script reads criterion's `std_dev` alongside the median and
reports each point's coefficient of variation (CV = std_dev / median) -- its
relative jitter -- next to the timing.

The gate then USES that jitter rather than ignoring it. A benchmark is FAILED
only when its median slowdown both:
  (a) exceeds `--threshold` (a gross slowdown), AND
  (b) is larger than the measurement noise -- more than `--noise-sigmas` times the
      two points' combined relative sigma (sqrt(cv_base^2 + cv_new^2), the
      first-order relative uncertainty of the ratio).
A delta that clears the threshold but sits INSIDE the noise band is reported as a
warning, not a failure: it is exactly the jitter-driven swing a naive median-only
gate would flake on. The median is used (not the mean) because it is robust to the
occasional outlier sample these points produce; improvements never fail. Net
effect: a calm bench needs only a clear threshold breach to fail, while a jittery
bench must move well beyond its own noise -- the honest bar.

How comparison works
--------------------
For each baseline NAME, criterion writes, per benchmark id, a directory
`target/criterion/<group>/<function>/<value>/<NAME>/` containing `estimates.json`
(the distribution) and `benchmark.json` (which carries the canonical `full_id`,
e.g. "freshness/parallel/256"). This script:
  * walks both baselines, keying each benchmark by that `full_id`;
  * reads `median.point_estimate` (falling back to `mean`) and
    `std_dev.point_estimate`, both in nanoseconds;
  * for every id present in BOTH baselines computes delta = (new - base) / base
    and the combined relative sigma;
  * prints a readable table (id, base median +/- CV%, new median +/- CV%, delta%);
  * exits 1 only on a CONFIDENT regression (over threshold AND beyond the noise
    band). Over-threshold-but-within-noise deltas are warnings; improvements never
    fail.
Benchmarks present in only one baseline (added or removed between base and head)
are reported as a note and ignored -- they cannot be compared, and their presence
or absence must not fail the gate.

Pure standard library, no pip dependencies, matching the repo's Python scripts
(scripts/bench_e2e.py).
'''

import argparse
import json
import math
import os
import os.path as path
import sys

# 0.50 = 50%. See the module docstring: chosen clearly above the 10-30% noise a
# shared CI runner shows for the SAME code, so the gate catches gross/algorithmic
# regressions without flaking on ordinary variance.
DEFAULT_THRESHOLD = 0.50

# A threshold-breaching slowdown must also exceed this many combined sigmas of
# measurement noise to FAIL (otherwise it is reported as within-noise). 2 sigma is
# "clearly outside the run-to-run jitter", not just a noisy median.
DEFAULT_NOISE_SIGMAS = 2.0

# Where criterion writes its baselines, relative to the repo root.
CRITERION_ROOT = path.join('target', 'criterion')


def eprint(*args, **kwargs):
    'Like print, but to stderr.'
    kwargs['file'] = sys.stderr
    print(*args, **kwargs)


def read_estimate(estimates_path):
    '''
    Read one criterion estimates.json; return (median_ns, stddev_ns) or None.

    Median is the point estimate (robust to outliers; falls back to the mean only
    if a future criterion drops median). stddev is `std_dev.point_estimate`, the
    sample standard deviation criterion already computed -- 0.0 if absent (e.g. a
    single-sample bench), which reads as "no measured jitter". Returns None if
    there is no usable median or the file is unreadable, so one malformed file
    degrades to "uncomparable" rather than crashing the gate.
    '''
    try:
        with open(estimates_path, 'r', encoding='utf-8') as f:
            data = json.load(f)
    except (OSError, ValueError):
        return None
    median = None
    for key in ('median', 'mean'):
        stat = data.get(key)
        if isinstance(stat, dict) and isinstance(stat.get('point_estimate'), (int, float)):
            median = float(stat['point_estimate'])
            break
    if median is None:
        return None
    sd = data.get('std_dev')
    stddev = float(sd['point_estimate']) if isinstance(sd, dict) and isinstance(
        sd.get('point_estimate'), (int, float)) else 0.0
    return (median, stddev)


def collect_baseline(root, baseline, bench_filters):
    '''
    Walk target/criterion and collect {full_id: (median_ns, stddev_ns)} for one
    baseline NAME.

    A benchmark's saved baseline is a directory literally named NAME that holds
    BOTH estimates.json and benchmark.json. Requiring benchmark.json (which the
    derived `change/` delta directory does not have, and the rolled-up group
    `report/` directories do not have) is what keeps the walk to real, comparable
    benchmark leaves -- and benchmark.json carries the canonical full_id, so ids
    never have to be reconstructed from path segments.

    `bench_filters`, if given, keeps only ids whose group (the segment before the
    first '/') is in the set -- the script's half of "gate only the stable
    benches".
    '''
    results = {}
    if not path.isdir(root):
        return results
    for dirpath, dirnames, filenames in os.walk(root):
        if path.basename(dirpath) != baseline:
            continue
        if 'estimates.json' not in filenames or 'benchmark.json' not in filenames:
            continue
        # Do not descend below a matched baseline dir.
        dirnames[:] = []
        try:
            with open(path.join(dirpath, 'benchmark.json'), 'r',
                      encoding='utf-8') as f:
                full_id = json.load(f).get('full_id')
        except (OSError, ValueError):
            full_id = None
        if not full_id:
            continue
        if bench_filters:
            group = full_id.split('/', 1)[0]
            if group not in bench_filters:
                continue
        est = read_estimate(path.join(dirpath, 'estimates.json'))
        if est is None or est[0] <= 0.0:
            # A zero/negative/absent median cannot anchor a ratio; skip it rather
            # than divide by zero or invent a delta.
            continue
        results[full_id] = est
    return results


def cv(median, stddev):
    'Coefficient of variation (relative jitter); 0 when the median is 0.'
    return stddev / median if median > 0.0 else 0.0


def fmt_ns(ns):
    'Human-friendly nanosecond formatting with a fixed-width-ish unit.'
    if ns >= 1e9:
        return '%8.3f s ' % (ns / 1e9)
    if ns >= 1e6:
        return '%8.3f ms' % (ns / 1e6)
    if ns >= 1e3:
        return '%8.3f us' % (ns / 1e3)
    return '%8.1f ns' % ns


def compare(base, new, threshold, noise_sigmas, min_ns):
    '''
    Compare two {full_id: (median_ns, stddev_ns)} maps.

    Returns (rows, confident, within_noise, only_base, only_new, skipped_fast):
      rows: (id, b_med, b_sd, n_med, n_sd, delta, sigma) for ids in BOTH, sorted
            by delta descending (worst first); sigma is the combined relative
            uncertainty of the ratio.
      confident: rows that FAIL -- delta > threshold AND delta > noise_sigmas*sigma.
      within_noise: rows over threshold but inside the noise band (warn, not fail).
      only_base / only_new: ids in just one baseline (reported, not failed).
      skipped_fast: ids dropped because both medians are below min_ns.
    '''
    common = sorted(set(base) & set(new))
    rows = []
    skipped_fast = []
    for bid in common:
        b_med, b_sd = base[bid]
        n_med, n_sd = new[bid]
        # With a wall-clock floor, skip a point only when BOTH sides are below it:
        # a point that crossed the floor between base and head is worth keeping.
        if min_ns > 0.0 and b_med < min_ns and n_med < min_ns:
            skipped_fast.append(bid)
            continue
        delta = (n_med - b_med) / b_med
        # First-order relative uncertainty of the ratio new/base.
        sigma = math.hypot(cv(b_med, b_sd), cv(n_med, n_sd))
        rows.append((bid, b_med, b_sd, n_med, n_sd, delta, sigma))
    rows.sort(key=lambda r: r[5], reverse=True)
    over = [r for r in rows if r[5] > threshold]
    confident = [r for r in over if r[5] > noise_sigmas * r[6]]
    within_noise = [r for r in over if r[5] <= noise_sigmas * r[6]]
    only_base = sorted(set(base) - set(new))
    only_new = sorted(set(new) - set(base))
    return rows, confident, within_noise, only_base, only_new, skipped_fast


def print_table(rows):
    'Print the id / base / new / delta% table, each timing with its CV (jitter).'
    if not rows:
        print('  (no benchmarks present in both baselines)')
        return
    width = max(len(r[0]) for r in rows)

    def cell(med, sd):
        return '%s +/-%3.0f%%' % (fmt_ns(med), cv(med, sd) * 100.0)

    header = '  {id:{w}}   {base:>17}   {new:>17}   {delta:>9}'.format(
        id='benchmark', w=width, base='base', new='new', delta='delta%')
    print(header)
    print('  ' + '-' * (len(header) - 2))
    for bid, b_med, b_sd, n_med, n_sd, delta, _sigma in rows:
        print('  {id:{w}}   {base:>17}   {new:>17}   {delta:+8.1f}%'.format(
            id=bid, w=width, base=cell(b_med, b_sd), new=cell(n_med, n_sd),
            delta=delta * 100.0))
    print()
    print("  (+/-N% is each point's coefficient of variation: its run-to-run "
          'jitter.)')


def main():
    p = argparse.ArgumentParser(
        description='Compare two saved criterion baselines and exit 1 on a '
                    'confident gross regression (over threshold AND beyond '
                    'measurement noise). Improvements never fail. Reports each '
                    'point with its standard deviation; see the module docstring.')
    p.add_argument(
        '--base', metavar='NAME', default='base',
        help='Baseline name for the reference (the PR base). Default: %(default)s')
    p.add_argument(
        '--new', metavar='NAME', default='pr',
        help='Baseline name for the candidate (the PR head). Default: %(default)s')
    p.add_argument(
        '--threshold', metavar='PCT', type=float, default=DEFAULT_THRESHOLD * 100.0,
        help='A median slowdown beyond this percent is a candidate regression. '
             'Conservative default absorbs shared-runner noise. Default: %(default)s')
    p.add_argument(
        '--noise-sigmas', metavar='N', type=float, default=DEFAULT_NOISE_SIGMAS,
        help='A candidate regression FAILS only if its delta also exceeds this '
             "many combined sigmas of the two points' jitter. Default: %(default)s")
    p.add_argument(
        '--bench', metavar='GROUP', action='append', default=None,
        help='Restrict to this benchmark group (the part before the first "/", '
             'e.g. freshness or query). Repeatable. Default: all groups found.')
    p.add_argument(
        '--min-ns', metavar='NS', type=float, default=0.0,
        help='Skip a benchmark when BOTH baselines are below this wall-clock '
             'floor in nanoseconds. The sub-100us points are the noisiest in '
             'relative terms; use e.g. --min-ns 100000 to drop them if one '
             'flakes. Default: 0 (compare everything).')
    p.add_argument(
        '--criterion-root', metavar='DIR', default=CRITERION_ROOT,
        help='Criterion output directory. Default: %(default)s')
    args = p.parse_args()

    threshold = args.threshold / 100.0
    bench_filters = set(args.bench) if args.bench else None

    base = collect_baseline(args.criterion_root, args.base, bench_filters)
    new = collect_baseline(args.criterion_root, args.new, bench_filters)

    if not base and not new:
        eprint('error: found no benchmarks for baseline %r or %r under %s' % (
            args.base, args.new, args.criterion_root))
        eprint('Did the benches run with --save-baseline %s / %s first?' % (
            args.base, args.new))
        sys.exit(2)

    rows, confident, within_noise, only_base, only_new, skipped_fast = compare(
        base, new, threshold, args.noise_sigmas, args.min_ns)

    print('benchmark regression check')
    print('  base baseline : %r  (%d benchmarks)' % (args.base, len(base)))
    print('  new  baseline : %r  (%d benchmarks)' % (args.new, len(new)))
    print('  fail rule     : delta > +%.1f%% AND delta > %.1f x combined jitter '
          '(median; improvements never fail)' % (
              threshold * 100.0, args.noise_sigmas))
    if args.min_ns > 0.0:
        print('  min-ns floor  : %.0f ns' % args.min_ns)
    print()
    print_table(rows)
    print()

    # Added/removed benchmarks cannot be compared; report them, never fail on
    # them. A benchmark legitimately appears or disappears when a bench file
    # changes between base and head.
    if only_new:
        print('note: present only in new baseline (added; not compared): %s'
              % ', '.join(only_new))
    if only_base:
        print('note: present only in base baseline (removed; not compared): %s'
              % ', '.join(only_base))
    if skipped_fast:
        print('note: below --min-ns floor on both sides (not compared): %s'
              % ', '.join(skipped_fast))

    # Over the threshold but inside the noise band: a swing this big is plausibly
    # just this benchmark's jitter, so report it without failing the build.
    if within_noise:
        print()
        print('warning: %d benchmark(s) over +%.1f%% but WITHIN measurement noise '
              '(not failed):' % (len(within_noise), threshold * 100.0))
        for bid, b_med, b_sd, n_med, n_sd, delta, sigma in within_noise:
            print('  %s  %s -> %s  (%+.1f%%, jitter +/-%.0f%%)' % (
                bid, fmt_ns(b_med), fmt_ns(n_med), delta * 100.0, sigma * 100.0))

    if confident:
        print()
        print('FAIL: %d benchmark(s) regressed beyond +%.1f%% AND beyond the '
              'noise band:' % (len(confident), threshold * 100.0))
        for bid, b_med, b_sd, n_med, n_sd, delta, sigma in confident:
            print('  %s  %s -> %s  (%+.1f%%, jitter +/-%.0f%%)' % (
                bid, fmt_ns(b_med), fmt_ns(n_med), delta * 100.0, sigma * 100.0))
        print()
        print('This gate is a smoke alarm for gross/algorithmic regressions, not '
              'a precise')
        print('ratchet. If this is shared-runner noise rather than a real '
              'slowdown, re-run the')
        print('job; if it keeps flaking, set continue-on-error: true on the CI '
              'step (see ci.yml).')
        sys.exit(1)

    print('OK: no benchmark regressed beyond +%.1f%% outside the measurement '
          'noise.' % (threshold * 100.0))


if __name__ == '__main__':
    main()
