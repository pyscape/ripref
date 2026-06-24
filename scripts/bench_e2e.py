#!/usr/bin/env python3

'''
Process-level end-to-end benchmark for ripref (rr) versus ripgrep (rg).

Modeled on ripgrep's benchsuite (crates/.../benchsuite/benchsuite): same
Command / Benchmark / Result shape, the same warmup-then-sample loop, the same
"time the whole process with time.time()" method, and the same per-tool output
equivalence guard. Pure standard library, no pip dependencies, exactly like
benchsuite.

What it measures
----------------
rr and rg do different things, so this frames ONE logical task: "locate the
definition of a symbol S in the corpus."

  * rr's model:  one-time `rr index` build, then each query is a cheap indexed
                 `rr read --locate S`.
  * rg's model:  no index; each query is a full `rg` scan for S.

It reports, benchsuite-style (a warmup primes the page cache, then N samples,
min / mean / stdev over the FULL process wall-clock):

  1. `rr index` build: the one-time setup cost (rg has none; reported alone).
  2. `rr read --locate S` per query vs `rg S` per query, on the same S.
  3. An equivalence guard (the benchsuite line-count analog): rg's matches must
     include the exact file:line that `rr read` resolves to. If they disagree
     the pair is FLAGGED and its comparison is not trusted.
  4. The crossover k: the query count at which (rr_index + k*rr_read) first
     beats (k*rg_scan). rr wins when you query repeatedly; rg wins one-off.

Invariants a plausible edit could break
---------------------------------------
  * Time the ENTIRE process (spawn + mmap + page cache + scan), never an inner
    phase. That whole-process wall-clock is the point of this layer; the
    in-process criterion benches already isolate the inner phases.
  * Numbers are WARM by default. A warmup run primes the OS page cache before
    sampling, just like benchsuite.

Cold-cache measurement (--cold-prepare)
---------------------------------------
By default this reports WARM numbers (the corpus and index are page-cached).
To report COLD first-touch numbers as well, pass --cold-prepare CMD: a shell
command run immediately BEFORE every timed run (each warmup AND each recorded
sample, for every command), outside the timed region, to evict the relevant
pages so the next invocation faults them from disk. This is the same idea as
hyperfine's --prepare. The hook is only useful if CMD actually drops the cache;
a CMD that does nothing yields warm numbers mislabeled as cold, so a CMD that
exits non-zero aborts the run rather than silently producing warm timings.

Recommended CMDs (the drop is OS-specific; this harness only invokes it):

  * Linux, full drop (needs root):
      sync && sudo sh -c 'echo 3 > /proc/sys/vm/drop_caches'
  * Linux, no root, targeted if vmtouch is installed (evict the corpus dir and
    the index file specifically):
      vmtouch -e <corpus-dir> <index-file>
  * Windows: there is no portable user-space page-cache drop, so cold is
    effectively Linux-only. Do not fake it on Windows; run cold on Linux.
  * rr emits forward-slash paths; ripgrep on Windows emits backslashes. The
    guard normalizes separators before comparing, or it would falsely flag
    every Windows pair.
  * There is no real `rg` on PATH here (the interactive one is a shell shim);
    rg and rr are always invoked by absolute path, located/built up front.

Corpora are never committed. This harness locates an on-disk corpus by path
(defaulting to a sibling checkout) and skips gracefully if it is absent.
'''

import argparse
import os
import os.path as path
import statistics
import subprocess
import sys
import time

# Default sibling-checkout locations. The brief's environment: rr and a genuine
# ripgrep both build under their own checkouts' target/release, and clam is the
# "many small files" corpus (the stressful shape for rr, whose freshness walk
# and index build scale with file count). All three are overridable on the CLI.
_REPO_ROOT = path.dirname(path.dirname(path.abspath(__file__)))
_SIBLINGS = path.dirname(_REPO_ROOT)

DEFAULT_CORPUS = path.join(_SIBLINGS, 'clam')

