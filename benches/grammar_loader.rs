//! Benchmarks for the two grammar-loading paths: a first-class language loaded
//! natively (the `tree-sitter-md` grammar crate, linked into the binary) vs a
//! third-party grammar loaded at runtime from a `.wasm` via tree-sitter's
//! wasmtime-backed `WasmStore`. Both measure the same three operations —
//! `language_init`, `query_compile`, `parse/throughput` — so the WASM path's
//! cost is directly comparable to native. Parser/store setup is hoisted out of
//! the parse loop in both, so `parse/throughput` measures parsing alone
//! (instantiation cost lives in `language_init`).
//!
//! Native runs by default:
//!   cargo bench --bench grammar_loader
//!
//! The WASM group is behind the `wasm` feature and needs the artifact at
//! `benches/wasm/tree-sitter-markdown.wasm` (regenerate it with
//! `benches/wasm/build-markdown-wasm.sh`):
//!   cargo bench --bench grammar_loader --features wasm
//!
//! Results land in `target/criterion/`.

use std::hint::black_box;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use streaming_iterator::StreamingIterator;
use tree_sitter::{Language, Parser, Query, QueryCursor};

// The markdown anchors query; matches the tree-sitter-md grammar used on both
// the native and wasm sides.
const QUERY_SOURCE: &str = "(atx_heading heading_content: (inline) @anchor)";

const SMALL_DOC: &str = "\
# Title

Some content.

## Section One

Content here.

### Subsection

More content.
";

const LARGE_DOC: &str = include_str!("../README.md");

// Both sides use the same grammar — the `tree-sitter-md` crate — so only the
// loading path differs. Native: the crate grammar linked in as a function
// pointer (the first-class path). WASM: the `.wasm` under benches/wasm/, built
// from the same crate sources (the third-party path). The correctness guard in
// the wasm group requires both to extract identical anchors.
fn make_language() -> Language {
    Language::new(tree_sitter_md::LANGUAGE)
}

/// Parse `src` and count `@anchor` captures — the per-document work both the
/// native and wasm parse benches measure. The parser must already have its
/// language (and, for wasm, its store) configured.
fn count_captures(parser: &mut Parser, query: &Query, src: &str) -> usize {
    let tree = parser.parse(src, None).unwrap();
    let mut cursor = QueryCursor::new();
    let mut hits = cursor.matches(query, tree.root_node(), src.as_bytes());
    let mut count = 0usize;
    while let Some(m) = hits.next() {
        count += m.captures.len();
    }
    count
}

// ---------------------------------------------------------------------------
// Native grammar loading
// ---------------------------------------------------------------------------

fn bench_native_language_init(c: &mut Criterion) {
    c.bench_function("native/language_init", |b| {
        b.iter(|| black_box(make_language()));
    });
}

fn bench_native_query_compile(c: &mut Criterion) {
    let language = make_language();
    c.bench_function("native/query_compile", |b| {
        b.iter(|| black_box(Query::new(&language, QUERY_SOURCE).unwrap()));
    });
}

