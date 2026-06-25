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
ripgrep-thin. The rename gives POSIX replace semantics on Unix, and on Windows 10
1607+ [`std::fs::rename`] performs a POSIX-semantics rename (Rust 1.85+, via
`FILE_RENAME_FLAG_POSIX_SEMANTICS`), falling back to `MoveFileEx` on older systems,
so the swap is atomic on both. The parent-directory fsync that makes the rename
itself durable is a no-op where a directory cannot be opened as a file (Windows),
which is acceptable: the atomicity, not the post-crash durability of the directory
entry, is what readers depend on.
*/

use std::fs::{File, OpenOptions, Permissions};
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
///
/// When `path` already exists its permissions are carried onto the replacement
/// (Unix), so a rewrite preserves the file mode instead of resetting it to the
/// umask default.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };

    // Read the destination's mode before writing so we can carry it onto the
    // replacement; absent (new file, or non-Unix) the temp keeps the umask default.
    let perms = permissions_to_carry(path);
    let tmp = create_temp(dir, path)?;
    // From here on, clean up the temp file on any error before the rename
    // succeeds (after a successful rename there is nothing left to remove).
    if let Err(e) =
        write_and_sync(&tmp.path, bytes, perms).and_then(|()| std::fs::rename(&tmp.path, path))
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

/// A created temp file, held only by its `path`. There is no `Drop` guard, so
/// cleanup on the error path is explicit in [`atomic_write`] and a successful
/// rename consumes it. We reopen for writing so the handle's lifetime is scoped
/// to the write.
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

/// Truncate-write `bytes` to `path`, carry `perms` onto it when an existing file's
/// mode is being preserved, then fsync before the rename so the bytes (and the
/// carried mode) are on disk, not just in the page cache, when the rename
/// publishes them.
fn write_and_sync(path: &Path, bytes: &[u8], perms: Option<Permissions>) -> io::Result<()> {
    let mut file = OpenOptions::new().write(true).truncate(true).open(path)?;
    file.write_all(bytes)?;
    if let Some(perms) = perms {
        file.set_permissions(perms)?;
    }
    file.sync_all()
}

/// The permissions to carry onto a replacement so an overwrite preserves the
/// existing file's mode instead of resetting it to the umask default. Unix-only:
/// there `Permissions` is the full mode, whereas on Windows it is just the
/// read-only bit, and stamping that onto the temp could block the replace, so we
/// let the replacement take its default permissions instead.
#[cfg(unix)]
fn permissions_to_carry(target: &Path) -> Option<Permissions> {
    std::fs::metadata(target).map(|m| m.permissions()).ok()
}

#[cfg(not(unix))]
fn permissions_to_carry(_target: &Path) -> Option<Permissions> {
    None
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

    #[cfg(unix)]
    #[test]
    fn perm_preserved_on_overwrite() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tmp_dir("perm");
        let path = dir.join("index");
        atomic_write(&path, b"old").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();

        atomic_write(&path, b"new contents").unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"new contents");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "overwrite must preserve the existing mode");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn large_file_roundtrip() {
        let dir = tmp_dir("large");
        let path = dir.join("index");
        // A few MiB, deliberately not a page multiple, to catch any truncation or
        // short write across page boundaries.
        let n = 3 * 1024 * 1024 + 7;
        let data: Vec<u8> = (0..n).map(|i| (i % 251) as u8).collect();
        atomic_write(&path, &data).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), data);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn cleanup_on_rename_failure() {
        let dir = tmp_dir("rename-fail");
        // A non-empty directory at the target path: renaming a file onto it fails
        // (EISDIR), exercising the error path after the temp is created.
        let path = dir.join("index");
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("occupant"), b"x").unwrap();

        let err = atomic_write(&path, b"data").unwrap_err();

        assert_ne!(
            err.kind(),
            io::ErrorKind::NotFound,
            "expected a rename failure"
        );
        // The directory and its contents survive untouched.
        assert!(path.is_dir());
        assert_eq!(std::fs::read(path.join("occupant")).unwrap(), b"x");
        // The temp file was cleaned up on the error path.
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n != "index")
            .collect();
        assert!(leftovers.is_empty(), "stray temp files: {leftovers:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn concurrent_writers_never_tear() {
        let dir = tmp_dir("concurrent");
        let path = dir.join("index");

        // Each writer's payload is a distinct byte repeated a distinct length, so a
        // torn or interleaved write would show up as a wrong-length or mixed file.
        const WRITERS: usize = 8;
        let payloads: Vec<Vec<u8>> = (0..WRITERS)
            .map(|i| vec![b'a' + i as u8; 1000 + i * 333])
            .collect();

        let handles: Vec<_> = payloads
            .iter()
            .map(|payload| {
                // Clone per thread: the move closure needs an owned payload, and
                // `payloads` is reused for the contains-check below, so it can't
                // be consumed by the iterator.
                let payload = payload.clone();
                let path = path.clone();
                std::thread::spawn(move || atomic_write(&path, &payload).unwrap())
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }

        // The published file is exactly one writer's payload, never a mixture.
        let got = std::fs::read(&path).unwrap();
        assert!(
            payloads.contains(&got),
            "final file matched no single writer's payload (len {})",
            got.len()
        );
        // No temp files survive the race.
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
