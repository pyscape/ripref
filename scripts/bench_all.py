#!/usr/bin/env python3

'''
Run the whole rr benchmark suite on one machine and capture a reproducible,
machine-specific summary.

This is the REPRODUCE INSTRUMENT behind the hand-curated BENCHMARKS.md: it
gathers the host's specs (CPU, RAM, filesystem/tmpfs, AV, OS, toolchain), runs
every Cargo bench plus the allocation pin and the process-level e2e harness, and
writes a timestamped per-machine markdown summary alongside lossless raw stdout.
Medians and their jitter are read from criterion's own estimates.json (the same
robust source scripts/bench_regression.py uses), never scraped from stdout.

All output lands under --out (default bench-results/), which this script adds to
.gitignore: these artifacts are machine-specific and are NOT committed. Only the
curated BENCHMARKS.md, written by hand from these summaries, is committed.

Pure standard library, no pip dependencies, matching the repo's other Python
scripts (scripts/bench_e2e.py, scripts/bench_regression.py). ASCII only. Every
spec probe degrades to "unknown" rather than crashing, and one failing bench
never aborts the rest.
'''

import argparse
import json
import os
import os.path as path
import platform
import re
import socket
import subprocess
import sys
import tempfile
from datetime import datetime

_REPO_ROOT = path.dirname(path.dirname(path.abspath(__file__)))
CRITERION_ROOT = path.join(_REPO_ROOT, 'target', 'criterion')
IS_WINDOWS = os.name == 'nt'


def eprint(*args, **kwargs):
    'Like print, but to stderr.'
    kwargs['file'] = sys.stderr
    print(*args, **kwargs)


def _exe(name):
    'Append .exe on Windows so absolute-path lookups hit the real binary.'
    return name + '.exe' if IS_WINDOWS else name


# --- spec probes -----------------------------------------------------------
#
# Each probe returns a small dict and NEVER raises: a missing tool, a parse
# failure, or an unsupported platform yields "unknown" fields so a partial spec
# block is still written. subprocess calls are wrapped in _run_text, which
# swallows every error into "" so a probe is just string parsing.

def _run_text(cmd, shell=False):
    'Run a command and return its stdout text, or "" on any failure.'
    try:
        out = subprocess.run(
            cmd, shell=shell, stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL, check=False)
    except (OSError, ValueError):
        return ''
    try:
        return out.stdout.decode('utf-8', 'replace')
    except AttributeError:
        return ''


def _powershell(snippet):
    'Run a PowerShell snippet (-NoProfile) and return its stdout, or "".'
    return _run_text(
        ['powershell', '-NoProfile', '-Command', snippet], shell=False)


def cpu_specs():
    '''
    {model, physical_cores, logical_threads, max_mhz}, all "unknown" on failure.

    Linux parses lscpu's stable "Field: value" lines; physical core count is
    Core(s) per socket times Socket(s). Windows reads Win32_Processor as JSON
    (one object, or a list on a multi-socket box -- the first is reported).
    '''
    out = {'model': 'unknown', 'physical_cores': 'unknown',
           'logical_threads': 'unknown', 'max_mhz': 'unknown'}
    if IS_WINDOWS:
        text = _powershell(
            'Get-CimInstance Win32_Processor | '
            'Select Name,NumberOfCores,NumberOfLogicalProcessors,MaxClockSpeed | '
            'ConvertTo-Json')
        try:
            data = json.loads(text)
        except ValueError:
            return out
        if isinstance(data, list):
            data = data[0] if data else {}
        if data.get('Name'):
            out['model'] = str(data['Name']).strip()
        if data.get('NumberOfCores') is not None:
            out['physical_cores'] = str(data['NumberOfCores'])
        if data.get('NumberOfLogicalProcessors') is not None:
            out['logical_threads'] = str(data['NumberOfLogicalProcessors'])
        if data.get('MaxClockSpeed'):
            out['max_mhz'] = str(data['MaxClockSpeed'])
        return out
    # Linux / other POSIX: lscpu.
    text = _run_text(['lscpu'])
    fields = {}
    for line in text.splitlines():
        if ':' in line:
            k, v = line.split(':', 1)
            fields[k.strip()] = v.strip()
    if fields.get('Model name'):
        out['model'] = fields['Model name']
    if fields.get('CPU(s)'):
        out['logical_threads'] = fields['CPU(s)']
    cps = fields.get('Core(s) per socket')
    sockets = fields.get('Socket(s)')
    if cps and cps.isdigit():
        try:
            out['physical_cores'] = str(int(cps) * int(sockets or 1))
        except ValueError:
            out['physical_cores'] = cps
    mhz = fields.get('CPU max MHz') or fields.get('CPU MHz')
    if mhz:
        # lscpu prints e.g. "4800.0000"; keep an integer MHz.
        try:
            out['max_mhz'] = str(int(float(mhz)))
        except ValueError:
            out['max_mhz'] = mhz
    return out


