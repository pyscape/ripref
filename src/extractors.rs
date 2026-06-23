/*!
Anchor extractors: the [`Extractor`] trait and the built-in [`PathExtractor`].

Language-specific extraction lives in [`crate::languages`], keyed by file
extension in [`crate::languages::LANGUAGES`]. The indexer calls [`PathExtractor`]
unconditionally (every file gets a path anchor) and then consults that registry.
*/

use std::path::Path;

use crate::refidx::ForwardEntry;

/// The extraction contract: given a file, emit zero or more [`ForwardEntry`]
/// records. Implementations handle their own errors; unreadable or malformed
/// files should produce an empty result, not a panic.
pub trait Extractor: Sync {
    /// Return true if this extractor should run on files with this extension.
    /// `ext` is `None` for files without an extension.
    fn supports(&self, ext: Option<&str>) -> bool;
    /// Extract anchors from `disk_path`, prefixing locations with `rel_path`.
    ///
    /// Each returned [`ForwardEntry`] must set `location` to
    /// `"rel_path:start-end"` with 1-based line numbers (`start == end` for
    /// single-line anchors).
    fn extract(&self, rel_path: &str, disk_path: &Path) -> Vec<ForwardEntry>;
}

/// Emits one anchor per file: the path itself, spanning the whole file (`path:1-N`).
/// Run unconditionally by the indexer before the language registry.
pub struct PathExtractor;

impl Extractor for PathExtractor {
    fn supports(&self, _ext: Option<&str>) -> bool {
        true
    }

    fn extract(&self, rel_path: &str, disk_path: &Path) -> Vec<ForwardEntry> {
        let end = count_lines(disk_path).unwrap_or(0).max(1);
        vec![ForwardEntry {
            anchor: rel_path.to_string(),
            location: format!("{rel_path}:1-{end}"),
        }]
    }
}

fn count_lines(path: &Path) -> std::io::Result<u64> {
    let bytes = std::fs::read(path)?;
    let newlines = bytes.iter().filter(|&&b| b == b'\n').count() as u64;
    let trailing = if bytes.is_empty() || *bytes.last().unwrap() == b'\n' {
        0
    } else {
        1
    };
    Ok(newlines + trailing)
}