# The symbol whose definition both tools must locate. Chosen from clam because
# `rr read take_trace` resolves to exactly one location and a plain `rg`
# scan finds that same file:line (so the equivalence guard passes). Override
# with --symbol for any other corpus.
DEFAULT_SYMBOL = 'take_trace'


def _exe(name):
    'Append .exe on Windows so absolute-path lookups hit the real binary.'
    return name + '.exe' if os.name == 'nt' else name


DEFAULT_RR = path.join(_REPO_ROOT, 'target', 'release', _exe('rr'))
DEFAULT_RG = path.join(_SIBLINGS, 'ripgrep', 'target', 'release', _exe('rg'))


def eprint(*args, **kwargs):
    'Like print, but to stderr.'
    kwargs['file'] = sys.stderr
    print(*args, **kwargs)


class PrepareError(Exception):
    'Raised when a --cold-prepare invocation exits non-zero.'


def run_prepare(prepare):
    '''
    Run the cold-cache prepare command, outside any timed region.

    A non-zero exit aborts: a prepare whose cache drop silently failed would
    leave pages warm and produce warm timings mislabeled as cold, which is
    worse than reporting no cold numbers at all.
    '''
    completed = subprocess.run(
        prepare, shell=True, stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL)
    if completed.returncode != 0:
        raise PrepareError(
            'cold-prepare command failed (exit %d): %s'
            % (completed.returncode, prepare))


class Command(object):
    '''
    One command run as part of a benchmark.

    A Command owns its argv and the captured stdout of its last run. Output
    redirection is controlled by the harness (run_one), not the caller, so the
    same command can be timed with stdout discarded or captured for the guard.
    '''

    def __init__(self, name, cmd, cwd=None):
        '''
        :param str name: human-readable label (the same tool may appear twice).
        :param list(str) cmd: argv including the binary path at index 0.
        :param str cwd: working directory for the run.
        '''
        self.name = name
        self.cmd = cmd
        self.cwd = cwd
        self.last_stdout = b''

    @property
    def binary(self):
        'The binary path (argv[0]).'
        return self.cmd[0]

    def exists(self):
        'True iff the binary exists. Absolute paths only; PATH is not trusted.'
        return path.isfile(self.binary)

    def run(self, capture):
        '''
        Run once. Returns the elapsed full-process wall-clock in seconds.

        stderr is always discarded. stdout is captured only when the caller
        needs it for the equivalence guard; discarding it otherwise keeps the
        measurement closer to what a user waits for.
        '''
        stdout = subprocess.PIPE if capture else subprocess.DEVNULL
        start = time.time()
        completed = subprocess.run(
            self.cmd, cwd=self.cwd,
            stdout=stdout, stderr=subprocess.DEVNULL)
        end = time.time()
        if capture:
            self.last_stdout = completed.stdout or b''
        return end - start


class Result(object):
    '''
    The samples collected for one Command, plus their distribution.

    Mirrors benchsuite's Result: a flat list of per-run durations from which
    min / mean / stdev are derived. stdev needs at least two samples.
    '''

    def __init__(self, name):
        self.name = name
        self.durations = []

    def add(self, duration):
        self.durations.append(duration)

    @property
    def min(self):
        return min(self.durations) if self.durations else None

    @property
    def mean(self):
        return statistics.mean(self.durations) if self.durations else None

    @property
    def stdev(self):
        return statistics.stdev(self.durations) if len(self.durations) > 1 \
            else 0.0


