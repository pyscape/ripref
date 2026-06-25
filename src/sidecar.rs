/*!
The committed snapshot sidecar, `.rr/`.

Durable reference evidence lives here, separate from the derived index. It is a
dot-directory, so the `hidden(true)` index walk already excludes it (writing it
never disturbs the index or its freshness set), yet it is committed and shared
like the git-notes pattern: out-of-band data keyed by a content hash.

```text
.rr/refs                  manifest: append-only, committed, TAB-separated
.rr/objects/<aa>/<rest>   content store, sharded like .git/objects
```

We keep the frozen **bytes**, not a git pointer, so recovery is independent of
git GC: a `git show <commit>` reference rots when the commit is force-pushed or
rebased away, the stored bytes do not. An object is named by its git blob id
([`crate::blobhash::blob_oid`]), so recovery is self-verifying (re-hash the bytes,
check the name) and equal to git's own blob id for the unfiltered paths `cite`
accepts, which lets it double as the tracking baseline and dedup key.

This module owns the on-disk layout and parsing; policy (what to cite, when a
ref is broken) lives in [`crate::commands`].
*/

use std::path::{Path, PathBuf};

use crate::atomic;
use crate::blobhash;

/// Magic line pinning the manifest format version.
pub const MANIFEST_MAGIC: &str = "rr-refs v1";

/// The intent of a pin: a frozen snapshot, or a tracked (living) baseline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Snapshot,
    Track,
}

impl Kind {
    /// The manifest field tag for this kind (`"snapshot"` / `"track"`).
    pub fn tag(self) -> &'static str {
        match self {
            Kind::Snapshot => "snapshot",
            Kind::Track => "track",
        }
    }
}

/// One pinned reference: a snapshot or a tracking baseline, with the location it
/// was pinned at and the content id of the stored object.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pin {
    pub kind: Kind,
    pub anchor: String,
    pub short: String,
    pub path: String,
    pub start: u64,
    pub end: u64,
    pub oid: String,
}

/// An explicit un-cite / un-track marker, keyed by `(anchor, short)`. A removed
/// pin is a visible committed diff; a tomb is the sanctioned way to retire one,
/// so `verify` can tell a deliberate removal from a silent manifest deletion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tomb {
    pub anchor: String,
    pub short: String,
    pub reason: String,
}

/// One manifest line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Record {
    Pin(Pin),
    Tomb(Tomb),
}

/// The parsed manifest: every record, in file order (append-only, so file order
/// is chronological).
#[derive(Clone, Debug, Default)]
pub struct Manifest {
    pub records: Vec<Record>,
}

impl Manifest {
    /// Parse the manifest text. An empty input is an empty manifest; a present
    /// one must open with [`MANIFEST_MAGIC`]. A malformed line is an error, not a
    /// silent skip, so corruption fails closed.
    pub fn parse(text: &str) -> Result<Manifest, String> {
        let mut lines = text.lines();
        match lines.next() {
            None => return Ok(Manifest::default()),
            Some(magic) if magic == MANIFEST_MAGIC => {}
            Some(other) => {
                return Err(format!(
                    "unrecognized manifest format {other:?} (expected {MANIFEST_MAGIC:?})"
                ))
            }
        }
        let mut records = Vec::new();
        for line in lines {
            if line.is_empty() {
                continue;
            }
            records.push(parse_record(line)?);
        }
        Ok(Manifest { records })
    }

    /// Whether `(anchor, short)` has a tomb (was explicitly retired).
    pub fn is_tombed(&self, anchor: &str, short: &str) -> bool {
        self.records
            .iter()
            .any(|r| matches!(r, Record::Tomb(t) if t.anchor == anchor && t.short == short))
    }

    /// Every live (non-tombed) pin of `kind`.
    pub fn live_pins(&self, kind: Kind) -> Vec<&Pin> {
        self.records
            .iter()
            .filter_map(|r| match r {
                Record::Pin(p) if p.kind == kind => Some(p),
                _ => None,
            })
            .filter(|p| !self.is_tombed(&p.anchor, &p.short))
            .collect()
    }

