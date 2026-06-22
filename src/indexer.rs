/*!
The writer side of `rr index`: walk the working tree and produce [`IndexData`].

For now this extracts only `path` anchors — a file path is its own stable
identity, so its forward entry is the path → whole-file span (`path:1-N`), and
the same paths populate the `paths` membership section. The walk reuses
ripgrep's `ignore` crate, so `.gitignore` and hidden-file rules match rr.toml's
defaults (`respect-gitignore = true`, `hidden = false`) for free.
*/

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use ignore::WalkBuilder;

use crate::refidx::{ForwardEntry, IndexData};

/// Walk `root` and build the index contents. `index_path` is excluded so the
/// index never indexes (or freshness-checks against) itself.
pub fn build(root: &Path, index_path: &Path) -> std::io::Result<IndexData> {
    let index_rel = index_path
        .strip_prefix(root)
        .unwrap_or(index_path)
        .to_path_buf();

    let mut forward = Vec::new();
    let mut paths = Vec::new();

    // Defaults mirror rr.toml: respect ignore files, skip hidden (which also
    // skips `.git/` and a dot-prefixed index dir like `.ref-cache/`).
    let walker = WalkBuilder::new(root)
        .hidden(true)
        .git_ignore(true)
        .git_exclude(true)
        .parents(true)
        .build();

    for result in walker {
        let dent = match result {
            Ok(d) => d,
            Err(_) => continue, // skip unreadable entries rather than aborting the whole walk
        };
        if !dent.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let rel = dent.path().strip_prefix(root).unwrap_or(dent.path());
        if rel == index_rel {
            continue;
        }
        let anchor = to_unix(rel);
        let end = count_lines(dent.path())?.max(1);
        let location = format!("{anchor}:1-{end}");
        forward.push(ForwardEntry {
            anchor: anchor.clone(),
            location,
        });
        paths.push(anchor);
    }

    // Sort forward for the reader's binary search; sort paths for deterministic output.
    forward.sort_by(|a, b| a.anchor.cmp(&b.anchor));
    paths.sort();

    let mtime = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    Ok(IndexData {
        mtime,
        tree: git_tree(root),
        forward,
        paths,
    })
}

/// Repo-relative path as forward-slash text, so an index is portable across
/// Windows and Unix.
fn to_unix(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Count content lines: newline count, plus one for a final unterminated line.
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

/// The HEAD tree SHA when the working tree is clean, else empty. `index` is the
/// only command that runs git, and only to stamp the index for provenance.
fn git_tree(root: &Path) -> String {
    let clean = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["status", "--porcelain"])
        .output()
        .map(|o| o.status.success() && o.stdout.is_empty())
        .unwrap_or(false);
    if !clean {
        return String::new();
    }
    Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "HEAD^{tree}"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// The newest mtime (Unix seconds) among `paths`, resolved relative to `root`.
/// This is the entire freshness computation: one `stat` per in-scope file, no
/// git, no hashing.
pub fn newest_mtime(paths: &[&str], root: &Path) -> u64 {
    let mut newest = 0u64;
    for p in paths {
        let full: PathBuf = root.join(p);
        if let Ok(secs) = std::fs::metadata(&full)
            .and_then(|m| m.modified())
            .map(|m| {
                m.duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0)
            })
        {
            newest = newest.max(secs);
        }
    }
    newest
}