def ram_total():
    'Total physical RAM as a human string (e.g. "31.9 GiB"), or "unknown".'
    if IS_WINDOWS:
        text = _powershell(
            '(Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory')
        digits = text.strip()
        if digits.isdigit():
            return _fmt_bytes(int(digits))
        return 'unknown'
    try:
        with open('/proc/meminfo', 'r', encoding='utf-8') as f:
            for line in f:
                if line.startswith('MemTotal:'):
                    # MemTotal is in kB.
                    kb = int(line.split()[1])
                    return _fmt_bytes(kb * 1024)
    except (OSError, ValueError, IndexError):
        pass
    return 'unknown'


def _fmt_bytes(n):
    'Bytes to a GiB/MiB string.'
    gib = n / (1024.0 ** 3)
    if gib >= 1.0:
        return '%.1f GiB' % gib
    return '%.0f MiB' % (n / (1024.0 ** 2))


def filesystem_specs():
    '''
    {repo_fs, temp_fs, temp_is_tmpfs, note} describing where I/O lands.

    Whether the temp dir is tmpfs is load-bearing: a tmpfs temp is RAM-backed,
    so benches that write a tree per iteration (index) see no real disk. On
    Windows there is no tmpfs; the report names NTFS and flags Defender's scan
    of every file write as the analogous perturbation.
    '''
    tempdir = tempfile.gettempdir()
    out = {'repo_fs': 'unknown', 'temp_fs': 'unknown',
           'temp_is_tmpfs': False, 'temp_dir': tempdir, 'note': ''}
    if IS_WINDOWS:
        out['repo_fs'] = _win_fs_type(_REPO_ROOT)
        out['temp_fs'] = _win_fs_type(tempdir)
        out['note'] = ('Windows has no tmpfs; temp is real disk. Defender '
                       'real-time scans every file write.')
        return out
    out['repo_fs'] = _findmnt_fstype(_REPO_ROOT)
    out['temp_fs'] = _findmnt_fstype(tempdir)
    out['temp_is_tmpfs'] = out['temp_fs'] == 'tmpfs'
    if out['temp_is_tmpfs']:
        out['note'] = 'temp dir is tmpfs (RAM-backed): index writes never hit disk.'
    return out


def _findmnt_fstype(target):
    'Filesystem type of the mount holding target (Linux), or "unknown".'
    text = _run_text(['findmnt', '-no', 'FSTYPE', '-T', target])
    return text.strip() or 'unknown'


def _win_fs_type(target):
    'Filesystem type (e.g. NTFS) of the drive holding target, or "unknown".'
    drive = path.splitdrive(path.abspath(target))[0]  # e.g. "C:"
    if not drive:
        return 'unknown'
    text = _powershell(
        "(Get-Volume -DriveLetter %s).FileSystemType" % drive.rstrip(':'))
    return text.strip() or 'unknown'


