/*!
Command implementations: the single writer (`index`) and one reader (`read`).

Each returns `Ok(exit_code)` for a normal outcome (including findings/stale,
which are non-zero but not errors) or `Err(message)` for a usage-level failure
the caller reports as exit `2`.
*/

use std::path::{Path, PathBuf};

use memmap2::Mmap;

use crate::cli::{self, LowArgs};
use crate::exit;
use crate::indexer;
use crate::refidx::{self, Reader};

/// `rr index` — build/refresh the index from the working tree.
pub fn run_index(args: &LowArgs) -> Result<u8, String> {
    let root = Path::new(".");
    let index_path = PathBuf::from(cli::index_path(args));

    let data = indexer::build(root, &index_path)
        .map_err(|e| format!("failed to walk the working tree: {e}"))?;
    let bytes = refidx::serialize(&data);

    if let Some(parent) = index_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create {}: {e}", parent.display()))?;
        }
    }
    std::fs::write(&index_path, &bytes)
        .map_err(|e| format!("failed to write {}: {e}", index_path.display()))?;

    if !args.quiet {
        println!(
            "indexed {} anchors across {} files",
            data.forward.len(),
            data.paths.len()
        );
    }
    Ok(exit::OK)
}

/// `rr read <anchor>` — dereference one anchor through the forward map.
pub fn run_read(args: &LowArgs) -> Result<u8, String> {
    let root = Path::new(".");
    let index_path = PathBuf::from(cli::index_path(args));
    let anchor = args.positional[0].to_string_lossy().into_owned();

    let file = match std::fs::File::open(&index_path) {
        Ok(f) => f,
        Err(_) => {
            eprintln!("no index at {} — run `rr index`", index_path.display());
            return Ok(exit::STALE);
        }
    };
    // SAFETY: the index is a regular file we just opened; concurrent writers go
    // through `rr index`, which writes a fresh file rather than mutating bytes.
    #[allow(unsafe_code)] // the one justified unsafe in the crate (posture: src/lib.rs)
    let mmap = unsafe { Mmap::map(&file) }
        .map_err(|e| format!("failed to mmap {}: {e}", index_path.display()))?;

    let reader = Reader::parse(&mmap).map_err(|e| format!("corrupt index: {e}"))?;

    // Freshness gates the answer: a reader exits 3 rather than answer stale.
    if indexer::newest_mtime(&reader.paths(), root) > reader.mtime {
        eprintln!("index is stale — rebuild with `rr index`");
        return Ok(exit::STALE);
    }

    let hits = reader.forward_lookup(&anchor);
    match hits.len() {
        0 => {
            eprintln!("no such anchor: {anchor}");
            Ok(exit::FINDINGS)
        }
        1 => {
            // Only the location line is printed today; the source-body print
            // and `-C`/`--context` are not yet implemented.
            println!("{}", hits[0]);
            Ok(exit::OK)
        }
        n => {
            eprintln!("ambiguous anchor: {anchor} resolves to {n} definitions");
            for loc in &hits {
                eprintln!("  {loc}");
            }
            eprintln!("(use `rr search` to see the collisions)");
            Ok(exit::FINDINGS)
        }
    }
}