    /// Resolve a `<anchor>` + `<commit>` to its pin of `kind`. The commit matches
    /// the stored short hash when either is a prefix of the other (the user
    /// typically passes back the exact short `cite` printed). Returns the latest
    /// matching pin, or an error describing not-found vs ambiguous so the caller
    /// can map both to BROKEN with a useful message.
    pub fn resolve(&self, kind: Kind, anchor: &str, commit: &str) -> Result<&Pin, ResolveError> {
        let matches: Vec<&Pin> = self
            .live_pins(kind)
            .into_iter()
            .filter(|p| p.anchor == anchor && commit_matches(&p.short, commit))
            .collect();
        match matches.as_slice() {
            [] => Err(ResolveError::NotFound),
            // Distinct commits matching one prefix is the ambiguous case; the
            // same (anchor, short) re-pinned just takes the most recent line.
            _ if matches.iter().any(|p| p.short != matches[0].short) => Err(
                ResolveError::Ambiguous(matches.iter().map(|p| p.short.clone()).collect()),
            ),
            _ => Ok(*matches.last().unwrap()),
        }
    }
}

/// Why [`Manifest::resolve`] could not return a unique pin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResolveError {
    NotFound,
    Ambiguous(Vec<String>),
}

/// Whether a stored short hash and a queried commit refer to the same commit:
/// equal, or one a hex prefix of the other.
fn commit_matches(stored: &str, queried: &str) -> bool {
    stored == queried || stored.starts_with(queried) || queried.starts_with(stored)
}

fn parse_record(line: &str) -> Result<Record, String> {
    let fields: Vec<&str> = line.split('\t').collect();
    let kind = match fields.first() {
        Some(&"snapshot") => Kind::Snapshot,
        Some(&"track") => Kind::Track,
        Some(&"tomb") => {
            if fields.len() != 4 {
                return Err(format!("malformed tomb record: {line:?}"));
            }
            return Ok(Record::Tomb(Tomb {
                anchor: fields[1].to_string(),
                short: fields[2].to_string(),
                reason: fields[3].to_string(),
            }));
        }
        _ => return Err(format!("unknown manifest record: {line:?}")),
    };
    if fields.len() != 6 {
        return Err(format!("malformed {} record: {line:?}", kind.tag()));
    }
    let (start, end) =
        parse_span(fields[4]).ok_or_else(|| format!("malformed span in {line:?}"))?;
    Ok(Record::Pin(Pin {
        kind,
        anchor: fields[1].to_string(),
        short: fields[2].to_string(),
        path: fields[3].to_string(),
        start,
        end,
        oid: fields[5].to_string(),
    }))
}

fn parse_span(s: &str) -> Option<(u64, u64)> {
    let (a, b) = s.split_once('-')?;
    Some((a.parse().ok()?, b.parse().ok()?))
}

/// Reject a manifest field value that contains the record/field separators, so a
/// pin can never silently corrupt the manifest (which would only surface as a
/// parse error on a later read).
fn ensure_no_separator(field: &str, value: &str) -> Result<(), String> {
    if value.contains('\t') || value.contains('\n') {
        return Err(format!(
            "{field} {value:?} contains a tab or newline, which the manifest format cannot represent"
        ));
    }
    Ok(())
}

/// The `.rr/` sidecar rooted under a working tree.
pub struct Sidecar {
    dir: PathBuf,
}

impl Sidecar {
    /// The sidecar at `<root>/.rr`.
    pub fn at(root: &Path) -> Sidecar {
        Sidecar {
            dir: root.join(".rr"),
        }
    }

    /// Path to the manifest file.
    pub fn refs_path(&self) -> PathBuf {
        self.dir.join("refs")
    }

    /// Sharded path to the object named `oid` (`objects/<aa>/<rest>`).
    pub fn object_path(&self, oid: &str) -> PathBuf {
        let (shard, rest) = oid.split_at(2.min(oid.len()));
        self.dir.join("objects").join(shard).join(rest)
    }

    /// Load and parse the manifest, treating an absent file as empty.
    pub fn load(&self) -> Result<Manifest, String> {
        match std::fs::read_to_string(self.refs_path()) {
            Ok(text) => Manifest::parse(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Manifest::default()),
            Err(e) => Err(format!(
                "failed to read {}: {e}",
                self.refs_path().display()
            )),
        }
    }