def av_status():
    'Windows Defender real-time status string; "n/a (Linux)" off Windows.'
    if not IS_WINDOWS:
        return 'n/a (no Windows Defender)'
    text = _powershell(
        '(Get-MpComputerStatus).RealTimeProtectionEnabled')
    val = text.strip().lower()
    if val == 'true':
        return 'Defender real-time protection: ON'
    if val == 'false':
        return 'Defender real-time protection: OFF'
    return 'Defender status: unknown'


def virt_status():
    '''
    Virtualization: the hypervisor name, "bare metal", or "unknown".

    This labels a run so a same-hardware VM comparison (a Windows guest and a
    Linux guest on one host) is unambiguous in the summary. Linux uses
    systemd-detect-virt (purpose-built). Windows matches the SMBIOS
    manufacturer/model against known hypervisor signatures, because
    HypervisorPresent reads true on ordinary bare-metal Windows (Hyper-V/VBS) and
    would false-positive.
    '''
    if IS_WINDOWS:
        text = _powershell(
            'Get-CimInstance Win32_ComputerSystem | '
            'Select Manufacturer,Model | ConvertTo-Json')
        try:
            data = json.loads(text)
        except ValueError:
            return 'unknown'
        blob = ('%s %s' % (data.get('Manufacturer', ''),
                           data.get('Model', ''))).lower()
        for sig in ('vmware', 'virtualbox', 'kvm', 'qemu', 'hyper-v',
                    'virtual machine', 'xen', 'parallels', 'bochs'):
            if sig in blob:
                return 'VM (%s)' % sig
        return 'bare metal (no known hypervisor signature)'
    text = _run_text(['systemd-detect-virt']).strip()
    if text == 'none':
        return 'bare metal'
    if text:
        return 'VM (%s)' % text
    return 'unknown'


def os_specs():
    'OS/kernel string; on Linux append /etc/os-release PRETTY_NAME.'
    base = '%s %s (%s)' % (platform.system(), platform.release(),
                           platform.machine())
    if not IS_WINDOWS:
        pretty = _os_release_pretty()
        if pretty:
            base = '%s -- %s' % (pretty, base)
    return base


def _os_release_pretty():
    'PRETTY_NAME from /etc/os-release, or "".'
    try:
        with open('/etc/os-release', 'r', encoding='utf-8') as f:
            for line in f:
                if line.startswith('PRETTY_NAME='):
                    return line.split('=', 1)[1].strip().strip('"')
    except OSError:
        pass
    return ''


def toolchain_specs():
    '{rustc, cargo} version strings, or "unknown".'
    rustc = _run_text([_exe('rustc'), '-vV'])
    cargo = _run_text([_exe('cargo'), '-V'])
    rustc_line = 'unknown'
    for line in rustc.splitlines():
        if line.startswith('rustc '):
            rustc_line = line.strip()
            break
    return {'rustc': rustc_line, 'cargo': cargo.strip() or 'unknown'}


def gather_specs():
    'All spec probes into one dict; each probe already degrades to unknown.'
    return {
        'host': socket.gethostname(),
        'cpu': cpu_specs(),
        'ram': ram_total(),
        'fs': filesystem_specs(),
        'av': av_status(),
        'virt': virt_status(),
        'os': os_specs(),
        'toolchain': toolchain_specs(),
    }


