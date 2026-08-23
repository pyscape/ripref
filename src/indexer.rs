/*!
The writer side of `rr index`: walk the working tree and produce
[`IndexData`]: every anchor's definitions, and every path mention in scoped
text `[[rr:AD-5]]`.

The walk reuses ripgrep's `ignore` crate, so `.gitignore` and hidden-file
rules match rr.toml's defaults (`respect-gitignore = true`, `hidden = false`)
for free. Per-file anchor extraction is delegated to the
`languages` registry; mention scanning runs only over files the
profile's scan scope selects. The walker itself is type-blind.
*/

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::time::{SystemTime, UNIX_EPOCH};

use ignore::overrides::Override;
use ignore::{WalkBuilder, WalkState};

use crate::config;
use crate::languages;
use crate::messages;
use crate::refidx::{ForwardEntry, IndexData, MentionEntry};
use crate::scan::{self, What};

/// What one worker produces for one file.
struct FileRecords {
    anchors: Vec<ForwardEntry>,
    mentions: Vec<MentionEntry>,
    path: String,
}

/// Walk `root` and build the index contents. `index_path` is excluded so the
/// index never indexes (or freshness-checks against) itself; `scope` selects
/// the files whose text is scanned for mentions.
///
/// The walk runs in parallel: reading and parsing each file is the bulk of
/// `rr index`, and the files are independent, so it fans them across cores
/// with `ignore`'s `build_parallel` and collects per-file results over an
/// `mpsc` channel. Order is not preserved (workers finish in scheduling
/// order), but it need not be: [`crate::refidx::serialize`] sorts to a total
/// order, so the on-disk image is identical regardless of the order records
/// arrive in.
///
/// Reports a failed read or walk entry rather than returning one
/// (`[[rr:error]]`).
pub fn build(root: &Path, index_path: &Path, scope: &Override, cfg: &config::Config) -> IndexData {
    let index_rel = index_path
        .strip_prefix(root)
        .unwrap_or(index_path)
        .to_path_buf();

    let (tx, rx) = mpsc::channel::<FileRecords>();

    // Skipping hidden files also skips `.git/` and a dot-prefixed index dir
    // like `.ref-cache/`. `ignore`'s own `sort_by_file_path` is serial-only
    // and would defeat the point of the parallel walk; ordering is recovered
    // by serialize's sort instead.
    let walker = WalkBuilder::new(root)
        .hidden(true)
        .git_ignore(true)
        .git_exclude(true)
        .parents(true)
        .build_parallel();

    let index_rel: &Path = &index_rel;
    walker.run(|| {
        let tx = tx.clone();
        Box::new(move |result| {
            let dent = match result {
                Ok(d) => d,
                Err(e) => {
                    messages::error(e);
                    return WalkState::Continue;
                }
            };
            if !dent.file_type().is_some_and(|t| t.is_file()) {
                return WalkState::Continue;
            }
            let rel = dent.path().strip_prefix(root).unwrap_or(dent.path());
            if rel == index_rel {
                return WalkState::Continue;
            }
            let rel_path = to_unix(rel);
            let ext = rel.extension().and_then(|e| e.to_str());

            let mut anchors = Vec::new();
            let mut mentions = Vec::new();
            let language = languages::for_extension(ext);
            let in_scope = config::in_scope(scope, &rel_path);
            // One read feeds both halves, so a failure is reported once.
            if language.is_some() || in_scope {
                match std::fs::read_to_string(dent.path()) {
                    Ok(content) => {
                        if let Some(language) = language {
                            anchors = language.extract_from_str(&rel_path, &content);
                        }
                        if in_scope {
                            for found in scan::scan(&content, scan::host_for(ext, cfg)) {
                                if let What::Mention { token, .. } = found.what {
                                    mentions.push(MentionEntry {
                                        token,
                                        location: format!(
                                            "{rel_path}:{}-{}",
                                            found.line, found.line
                                        ),
                                    });
                                }
                            }
                        }
                    }
                    // A binary file fails as InvalidData: nothing to anchor,
                    // and not scoped text.
                    Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {}
                    Err(e) => messages::error(format_args!("{rel_path}: {e}")),
                }
            }

            // The receiver outlives the walk, so this only errs if it
            // panicked; dropping the file's work is the right thing then.
            let _ = tx.send(FileRecords {
                anchors,
                mentions,
                path: rel_path,
            });
            WalkState::Continue
        })
    });
    // Close the channel so the drain below terminates: every worker's clone
    // is dropped when `run` returns, leaving only this original sender.
    drop(tx);

    let mut forward = Vec::new();
    let mut mentions = Vec::new();
    let mut paths = Vec::new();
    for records in rx {
        forward.extend(records.anchors);
        mentions.extend(records.mentions);
        paths.push(records.path);
    }

    // Sort into the canonical total order here, so the returned `IndexData`
    // is deterministic regardless of the scheduling-dependent order the
    // parallel walk produced. `serialize` re-asserts this order, but on
    // already-sorted input its adaptive sort is ~O(n), so the work is done
    // once, not twice.
    forward.sort_by(|a, b| {
        a.anchor
            .cmp(&b.anchor)
            .then_with(|| a.location.cmp(&b.location))
    });
    mentions.sort_by(|a, b| {
        a.token
            .cmp(&b.token)
            .then_with(|| a.location.cmp(&b.location))
    });
    paths.sort();

    let mtime = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    IndexData {
        mtime,
        tree: git_tree(root),
        forward,
        mentions,
        paths,
    }
}