fn bench_native_parse(c: &mut Criterion) {
    let language = make_language();
    let query = Query::new(&language, QUERY_SOURCE).unwrap();
    let mut parser = Parser::new();
    parser.set_language(&language).unwrap();

    let mut group = c.benchmark_group("native/parse");
    for (label, source) in [("small", SMALL_DOC), ("readme", LARGE_DOC)] {
        group.bench_with_input(BenchmarkId::new("throughput", label), source, |b, src| {
            b.iter(|| black_box(count_captures(&mut parser, &query, src)));
        });
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// WASM grammar loading (behind --features wasm)
//
// Mirrors the native benches one-for-one so the only difference measured is the
// grammar's origin: a `.wasm` loaded through tree-sitter's wasmtime-backed
// `WasmStore` instead of a statically-linked C function.
//
// A wasm `Language` is only valid while the `WasmStore` it was loaded from is
// alive, so every bench keeps that store alive (for `parse`, the store is moved
// into the parser, which then owns it). `WasmStore` does not expose wasmtime's
// `Module::serialize` AOT cache, so `wasm/language_init` is the *un-cached* cost
// — ~95% of which is the cranelift compile of the ~400 KB module (the rest is
// store setup). AOT caching would cut that to well under 1 ms; a cheaper
// `opt_level` does not help. See `examples/wasm_load_probe.rs` for the
// decomposition and the cached ceiling.
// ---------------------------------------------------------------------------

#[cfg(feature = "wasm")]
mod wasm_bench {
    use super::*;
    use tree_sitter::{wasmtime::Engine, WasmStore};

    const WASM_BYTES: &[u8] = include_bytes!("wasm/tree-sitter-markdown.wasm");

    // The wasm loader resolves the grammar's `tree_sitter_<name>` export, so the
    // name must match the native grammar's symbol (`tree_sitter_markdown`).
    const GRAMMAR_NAME: &str = "markdown";

    pub fn language_init(c: &mut Criterion) {
        let engine = Engine::default();
        c.bench_function("wasm/language_init", |b| {
            b.iter(|| {
                let mut store = WasmStore::new(&engine).unwrap();
                // The returned Language drops at the end of this statement,
                // before `store` — so it never outlives the store it came from.
                black_box(store.load_language(GRAMMAR_NAME, WASM_BYTES).unwrap());
            });
        });
    }

    pub fn query_compile(c: &mut Criterion) {
        let engine = Engine::default();
        let mut store = WasmStore::new(&engine).unwrap();
        let language = store.load_language(GRAMMAR_NAME, WASM_BYTES).unwrap();
        // `store` is held for the whole bench so `language` stays valid.
        c.bench_function("wasm/query_compile", |b| {
            b.iter(|| black_box(Query::new(&language, QUERY_SOURCE).unwrap()));
        });
    }

    pub fn parse(c: &mut Criterion) {
        let engine = Engine::default();
        let mut store = WasmStore::new(&engine).unwrap();
        let language = store.load_language(GRAMMAR_NAME, WASM_BYTES).unwrap();
        let query = Query::new(&language, QUERY_SOURCE).unwrap();
        let mut parser = Parser::new();
        // The parser takes ownership of the store that loaded `language`, which
        // keeps `language` valid for as long as the parser lives.
        parser.set_wasm_store(store).unwrap();
        parser.set_language(&language).unwrap();

        // Correctness guard: a silently-broken `.wasm` (wrong/stale grammar,
        // zero matches) would produce fast but meaningless timings. Require the
        // wasm grammar to extract the same number of anchors as native before
        // trusting any number below it.
        {
            let native_language = make_language();
            let native_query = Query::new(&native_language, QUERY_SOURCE).unwrap();
            let mut native_parser = Parser::new();
            native_parser.set_language(&native_language).unwrap();
            assert_eq!(
                count_captures(&mut parser, &query, SMALL_DOC),
                count_captures(&mut native_parser, &native_query, SMALL_DOC),
                "wasm grammar must extract the same anchors as native",
            );
        }

        let mut group = c.benchmark_group("wasm/parse");
        for (label, source) in [("small", SMALL_DOC), ("readme", LARGE_DOC)] {
            group.bench_with_input(BenchmarkId::new("throughput", label), source, |b, src| {
                b.iter(|| black_box(count_captures(&mut parser, &query, src)));
            });
        }
        group.finish();
    }
}

criterion_group!(
    native,
    bench_native_language_init,
    bench_native_query_compile,
    bench_native_parse,
);

#[cfg(feature = "wasm")]
criterion_group!(
    wasm,
    wasm_bench::language_init,
    wasm_bench::query_compile,
    wasm_bench::parse,
);

#[cfg(feature = "wasm")]
criterion_main!(native, wasm);

#[cfg(not(feature = "wasm"))]
criterion_main!(native);