def specs_block(specs):
    'Render the spec dict as an ASCII markdown block.'
    cpu = specs['cpu']
    fs = specs['fs']
    lines = []
    lines.append('## Machine specs')
    lines.append('')
    lines.append('| field | value |')
    lines.append('| ----- | ----- |')
    lines.append('| host | %s |' % specs['host'])
    lines.append('| CPU | %s |' % cpu['model'])
    lines.append('| physical cores | %s |' % cpu['physical_cores'])
    lines.append('| logical threads | %s |' % cpu['logical_threads'])
    lines.append('| max clock (MHz) | %s |' % cpu['max_mhz'])
    lines.append('| RAM | %s |' % specs['ram'])
    lines.append('| OS | %s |' % specs['os'])
    lines.append('| repo filesystem | %s |' % fs['repo_fs'])
    lines.append('| temp dir | %s |' % fs['temp_dir'])
    lines.append('| temp filesystem | %s%s |' % (
        fs['temp_fs'], ' (tmpfs, RAM-backed)' if fs['temp_is_tmpfs'] else ''))
    lines.append('| anti-virus | %s |' % specs['av'])
    lines.append('| virtualization | %s |' % specs['virt'])
    lines.append('| rustc | %s |' % specs['toolchain']['rustc'])
    lines.append('| cargo | %s |' % specs['toolchain']['cargo'])
    if fs['note']:
        lines.append('')
        lines.append('Note: %s' % fs['note'])
    return '\n'.join(lines)


# --- bench registry --------------------------------------------------------
#
# One entry per logical bench. `argv` is the subprocess command (run from the
# repo root). `groups`, when set, are the criterion group prefixes whose
# estimates.json files feed the summary table; benches with no criterion output
# (alloc, e2e) carry groups=None and are reported as ran/failed only.

def bench_registry():
    'The ordered list of benches this suite runs.'
    return [
        {'name': 'freshness',
         'argv': ['cargo', 'bench', '--bench', 'freshness'],
         'groups': ['freshness']},
        {'name': 'index',
         'argv': ['cargo', 'bench', '--bench', 'index'],
         'groups': ['index']},
        {'name': 'query',
         'argv': ['cargo', 'bench', '--bench', 'query'],
         'groups': ['query']},
        {'name': 'freshness_scaling',
         'argv': ['cargo', 'bench', '--bench', 'freshness_scaling'],
         'groups': ['freshness_scaling']},
        {'name': 'grammar_loader',
         'argv': ['cargo', 'bench', '--bench', 'grammar_loader'],
         'groups': ['native']},
        {'name': 'grammar_loader_wasm',
         'argv': ['cargo', 'bench', '--bench', 'grammar_loader',
                  '--features', 'wasm'],
         'groups': ['wasm']},
        {'name': 'alloc',
         'argv': ['cargo', 'test', '--test', 'alloc'],
         'groups': None},
        # e2e is handled specially in run_suite (it needs corpus/rg paths and
        # may be skipped); kept out of this list so it does not run as a plain
        # subprocess with the wrong arguments.
    ]


def run_bench(entry, raw_dir, host):
    '''
    Run one bench, stream+capture its stdout to a raw file, return (ok, note).

    A non-zero exit is recorded, not raised: the wasm group commonly fails
    without the prebuilt artifact or a C toolchain, and the suite must keep
    going. stderr is folded into the captured stream so a failure's reason is
    preserved in the raw file.
    '''
    raw_path = path.join(raw_dir, '%s-%s.txt' % (host, entry['name']))
    try:
        completed = subprocess.run(
            entry['argv'], cwd=_REPO_ROOT,
            stdout=subprocess.PIPE, stderr=subprocess.STDOUT, check=False)
        text = completed.stdout.decode('utf-8', 'replace')
        rc = completed.returncode
    except OSError as e:
        text = 'failed to launch: %s\n' % e
        rc = -1
    with open(raw_path, 'w', encoding='utf-8', newline='\n') as f:
        f.write(text)
    if rc == 0:
        return True, 'ok'
    return False, 'exit %d (see raw)' % rc


