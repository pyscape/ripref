/*!
The thin `git` shell-outs that `cite` / `track` / `verify` depend on.

These are the only places (besides the index freshness stamp in
[`crate::indexer`]) that `rr` runs `git`, and they run it by name on `PATH`,
never by absolute path or through libgit2, so a test can count invocations with
`GIT_TRACE2_EVENT` and so a sandboxed environment sees exactly the calls it
expects. Each helper is one process, returns `None` on any failure (not a git
repo, path not tracked, git absent), and leaves the policy decision to the
caller.

Why git and not our own object reading: a snapshot is gated on the *committed*
content of a path, and drift is measured against it, so the authoritative answer
is git's own view of the blob (it applies the same clean filter / EOL handling
git would). `cite` refuses filtered paths up front (see [`check_attr_filter`]),
so for everything we actually store, git's blob id equals
[`crate::blobhash::blob_oid`] of the working-tree bytes.
*/

use std::path::Path;
use std::process::Command;

/// Run `git -C root <args>` and return trimmed stdout when it exits zero.
fn run(root: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(
        String::from_utf8_lossy(&out.stdout)
            .trim_end_matches('\n')
            .to_string(),
    )
}

/// The blob id git would assign the **working-tree** file at `path` (applying any
/// clean filter), i.e. `git hash-object <path>`. This reads the file on disk, not
/// the index, so it sees edits even under `--skip-worktree`. Returns `None` if
/// the file is unreadable or git is unavailable.
pub fn hash_object(root: &Path, path: &str) -> Option<String> {
    run(root, &["hash-object", "--", path])
}

/// The blob id recorded for `path` at `HEAD`, i.e. `git rev-parse HEAD:<path>`.
/// `None` when the path is not tracked at `HEAD` (so "is this committed?" is
/// `rev_parse_head_blob(..) == Some(hash_object(..))`).
pub fn rev_parse_head_blob(root: &Path, path: &str) -> Option<String> {
    run(root, &["rev-parse", &format!("HEAD:{path}")])
}

/// The short `HEAD` commit hash, i.e. `git rev-parse --short HEAD`.
pub fn short_head(root: &Path) -> Option<String> {
    run(root, &["rev-parse", "--short", "HEAD"])
}

/// The `filter` attribute git resolves for `path` (the trailing word of
/// `git check-attr filter -- <path>`): `"unspecified"` when none applies, else
/// the filter name (`"lfs"`, a custom clean filter, ...). `None` only if git
/// could not be run.
pub fn check_attr_filter(root: &Path, path: &str) -> Option<String> {
    let line = run(root, &["check-attr", "filter", "--", path])?;
    // Output is `<path>: filter: <value>`; the value is the last colon field.
    line.rsplit(": ").next().map(|v| v.trim().to_string())
}

/// Whether `path` is subject to a clean filter or Git-LFS, which `cite` refuses:
/// LFS content lives off-repo and a clean filter's pre-clean bytes can re-leak
/// what the filter was meant to strip, so neither is safe to freeze verbatim.
pub fn is_filtered_path(root: &Path, path: &str) -> bool {
    match check_attr_filter(root, path) {
        Some(v) => !matches!(v.as_str(), "unspecified" | "unset" | ""),
        None => false, // not a git repo / git absent: the commit gate already failed
    }
}

/// The committed content of a tracked text file at `HEAD`, i.e.
/// `git show HEAD:<path>`. Used by `verify` to compare the working `.rr/refs`
/// against its committed form (fail-closed on a silent manifest deletion).
/// `None` when the path is not committed.
pub fn show_head_file(root: &Path, path: &str) -> Option<String> {
    run(root, &["show", &format!("HEAD:{path}")])
}