class Benchmark(object):
    '''
    A group of commands timed on one corpus, benchsuite-style.

    Each command is run `warmup` times (discarded, to prime the page cache)
    then `count` times (recorded). run_one wraps a single process in
    time.time(), exactly like benchsuite's run_one.
    '''

    def __init__(self, warmup=1, count=10, prepare=None):
        '''
        :param str prepare: cold-cache shell command run before every timed run
            (warmups and samples), outside the timed region. None = warm.
        '''
        self.warmup = warmup
        self.count = count
        self.prepare = prepare

    def run(self, cmd, capture_last=False):
        '''
        Warm up, then sample `cmd`. Returns its Result.

        :param bool capture_last: capture stdout on the final sample so the
            caller can run the equivalence guard on real query output.
        '''
        result = Result(cmd.name)
        for _ in range(self.warmup):
            self._timed_run(cmd, capture=False)
        for i in range(self.count):
            last = capture_last and i == self.count - 1
            result.add(self._timed_run(cmd, capture=last))
        return result

    def _timed_run(self, cmd, capture):
        '''
        Run the prepare hook (if any) then time one invocation.

        Nothing runs between the prepare and the timed run that could re-warm
        the cache, and the prepare's own time is never counted.
        '''
        if self.prepare is not None:
            run_prepare(self.prepare)
        return cmd.run(capture=capture)


# --- The rr-vs-rg task: locate the definition of symbol S ------------------

def normalize_locations(text):
    '''
    Extract a set of normalized "file:line" tokens from tool output.

    Both rr's `--locate` ("path:start-end") and rg's `-n` ("path:line:match")
    are reduced to {"path:line"} with forward slashes, so a Windows backslash
    path from rg compares equal to rr's forward-slash path. For an rr span
    "path:start-end" only the start line is kept: that is the line rg's scan is
    expected to contain.
    '''
    locs = set()
    for raw in text.splitlines():
        line = raw.strip()
        if not line:
            continue
        # Windows drive letters ("C:\\...") also contain a colon, but tool
        # output here is corpus-relative, so splitting on the first two colons
        # is unambiguous: <path>:<line>[:<rest>].
        parts = line.split(':')
        if len(parts) < 2:
            continue
        file_part = parts[0].replace('\\', '/')
        line_part = parts[1].split('-')[0].strip()  # "start-end" -> "start"
        if not line_part.isdigit():
            continue
        locs.add('%s:%s' % (file_part, line_part))
    return locs


def equivalence_guard(rr_cmd, rg_cmd):
    '''
    The benchsuite line-count analog for this task.

    Resolve S both ways and confirm rg's match set CONTAINS the single
    file:line that rr resolves to. Returns (ok, rr_loc, rg_count). A False ok
    means the pair disagrees and its timing comparison is not trustworthy
    (e.g. S is ambiguous, or rg's pattern misses the definition).
    '''
    rr_cmd.run(capture=True)
    rg_cmd.run(capture=True)
    rr_locs = normalize_locations(rr_cmd.last_stdout.decode('utf-8', 'replace'))
    rg_locs = normalize_locations(rg_cmd.last_stdout.decode('utf-8', 'replace'))
    # rr read --locate of a well-chosen S yields exactly one location.
    if len(rr_locs) != 1:
        return False, ', '.join(sorted(rr_locs)) or '(none)', len(rg_locs)
    rr_loc = next(iter(rr_locs))
    return (rr_loc in rg_locs), rr_loc, len(rg_locs)


def crossover_k(index_mean, rr_read_mean, rg_scan_mean):
    '''
    Smallest integer k where rr's amortized total beats rg's.

    Solve index + k*rr_read < k*rg_scan. If rr_read >= rg_scan, rr's per-query
    is not cheaper, so the index never amortizes: return None.
    '''
    per_query_saving = rg_scan_mean - rr_read_mean
    if per_query_saving <= 0:
        return None
    import math
    # Strict inequality, so the smallest integer strictly greater than the tie
    # point; +1 when the tie lands exactly on an integer.
    tie = index_mean / per_query_saving
    k = math.floor(tie) + 1
    return k


def find_corpus(corpus):
    'Return the corpus dir if it exists, else None.'
    return corpus if corpus and path.isdir(corpus) else None