def run_e2e(args, raw_dir, host):
    '''
    Run scripts/bench_e2e.py (warm, and cold if --cold given), or skip it.

    e2e needs a real corpus and a real rg binary; if either is missing this
    SKIPS with a clear note rather than failing the whole run -- it is the one
    bench that depends on artifacts the suite cannot synthesize.
    '''
    e2e_script = path.join(_REPO_ROOT, 'scripts', 'bench_e2e.py')
    corpus = args.e2e_corpus
    rg = args.rg
    if not corpus or not path.isdir(corpus):
        return 'skipped', 'no --e2e-corpus (or path missing): %s' % corpus
    if not rg or not path.isfile(rg):
        return 'skipped', 'no --rg (or path missing): %s' % rg
    base_cmd = [sys.executable, e2e_script, '--corpus', corpus, '--rg', rg]
    runs = [('warm', list(base_cmd))]
    if args.cold:
        runs.append(('cold', base_cmd + ['--cold-prepare', args.cold]))
    overall_ok = True
    chunks = []
    for label, cmd in runs:
        try:
            completed = subprocess.run(
                cmd, cwd=_REPO_ROOT, stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT, check=False)
            text = completed.stdout.decode('utf-8', 'replace')
            rc = completed.returncode
        except OSError as e:
            text = 'failed to launch: %s\n' % e
            rc = -1
        chunks.append('===== e2e (%s) exit %d =====\n%s' % (label, rc, text))
        if rc != 0:
            overall_ok = False
    raw_path = path.join(raw_dir, '%s-e2e.txt' % host)
    with open(raw_path, 'w', encoding='utf-8', newline='\n') as f:
        f.write('\n'.join(chunks))
    if overall_ok:
        return 'ran', 'warm%s' % (' + cold' if args.cold else '')
    return 'failed', 'one or more e2e runs exited non-zero (see raw)'


# --- criterion median extraction -------------------------------------------
#
# Identical robust source as scripts/bench_regression.py: read median (falling
# back to mean) and std_dev point estimates straight from estimates.json, keyed
# by the canonical full_id in the sibling benchmark.json. Never scrape stdout.

def read_estimate(estimates_path):
    'Return (median_ns, stddev_ns) from one estimates.json, or None.'
    try:
        with open(estimates_path, 'r', encoding='utf-8') as f:
            data = json.load(f)
    except (OSError, ValueError):
        return None
    median = None
    for key in ('median', 'mean'):
        stat = data.get(key)
        if isinstance(stat, dict) and isinstance(
                stat.get('point_estimate'), (int, float)):
            median = float(stat['point_estimate'])
            break
    if median is None:
        return None
    sd = data.get('std_dev')
    stddev = float(sd['point_estimate']) if isinstance(sd, dict) and isinstance(
        sd.get('point_estimate'), (int, float)) else 0.0
    return (median, stddev)


def collect_group(groups):
    '''
    {full_id: (median_ns, stddev_ns)} for benchmark leaves whose group prefix is
    in `groups`.

    A leaf is a directory holding BOTH estimates.json and benchmark.json (the
    same gate scripts/bench_regression.py uses to skip change/ and report/
    dirs). The group is the segment before the first '/' in the full_id; the
    wasm/native split lives there, which is why grammar_loader native and wasm
    can be told apart from the same target/criterion tree.
    '''
    results = {}
    want = set(groups)
    if not path.isdir(CRITERION_ROOT):
        return results
    for dirpath, dirnames, filenames in os.walk(CRITERION_ROOT):
        if 'estimates.json' not in filenames or 'benchmark.json' not in filenames:
            continue
        dirnames[:] = []
        try:
            with open(path.join(dirpath, 'benchmark.json'), 'r',
                      encoding='utf-8') as f:
                full_id = json.load(f).get('full_id')
        except (OSError, ValueError):
            full_id = None
        if not full_id:
            continue
        group = full_id.split('/', 1)[0]
        if group not in want:
            continue
        est = read_estimate(path.join(dirpath, 'estimates.json'))
        if est is None:
            continue
        results[full_id] = est
    return results


def fmt_ns(ns):
    'Human-friendly nanosecond formatting.'
    if ns >= 1e9:
        return '%.3f s' % (ns / 1e9)
    if ns >= 1e6:
        return '%.3f ms' % (ns / 1e6)
    if ns >= 1e3:
        return '%.3f us' % (ns / 1e3)
    return '%.1f ns' % ns