/// `[[rr:AD-1#Decision outcome]]`
fn to_unix(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// `[[rr:ripref (rr)#Freshness]]`
pub(crate) fn git_tree(root: &Path) -> String {
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

/// `[[rr:ripref (rr)#Freshness]]`
/// The reduction is an order-independent `max` over disjoint chunks, so the
/// parallel result is identical to the serial one.
pub fn newest_mtime(paths: &[&str], root: &Path) -> u64 {
    // [[rr:Findings that hold on both platforms]]
    const PARALLEL_THRESHOLD: usize = 256;
    let n = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(paths.len());
    if n <= 1 || paths.len() < PARALLEL_THRESHOLD {
        return newest_serial(paths, root);
    }
    let chunk = paths.len().div_ceil(n);
    std::thread::scope(|s| {
        paths
            .chunks(chunk)
            .map(|c| s.spawn(move || newest_serial(c, root)))
            .collect::<Vec<_>>()
            .into_iter()
            .filter_map(|h| h.join().ok())
            .max()
            .unwrap_or(0)
    })
}

/// Kept standalone (rather than inlined into `newest_mtime`) so the
/// parallel and serial reductions can be tested for equality.
fn newest_serial(paths: &[&str], root: &Path) -> u64 {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The parallel `newest_mtime` must agree with the serial reducer for any
    /// filesystem state. Generating more than `PARALLEL_THRESHOLD` files
    /// forces the chunked thread::scope path (not the serial fallback), so
    /// equality here proves the parallel reduction preserves the result.
    #[test]
    fn parallel_equals_serial() {
        let dir = std::env::temp_dir().join(format!(
            "rr-newest-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        let names: Vec<String> = (0..300).map(|i| format!("f{i}.txt")).collect();
        for name in &names {
            std::fs::write(dir.join(name), b"x").unwrap();
        }
        // Include a path that does not exist: it must contribute 0 and be
        // ignored identically by both reducers.
        let mut paths: Vec<&str> = names.iter().map(String::as_str).collect();
        paths.push("does-not-exist.txt");
        assert!(paths.len() > 256, "must exceed the parallel threshold");

        let parallel = newest_mtime(&paths, &dir);
        let serial = newest_serial(&paths, &dir);
        assert_eq!(parallel, serial);
        assert!(parallel > 0, "freshly written files have a real mtime");

        std::fs::remove_dir_all(&dir).ok();
    }
}
