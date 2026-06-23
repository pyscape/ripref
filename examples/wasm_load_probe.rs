//! Diagnostic: decompose the wasm grammar-load cost and bound what AOT caching
//! or a cheaper compiler setting would save. Throwaway — informs the
//! native-vs-WASM decision; not part of the benchmark proper.
//!
//!   cargo run --release --example wasm_load_probe --features wasm

#[cfg(not(feature = "wasm"))]
fn main() {
    eprintln!("rebuild with --features wasm");
}

#[cfg(feature = "wasm")]
fn main() {
    use std::time::Instant;
    use tree_sitter::wasmtime::{Config, Engine, Module, OptLevel};
    use tree_sitter::WasmStore;

    const WASM: &[u8] = include_bytes!("../benches/wasm/tree-sitter-markdown.wasm");

    fn bench(label: &str, iters: u32, mut f: impl FnMut()) {
        f(); // warmup
        let t = Instant::now();
        for _ in 0..iters {
            f();
        }
        println!(
            "{:<40} {:>12?}   (avg of {})",
            label,
            t.elapsed() / iters,
            iters
        );
    }

    let engine = Engine::default(); // default = Cranelift, opt_level = Speed

    // What `wasm/language_init` in the bench actually measures, and its parts:
    bench("WasmStore::new (store setup only)", 100, || {
        WasmStore::new(&engine).unwrap();
    });
    bench("WasmStore::new + load_language", 10, || {
        let mut store = WasmStore::new(&engine).unwrap();
        store.load_language("markdown", WASM).unwrap();
    });

    // The dominant cost is the cranelift compile of the module:
    bench("Module::new  (compile, opt=Speed)", 10, || {
        Module::new(&engine, WASM).unwrap();
    });

    // AOT ceiling: compile once, cache the machine code, reload it. This is what
    // wasmtime's Module::serialize/deserialize buys — but tree-sitter's
    // WasmStore does not expose it, so the bench can't use it today.
    let blob = Module::new(&engine, WASM).unwrap().serialize().unwrap();
    println!("serialized module blob: {} KB", blob.len() / 1024);
    bench("Module::deserialize (AOT cached load)", 100, || {
        // SAFETY: blob came from Module::serialize on this same engine.
        unsafe { Module::deserialize(&engine, &blob).unwrap() };
    });

    // Cheaper-compiler lever, available today via the Engine we pass to
    // WasmStore: trade execution speed for compile speed.
    let mut cfg = Config::new();
    cfg.cranelift_opt_level(OptLevel::None);
    let engine_fast = Engine::new(&cfg).unwrap();
    bench("Module::new  (compile, opt=None)", 10, || {
        Module::new(&engine_fast, WASM).unwrap();
    });
}