def cv_pct(median, stddev):
    'Coefficient of variation as a percent; 0 when the median is 0.'
    return (stddev / median * 100.0) if median > 0.0 else 0.0


def bench_table(entry):
    '''
    A markdown table of every criterion leaf for this bench (median + CV%).

    Returns the table text, or None when the bench has no criterion groups
    (alloc, e2e) or no estimates were found (e.g. the bench failed before
    producing output).
    '''
    if not entry.get('groups'):
        return None
    data = collect_group(entry['groups'])
    if not data:
        return None
    lines = ['| benchmark | median | CV%% (jitter) |'.replace('%%', '%'),
             '| --------- | ------ | ------------ |']
    for full_id in sorted(data):
        median, stddev = data[full_id]
        lines.append('| %s | %s | %.0f%% |' % (
            full_id, fmt_ns(median), cv_pct(median, stddev)))
    return '\n'.join(lines)


# --- gitignore -------------------------------------------------------------

def ensure_gitignored(out_dir):
    '''
    Append the out dir to .gitignore unless it is already ignored.

    Compares the literal trailing-slash form (e.g. "bench-results/"); if any
    .gitignore line already matches that pattern, nothing is written. The out
    dir is normalized to a repo-relative POSIX path so the pattern is portable.
    '''
    gitignore = path.join(_REPO_ROOT, '.gitignore')
    rel = path.relpath(path.abspath(out_dir), _REPO_ROOT).replace(os.sep, '/')
    pattern = rel.rstrip('/') + '/'
    existing = ''
    try:
        with open(gitignore, 'r', encoding='utf-8') as f:
            existing = f.read()
    except OSError:
        existing = ''
    for line in existing.splitlines():
        stripped = line.strip()
        if stripped in (pattern, pattern.rstrip('/'), rel):
            return False
    block = '\n# Machine-specific benchmark artifacts (bench_all.py); not\n' \
            '# committed -- the curated BENCHMARKS.md is.\n%s\n' % pattern
    sep = '' if existing.endswith('\n') or not existing else '\n'
    with open(gitignore, 'a', encoding='utf-8', newline='\n') as f:
        f.write(sep + block)
    return True


# --- orchestration ---------------------------------------------------------

def run_suite(args):
    'Gather specs, run the (unskipped) benches, write summary + raw, report.'
    out_dir = path.abspath(args.out)
    raw_dir = path.join(out_dir, 'raw')
    os.makedirs(raw_dir, exist_ok=True)
    gitignore_changed = ensure_gitignored(out_dir)

    specs = gather_specs()
    host = re.sub(r'[^A-Za-z0-9_.-]', '_', specs['host']) or 'host'
    stamp = datetime.now().strftime('%Y%m%d')

    skip = set(s.strip() for s in args.skip.split(',') if s.strip()) \
        if args.skip else set()

    ran, failed, skipped = [], [], []
    summary_sections = []

    for entry in bench_registry():
        name = entry['name']
        if name in skip:
            skipped.append(name)
            continue
        eprint('# running %s ...' % name)
        ok, note = run_bench(entry, raw_dir, host)
        (ran if ok else failed).append(name)
        table = bench_table(entry)
        sec = ['### %s%s' % (name, '' if ok else ' (FAILED: %s)' % note), '']
        if table:
            sec.append(table)
        elif entry.get('groups'):
            sec.append('(no criterion estimates found -- bench produced no '
                       'output)')
        else:
            sec.append('(no criterion output; %s)' % note)
        summary_sections.append('\n'.join(sec))

    # e2e is special: skip name "e2e", artifact-gated, not a plain cargo bench.
    if 'e2e' in skip:
        skipped.append('e2e')
        summary_sections.append('### e2e\n\n(skipped by --skip)')
    else:
        eprint('# running e2e ...')
        status, note = run_e2e(args, raw_dir, host)
        if status == 'ran':
            ran.append('e2e')
        elif status == 'failed':
            failed.append('e2e')
        else:
            skipped.append('e2e')
        summary_sections.append('### e2e (%s)\n\n(%s; see raw)' % (status, note))

    summary_path = path.join(out_dir, '%s-%s.md' % (host, stamp))
    write_summary(summary_path, specs, summary_sections, ran, failed, skipped,
                  args)

    print()
    print('specs: %s | %s cores / %s threads | %s RAM | %s' % (
        specs['cpu']['model'], specs['cpu']['physical_cores'],
        specs['cpu']['logical_threads'], specs['ram'], specs['os']))
    print('ran:     %s' % (', '.join(ran) or '(none)'))
    print('failed:  %s' % (', '.join(failed) or '(none)'))
    print('skipped: %s' % (', '.join(skipped) or '(none)'))
    print('summary: %s' % summary_path)
    print('raw:     %s' % raw_dir)
    if gitignore_changed:
        print('gitignore: added %s/' % path.relpath(out_dir, _REPO_ROOT).replace(
            os.sep, '/'))