    /// Append one manifest line, rewriting the file atomically (read, append,
    /// atomic replace) so a crash never leaves a half-written record. Creates the
    /// file with its magic header on first write.
    fn append_line(&self, line: &str) -> Result<(), String> {
        std::fs::create_dir_all(&self.dir)
            .map_err(|e| format!("failed to create {}: {e}", self.dir.display()))?;
        let mut text = match std::fs::read_to_string(self.refs_path()) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => format!("{MANIFEST_MAGIC}\n"),
            Err(e) => {
                return Err(format!(
                    "failed to read {}: {e}",
                    self.refs_path().display()
                ))
            }
        };
        text.push_str(line);
        text.push('\n');
        atomic::atomic_write(&self.refs_path(), text.as_bytes())
            .map_err(|e| format!("failed to write {}: {e}", self.refs_path().display()))
    }

    /// Append a snapshot/track pin line.
    pub fn append_pin(&self, pin: &Pin) -> Result<(), String> {
        // Reject any field that would corrupt the TAB/newline-delimited record.
        // An anchor can in principle carry a tab (a markdown heading), so this is
        // a real guard, not a formality: refuse to write a malformed manifest
        // rather than produce one that fails to parse on the next read.
        ensure_no_separator("anchor", &pin.anchor)?;
        ensure_no_separator("path", &pin.path)?;
        ensure_no_separator("short", &pin.short)?;
        ensure_no_separator("oid", &pin.oid)?;
        self.append_line(&format!(
            "{}\t{}\t{}\t{}\t{}-{}\t{}",
            pin.kind.tag(),
            pin.anchor,
            pin.short,
            pin.path,
            pin.start,
            pin.end,
            pin.oid,
        ))
    }

    /// Append a tomb (explicit un-cite / un-track) line.
    pub fn append_tomb(&self, tomb: &Tomb) -> Result<(), String> {
        ensure_no_separator("anchor", &tomb.anchor)?;
        ensure_no_separator("short", &tomb.short)?;
        ensure_no_separator("reason", &tomb.reason)?;
        self.append_line(&format!(
            "tomb\t{}\t{}\t{}",
            tomb.anchor, tomb.short, tomb.reason
        ))
    }

    /// Store `bytes` as the object named `oid`, atomically. Content-addressed, so
    /// a re-store of identical content is a harmless no-op; we skip the rewrite
    /// when the object already exists.
    pub fn write_object(&self, oid: &str, bytes: &[u8]) -> Result<(), String> {
        let path = self.object_path(oid);
        if path.exists() {
            return Ok(());
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create {}: {e}", parent.display()))?;
        }
        atomic::atomic_write(&path, bytes)
            .map_err(|e| format!("failed to write {}: {e}", path.display()))
    }

    /// Read the object named `oid`, or `None` if it is absent.
    pub fn read_object(&self, oid: &str) -> Option<Vec<u8>> {
        std::fs::read(self.object_path(oid)).ok()
    }
}

/// A recovered object's content kind, so recovery never presents non-source as
/// source. `cite` already refuses LFS / filtered paths, so a pointer never
/// reaches here; the live distinction that matters is text vs binary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Content {
    Text,
    Binary,
}

/// Classify object bytes the way git decides "binary": a NUL in the first 8 KiB.
pub fn classify(bytes: &[u8]) -> Content {
    let window = &bytes[..bytes.len().min(8192)];
    if window.contains(&0) {
        Content::Binary
    } else {
        Content::Text
    }
}

/// The raw bytes of lines `start..=end` (1-based) of `bytes`, terminators
/// included, so a CRLF file recovers as CRLF (the stored working-tree form), not
/// normalized to LF. Out-of-range lines clamp to the available content.
pub fn slice_lines(bytes: &[u8], start: u64, end: u64) -> &[u8] {
    let line_start = |n: u64| -> usize {
        if n <= 1 {
            return 0;
        }
        let mut newlines = 0u64;
        for (i, &b) in bytes.iter().enumerate() {
            if b == b'\n' {
                newlines += 1;
                if newlines == n - 1 {
                    return i + 1;
                }
            }
        }
        bytes.len()
    };
    let s = line_start(start);
    let e = line_start(end.saturating_add(1)).max(s);
    &bytes[s..e]
}