def print_row(label, result, max_label):
    '''
    Print one benchsuite-style distribution row: min / mean +/- stdev.

    benchsuite reports mean +/- stdev; min is added here because for a warm,
    cache-primed micro-task the minimum is the most reproducible single number
    (least perturbed by scheduler noise), and the brief asks for it.
    '''
    if result is None or result.mean is None:
        print('{0:{1}}  (skipped)'.format(label, max_label))
        return
    print('{label:{pad}}  min {mn:7.1f} ms   mean {mean:7.1f} +/- '
          '{stdev:6.1f} ms'.format(
              label=label, pad=max_label,
              mn=result.min * 1000.0,
              mean=result.mean * 1000.0,
              stdev=result.stdev * 1000.0))


def main():
    p = argparse.ArgumentParser(
        description='Process-level end-to-end benchmark: ripref (rr) vs '
                    'ripgrep (rg), locating a symbol definition. Warm '
                    '(cache-primed) full-process wall-clock, modeled on '
                    "ripgrep's benchsuite.")
    p.add_argument(
        '--corpus', metavar='DIR', default=DEFAULT_CORPUS,
        help='Directory to index and search (the corpus). Skipped gracefully '
             'if absent. Default: %(default)s')
    p.add_argument(
        '--rr', metavar='PATH', default=DEFAULT_RR,
        help='Absolute path to the rr binary. Default: %(default)s')
    p.add_argument(
        '--rg', metavar='PATH', default=DEFAULT_RG,
        help='Absolute path to a genuine ripgrep binary (NOT a PATH lookup; '
             'there is no real rg on PATH in this environment). '
             'Default: %(default)s')
    p.add_argument(
        '--symbol', metavar='S', default=DEFAULT_SYMBOL,
        help='The symbol whose definition both tools locate. Must resolve to '
             'one location under `rr read` and be found by `rg`. '
             'Default: %(default)s')
    p.add_argument(
        '--samples', metavar='N', type=int, default=10,
        help='Recorded samples per command (after warmup). Default: %(default)s')
    p.add_argument(
        '--warmup', metavar='N', type=int, default=1,
        help='Warmup runs per command (discarded; primes the page cache). '
             'Default: %(default)s')
    p.add_argument(
        '--cold-prepare', metavar='CMD', dest='cold_prepare', default=None,
        help='Shell command run immediately before every timed run (each '
             'warmup and each sample), outside the timed region, to evict the '
             'corpus/index pages so each invocation faults from disk (cold '
             'first-touch), like hyperfine --prepare. Omit for WARM numbers '
             '(the default, unchanged). A non-zero exit aborts so a failed '
             'cache drop cannot silently yield warm numbers labeled cold. '
             "Linux full drop (root): \"sync && sudo sh -c 'echo 3 > "
             '/proc/sys/vm/drop_caches\'". Linux no-root: '
             '"vmtouch -e <corpus> <index>". No portable equivalent on '
             'Windows, so cold is Linux-only.')
    args = p.parse_args()

    corpus = find_corpus(args.corpus)
    if corpus is None:
        eprint('corpus not found: %s' % args.corpus)
        eprint('Pass --corpus PATH to a real on-disk tree. (Corpora are not '
               'committed; this harness only locates them.)')
        sys.exit(0)

    rr_bin, rg_bin = args.rr, args.rg
    missing = [b for b in (rr_bin, rg_bin) if not path.isfile(b)]
    if missing:
        for b in missing:
            eprint('binary not found: %s' % b)
        eprint('Build them in their checkouts:')
        eprint('  cargo build --release --manifest-path '
               '<rr>/Cargo.toml')
        eprint('  cargo build --release --manifest-path '
               '<ripgrep>/Cargo.toml')
        sys.exit(1)

    s = args.symbol
    index_cmd = Command('rr index', [rr_bin, 'index'], cwd=corpus)
    # --locate keeps rr's output to the bare resolved location, and --no-color
    # keeps it parseable; this is the cheap indexed lookup rr is built for.
    rr_read_cmd = Command(
        'rr read', [rr_bin, 'read', '--locate', '--no-color', s], cwd=corpus)
    # rg's natural model: a plain recursive scan for S, no index. -n gives the
    # file:line the guard checks against rr's resolved location.
    rg_scan_cmd = Command('rg scan', [rg_bin, '-n', s], cwd=corpus)

    cold = args.cold_prepare is not None
    mode = 'cold, full-process wall-clock' if cold \
        else 'warm, full-process wall-clock'
    print('ripref end-to-end benchmark (%s)' % mode)
    print('  corpus : %s' % corpus)
    print('  rr     : %s' % rr_bin)
    print('  rg     : %s' % rg_bin)
    print('  symbol : %s   (task: locate the definition)' % s)
    print('  method : %d warmup run(s) prime the page cache, then %d samples'
          % (args.warmup, args.samples))
    if cold:
        print('  mode   : COLD (prepare = "%s")' % args.cold_prepare)
        print('           prepare runs before every timed run, outside the '
              'timing.')
    else:
        print('  mode   : WARM (no --cold-prepare). Cold first-touch needs '
              '--cold-prepare')
        print('           with a real cache-drop CMD (Linux only); not faked '
              'on Windows.')
    print()

    # The index must exist before the read is timed, and the guard needs both
    # tools answering. Build it once up front (outside the timing loop).
    eprint('# building index once before sampling reads...')
    index_cmd.run(capture=False)

    ok, rr_loc, rg_count = equivalence_guard(rr_read_cmd, rg_scan_cmd)
    print('equivalence guard (rg matches must include rr\'s resolved line):')
    print('  rr read %s  ->  %s' % (s, rr_loc))
    print('  rg scan %s  ->  %d matching line(s)' % (s, rg_count))
    if ok:
        print('  OK: rg\'s matches include rr\'s resolved location.')
    else:
        print('  FLAGGED: tools disagree; the comparison below is NOT trusted.')
        print('  (Pick a --symbol that rr resolves to one location and rg '
              'finds.)')
    print()

    # The equivalence guard above stays warm: it is a correctness check, not a
    # timing, so only the sampled timings carry the cold prepare.
    bench = Benchmark(
        warmup=args.warmup, count=args.samples, prepare=args.cold_prepare)
    try:
        index_res = bench.run(index_cmd)
        rr_read_res = bench.run(rr_read_cmd)
        rg_scan_res = bench.run(rg_scan_cmd)
    except PrepareError as e:
        eprint('error: %s' % e)
        eprint('Aborting: a failed cache drop would mislabel warm numbers as '
               'cold.')
        sys.exit(1)

    labels = ['rr index (one-time setup)', 'rr read (per query)',
              'rg scan (per query)']
    max_label = max(len(x) for x in labels)
    print('results:')
    print_row(labels[0], index_res, max_label)
    print_row(labels[1], rr_read_res, max_label)
    print_row(labels[2], rg_scan_res, max_label)
    print()

    k = crossover_k(index_res.mean, rr_read_res.mean, rg_scan_res.mean)
    print('crossover (amortized: rr_index + k*rr_read  vs  k*rg_scan):')
    if k is None:
        print('  rr read is not cheaper per query than rg scan on this corpus,')
        print('  so the one-time index never amortizes. rg wins at every k.')
    else:
        rr_total = index_res.mean + k * rr_read_res.mean
        rg_total = k * rg_scan_res.mean
        print('  k = %d queries: rr first wins.' % k)
        print('    at k=%d: rr total %.0f ms  vs  rg total %.0f ms'
              % (k, rr_total * 1000.0, rg_total * 1000.0))
        print('  Below k, rg\'s no-setup scan wins; at/above k, rr\'s indexed '
              'reads win.')

    if not ok:
        sys.exit(1)


if __name__ == '__main__':
    main()