def write_summary(summary_path, specs, sections, ran, failed, skipped, args):
    'Write the machine summary markdown (ASCII).'
    when = datetime.now().strftime('%Y-%m-%d %H:%M:%S')
    parts = []
    parts.append('# rr benchmark summary -- %s' % specs['host'])
    parts.append('')
    parts.append('Generated %s by scripts/bench_all.py. Machine-specific; not '
                 'committed.' % when)
    parts.append('Medians and CV%% (std_dev / median) are read from criterion '
                 'estimates.json.'.replace('%%', '%'))
    parts.append('')
    parts.append(specs_block(specs))
    parts.append('')
    parts.append('## Run status')
    parts.append('')
    parts.append('- ran: %s' % (', '.join(ran) or '(none)'))
    parts.append('- failed: %s' % (', '.join(failed) or '(none)'))
    parts.append('- skipped: %s' % (', '.join(skipped) or '(none)'))
    parts.append('')
    parts.append('## Benchmarks')
    parts.append('')
    parts.append('\n\n'.join(sections))
    parts.append('')
    with open(summary_path, 'w', encoding='utf-8', newline='\n') as f:
        f.write('\n'.join(parts))


def main():
    p = argparse.ArgumentParser(
        description='Gather this machine\'s specs and run the whole rr '
                    'benchmark suite, capturing a timestamped per-machine '
                    'summary. The reproduce instrument behind BENCHMARKS.md; '
                    'output is machine-specific and gitignored.')
    p.add_argument(
        '--skip', metavar='A,B', default='',
        help='Comma-separated bench names to skip. Names: freshness, index, '
             'query, freshness_scaling, grammar_loader, grammar_loader_wasm, '
             'alloc, e2e.')
    p.add_argument(
        '--e2e-corpus', metavar='PATH', dest='e2e_corpus', default=None,
        help='Corpus directory for scripts/bench_e2e.py. e2e is SKIPPED (not '
             'failed) if this is missing or absent on disk.')
    p.add_argument(
        '--rg', metavar='PATH', default=None,
        help='Absolute path to a genuine ripgrep binary for e2e. e2e is '
             'SKIPPED if missing.')
    p.add_argument(
        '--cold', metavar='CMD', default=None,
        help='Cold-cache prepare command passed through to bench_e2e.py as '
             '--cold-prepare (Linux only; see that script). Omit for warm-only '
             'e2e.')
    p.add_argument(
        '--out', metavar='DIR', default='bench-results',
        help='Output directory for the summary and raw stdout. Added to '
             '.gitignore. Default: %(default)s')
    args = p.parse_args()
    run_suite(args)


if __name__ == '__main__':
    main()
