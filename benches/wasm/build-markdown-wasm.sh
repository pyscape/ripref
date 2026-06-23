#!/usr/bin/env bash
#
# Rebuild benches/wasm/tree-sitter-markdown.wasm — the third-party-path /
# benchmark artifact for benches/grammar_loader.rs (--features wasm). Normal
# builds and the native benchmark never need it; rerun only to refresh the
# committed .wasm (e.g. after bumping tree-sitter-md).
#
# It compiles the SAME grammar the native side uses — the `tree-sitter-md` crate
# — to a tree-sitter-loadable wasm module, with the wasi-sdk clang and the exact
# flags tree-sitter's own loader uses (tree-sitter-loader, compile_parser_to_wasm).
# `-DNDEBUG` drops assert()/__assert_fail (which isn't in tree-sitter's wasm
# stdlib); that's why no runtime shim is needed — the crate's scanner is plain C
# and every remaining import is in tree-sitter/src/wasm/stdlib-symbols.txt.
#
# Toolchain: the wasi-sdk clang that `tree-sitter build --wasm` downloads to
# %LOCALAPPDATA%\tree-sitter\wasi-sdk (or point TREE_SITTER_WASI_SDK_PATH at one).

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
out="$here/tree-sitter-markdown.wasm"

# tree-sitter-md crate sources from the Cargo registry (parser.c + scanner.c
# live under tree-sitter-markdown/src/). Highest version wins.
src="$(ls -d "$HOME"/.cargo/registry/src/*/tree-sitter-md-*/tree-sitter-markdown/src 2>/dev/null | sort -V | tail -1)"
if [ -z "${src:-}" ] || [ ! -f "$src/parser.c" ]; then
  echo "tree-sitter-md crate sources not found under ~/.cargo/registry; run 'cargo fetch' first." >&2
  exit 1
fi

sdk="${TREE_SITTER_WASI_SDK_PATH:-${LOCALAPPDATA:-$HOME/.cache}/tree-sitter/wasi-sdk}"
sdk="${sdk//\\//}"
clang="$sdk/bin/clang"
[ -x "$clang" ] || clang="$clang.exe"
if [ ! -x "$clang" ]; then
  echo "wasi-sdk clang not found under '$sdk'." >&2
  echo "Run 'tree-sitter build --wasm' once to fetch it, or set TREE_SITTER_WASI_SDK_PATH." >&2
  exit 1
fi

# cygpath -m yields Windows C:/... paths for the native clang on MSYS/Git Bash;
# a no-op elsewhere.
win() { cygpath -m "$1" 2>/dev/null || printf '%s' "$1"; }

"$clang" --target=wasm32-unknown-wasi \
  -o "$(win "$out")" \
  -fPIC -shared -Os -DNDEBUG \
  -Wl,--export=tree_sitter_markdown \
  -Wl,--allow-undefined \
  -Wl,--no-entry \
  -nostdlib -fno-exceptions -fvisibility=hidden \
  -I "$(win "$src")" \
  "$(win "$src/parser.c")" "$(win "$src/scanner.c")"

echo "wrote $out ($(stat -c %s "$out") bytes)"
echo "  from $src"