/// The git blob id of `bytes` (the object-store name), re-exported so callers do
/// not reach into [`crate::blobhash`] directly.
pub fn oid_of(bytes: &[u8]) -> String {
    blobhash::blob_oid(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_roundtrips_pins_and_tombs() {
        let text = format!(
            "{MANIFEST_MAGIC}\n\
             snapshot\tBENCHMARKS.md#findings\teed9fe4\tBENCHMARKS.md\t34-83\ta1b2c3\n\
             track\tsrc/app.py\t0123abc\tsrc/app.py\t1-40\tdeadbeef\n\
             tomb\told\tcafef00\tsuperseded\n"
        );
        let m = Manifest::parse(&text).unwrap();
        assert_eq!(m.records.len(), 3);
        assert_eq!(m.live_pins(Kind::Snapshot).len(), 1);
        assert_eq!(m.live_pins(Kind::Track).len(), 1);
        assert!(m.is_tombed("old", "cafef00"));

        let pin = m
            .resolve(Kind::Snapshot, "BENCHMARKS.md#findings", "eed9fe4")
            .unwrap();
        assert_eq!(pin.path, "BENCHMARKS.md");
        assert_eq!((pin.start, pin.end), (34, 83));
        assert_eq!(pin.oid, "a1b2c3");
    }

    #[test]
    fn empty_manifest_is_ok_bad_magic_is_error() {
        assert!(Manifest::parse("").unwrap().records.is_empty());
        assert!(Manifest::parse("not a manifest\n").is_err());
        // A malformed record fails closed rather than being skipped.
        assert!(Manifest::parse(&format!("{MANIFEST_MAGIC}\nsnapshot\ttoo\tfew\n")).is_err());
    }

    #[test]
    fn resolve_reports_not_found_and_ambiguous() {
        let text = format!(
            "{MANIFEST_MAGIC}\n\
             snapshot\ta\t1111111\tp\t1-1\toidA\n\
             snapshot\ta\t2222222\tp\t1-1\toidB\n"
        );
        let m = Manifest::parse(&text).unwrap();
        assert!(matches!(
            m.resolve(Kind::Snapshot, "a", "9999999"),
            Err(ResolveError::NotFound)
        ));
        // A short common prefix matching two distinct commits is ambiguous.
        assert!(matches!(
            m.resolve(Kind::Snapshot, "a", ""),
            Err(ResolveError::Ambiguous(_))
        ));
        assert_eq!(
            m.resolve(Kind::Snapshot, "a", "1111111").unwrap().oid,
            "oidA"
        );
    }

    #[test]
    fn tomb_hides_a_pin_from_resolve() {
        let text = format!(
            "{MANIFEST_MAGIC}\n\
             snapshot\ta\t1111111\tp\t1-1\toidA\n\
             tomb\ta\t1111111\tgone\n"
        );
        let m = Manifest::parse(&text).unwrap();
        assert!(m.resolve(Kind::Snapshot, "a", "1111111").is_err());
        assert!(m.live_pins(Kind::Snapshot).is_empty());
    }

    #[test]
    fn object_path_is_sharded_like_git() {
        let sc = Sidecar::at(Path::new("/repo"));
        assert_eq!(
            sc.object_path("a1b2c3d4e5"),
            Path::new("/repo/.rr/objects/a1/b2c3d4e5")
        );
    }

    #[test]
    fn slice_lines_preserves_crlf_and_clamps() {
        let body = b"line1\r\nline2\r\nline3\r\n";
        // Lines 1..=2, CRLF terminators preserved verbatim.
        assert_eq!(slice_lines(body, 1, 2), b"line1\r\nline2\r\n");
        // Single middle line.
        assert_eq!(slice_lines(body, 2, 2), b"line2\r\n");
        // Past the end clamps to available content.
        assert_eq!(slice_lines(body, 3, 99), b"line3\r\n");
        // A file with no trailing newline still yields its last line.
        assert_eq!(slice_lines(b"a\nb", 2, 2), b"b");
    }

    #[test]
    fn classify_flags_nul_as_binary() {
        assert_eq!(classify(b"plain text\n"), Content::Text);
        assert_eq!(classify(b"with\0nul"), Content::Binary);
    }

    #[test]
    fn append_pin_rejects_separator_in_fields() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("rr-sep-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let sc = Sidecar::at(&root);
        let bad = Pin {
            kind: Kind::Snapshot,
            anchor: "head\tline".to_string(), // a tab would split into extra fields
            short: "abc1234".to_string(),
            path: "f.md".to_string(),
            start: 1,
            end: 1,
            oid: "deadbeef".to_string(),
        };
        let err = sc
            .append_pin(&bad)
            .expect_err("a tab in the anchor must be refused");
        assert!(err.contains("tab or newline"), "{err}");
        // And nothing was written: the manifest stays absent.
        assert!(!sc.refs_path().exists(), "a refused append writes nothing");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn object_store_roundtrips_bytes() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("rr-sidecar-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let sc = Sidecar::at(&root);
        let bytes = b"frozen content\n";
        let oid = oid_of(bytes);
        sc.write_object(&oid, bytes).unwrap();
        assert_eq!(sc.read_object(&oid).as_deref(), Some(&bytes[..]));
        // Re-store of identical content is a no-op, not an error.
        sc.write_object(&oid, bytes).unwrap();
        std::fs::remove_dir_all(&root).ok();
    }
}
