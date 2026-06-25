/*!
Crash-safe file replacement: write to a temp file in the same directory, fsync
it, then atomically rename it over the destination.

`rr` rewrites whole files in place (the index, and the sidecar manifest/objects).
A plain `std::fs::write` truncates the destination and streams new bytes into it,
so a crash or a concurrent reader can observe a half-written file (or, for an
mmap'd reader, a SIGBUS). [`atomic_write`] removes that window: a reader always
sees either the complete old file or the complete new one.

This is the [`std`]-only equivalent of the `atomic-write-file` crate's core
(temp-in-same-dir + fsync + rename), hand-rolled to keep the dependency tree
ripgrep-thin. The rename gives POSIX replace semantics on Unix, and on Windows
[`std::fs::rename`] maps to a replacing `MoveFileEx`, so the swap is atomic on
both. The parent-directory fsync that makes the rename itself durable is a no-op
where a directory cannot be opened as a file (Windows), which is acceptable: the
atomicity, not the post-crash durability of the directory entry, is what readers
depend on.
*/

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Per-process counter making temp names unique without an rng dependency; the
/// pid keeps them unique across processes, the counter within one.
static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Atomically replace `path` with `bytes`.
///
/// Writes a sibling temp file, fsyncs its contents, renames it onto `path`, then
/// best-effort fsyncs the parent directory. On any failure before the rename the
/// temp file is cleaned up, so a failed write never litters the directory.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };

    let tmp = create_temp(dir, path)?;
    // From here on, clean up the temp file on any error before the rename
    // succeeds (after a successful rename there is nothing left to remove).
    if let Err(e) = write_and_sync(&tmp.path, bytes).and_then(|()| std::fs::rename(&tmp.path, path))
    {
        let _ = std::fs::remove_file(&tmp.path);
        return Err(e);
    }

    // Make the rename itself durable. Opening a directory as a file is not
    // supported everywhere (notably Windows); a failure here costs only
    // post-crash durability of the directory entry, not atomicity, so ignore it.
    if let Ok(dir_file) = File::open(dir) {
        let _ = dir_file.sync_all();
    }
    Ok(())
}

/// A temp file removed on drop unless [`TempFile::path`] was renamed away first.
/// Only the path is retained; we reopen for writing so the handle's lifetime is
/// scoped to the write.
struct TempFile {
    path: PathBuf,
}

/// Create a uniquely named, empty temp file in `dir` next to `target`, retrying
/// on the vanishingly unlikely name collision. The leading dot keeps it out of
/// the way; the `target` file name anchors it so concurrent writes to different
/// files in one directory never contend for the same temp name.
fn create_temp(dir: &Path, target: &Path) -> io::Result<TempFile> {
    let stem = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "index".to_string());
    let pid = std::process::id();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    loop {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = dir.join(format!(".{stem}.tmp.{pid}.{nanos}.{n}"));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(_) => return Ok(TempFile { path }),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
}

/// Truncate-write `bytes` to `path` and fsync the file before it is renamed, so
/// the bytes are on disk (not just in the page cache) when the rename publishes
/// them.
fn write_and_sync(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new().write(true).truncate(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("rr-atomic-{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn writes_new_file() {
        let dir = tmp_dir("new");
        let path = dir.join("index");
        atomic_write(&path, b"hello").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"hello");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn replaces_existing_and_leaves_no_temp() {
        let dir = tmp_dir("replace");
        let path = dir.join("index");
        atomic_write(&path, b"old contents").unwrap();
        atomic_write(&path, b"new").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        // No stray temp files survive a successful write.
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n != "index")
            .collect();
        assert!(leftovers.is_empty(), "stray temp files: {leftovers:?}");
        std::fs::remove_dir_all(&dir).ok();
    }
}
