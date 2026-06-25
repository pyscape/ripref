#!/usr/bin/env bash
# Run every local quality gate in one shot. Mirrors .github/workflows/ci.yml, so
# a green run here is a green run there. The six fast gates run by default; add
# the slow coverage gate (needs cargo-llvm-cov) with --cov.
#
# Usage: scripts/check.sh [--cov]
set -uo pipefail

cd "$(git rev-parse --show-toplevel)" || exit 2

run_cov=0
for arg in "$@"; do
  case "$arg" in
    --cov | --coverage) run_cov=1 ;;
    -h | --help)
      printf 'Usage: scripts/check.sh [--cov]\n'
      printf '  --cov  also run the coverage gate (cargo cov-gate)\n'
      exit 0
      ;;
    *)
      printf 'unknown argument: %s\n' "$arg" >&2
      exit 2
      ;;
  esac
done

failures=()

# run <label> <command...> : run a gate, record pass/fail, and keep going so one
# failure does not hide the others.
run() {
  local label="$1"
  shift
  printf '\n=== %s ===\n' "$label"
  if "$@"; then
    printf 'ok: %s\n' "$label"
  else
    printf 'FAIL: %s\n' "$label"
    failures+=("$label")
  fi
}

# require <tool> <hint> : record a missing prerequisite as a failure rather than
# crashing partway through the run.
require() {
  if command -v "$1" >/dev/null 2>&1; then
    return 0
  fi
  printf '\nFAIL: %s not found (%s)\n' "$1" "$2"
  failures+=("$1 missing")
  return 1
}

run "Format (cargo fmt-check)" cargo fmt-check
run "Lint (cargo lint)" cargo lint
run "Test (cargo test --all)" cargo test --all

# The citation grammar's canonical regex (`[[rr:scripts/citation_regex_oracle.py]]`) is the
# oracle the hand-rolled `citation::decode` is tested against. Re-assert its own
# contract table (reader, scanner, no-false-positives) here so a regex edit cannot
# silently change behavior. Skips cleanly when python3 is absent, like the
# python3/git-gated tests, rather than recording a failure.
if command -v python3 >/dev/null 2>&1; then
  run "Citation regex oracle (selfcheck)" python3 scripts/citation_regex_oracle.py --selfcheck
else
  printf '\nskip: Citation regex oracle (python3 not found)\n'
fi

if require rumdl "cargo install rumdl --locked"; then
  run "Markdown lint (rumdl check)" rumdl check .
  run "Markdown format (rumdl fmt --check)" rumdl fmt --check .
fi

# The ASCII gate inverts the usual convention: rg exits 0 when it FINDS a
# non-ASCII byte, which is the failure case. Same tool and glob as CI, so the
# verdict matches exactly; fixtures under tests/data are exempt.
if require rg "cargo install ripgrep (or build from your local checkout)"; then
  printf '\n=== Markdown ASCII-only ===\n'
  if hits=$(rg -n --column -g '*.md' -g '!tests/data/**' '[^\x00-\x7F]' .); then
    printf '%s\n' "$hits"
    printf 'FAIL: non-ASCII characters in Markdown (replace em dashes, curly quotes, etc.)\n'
    failures+=("Markdown ASCII-only")
  else
    printf 'ok: Markdown ASCII-only\n'
  fi
fi

if [ "$run_cov" -eq 1 ]; then
  if require cargo-llvm-cov "cargo install cargo-llvm-cov"; then
    run "Coverage (cargo cov-gate)" cargo cov-gate
  fi
fi

printf '\n========================================\n'
if [ "${#failures[@]}" -eq 0 ]; then
  printf 'All gates passed.\n'
else
  printf 'FAILED (%d): %s\n' "${#failures[@]}" "${failures[*]}"
  exit 1
fi
