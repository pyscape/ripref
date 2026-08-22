/*!
Anchor extractors: the [`Extractor`] trait and the built-in [`PathExtractor`].

Language-specific extraction lives in [`crate::languages`], keyed by file
extension in [`crate::languages::LANGUAGES`]. The indexer calls [`PathExtractor`]
unconditionally (every file gets a path anchor) and then consults that registry.
*/

use std::path::Path;

use crate::refidx::ForwardEntry;

/// Implementations handle their own errors; unreadable or malformed files
/// should produce an empty result, not a panic.
pub trait Extractor: Sync {
    fn supports(&self, ext: Option<&str>) -> bool;
    /// `[[rr:AD-1#Decision outcome]]`
    fn extract(&self, rel_path: &str, disk_path: &Path) -> Vec<ForwardEntry>;
}

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
