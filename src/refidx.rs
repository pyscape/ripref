/*!
The on-disk index format, `refidx v1`.

Flat, sorted, newline-terminated UTF-8 records, with a self-describing section
table. The writer ([`serialize`]) produces the bytes; the reader ([`Reader`])
borrows an mmap'd `&[u8]` and binary-searches a section in place. The writer
populates the `forward` and `paths` sections from `path` anchors today, and
writes an empty-but-present `reverse` section: the section table must list every
core section even when it is zero-length, so the reader can locate each by name.

A format change must bump `MAGIC`, so an old reader rejects a new index rather
than misparsing it.
*/

use std::collections::HashMap;

/// Magic line pinning the format version; [`Reader::parse`] rejects any other value.
pub const MAGIC: &str = "refidx v1";

/// One forward-map entry: an anchor and the location it defines.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForwardEntry {
    pub anchor: String,
    /// The location body, exactly as printed: `file:start-end`.
    pub location: String,
}

/// The logical contents of an index, before/after (de)serialization.
#[derive(Clone, Debug, Default)]
pub struct IndexData {
    /// Build time, Unix seconds — the freshness baseline.
    pub mtime: u64,
    /// Git tree SHA, or empty when the tree is dirty / not a git repo.
    pub tree: String,
    /// Forward map, MUST be sorted by `anchor` so [`Reader::forward_lookup`] can
    /// binary-search it.
    pub forward: Vec<ForwardEntry>,
    /// Every in-scope path, MUST be sorted; backs the dangling/freshness checks.
    pub paths: Vec<String>,
}

/// Serialize an [`IndexData`] into the `refidx v1` byte layout.
pub fn serialize(data: &IndexData) -> Vec<u8> {
    let mut forward_body = String::new();
    for e in &data.forward {
        forward_body.push_str("fwd:");
        forward_body.push_str(&e.anchor);
        forward_body.push('\t');
        forward_body.push_str(&e.location);
        forward_body.push('\n');
    }
    let reverse_body = String::new(); // no reverse entries yet; section stays present but empty
    let mut paths_body = String::new();
    for p in &data.paths {
        paths_body.push_str(p);
        paths_body.push('\n');
    }

    let (l_fwd, l_rev, l_paths) = (forward_body.len(), reverse_body.len(), paths_body.len());

    // The section offsets are absolute from file start, so they depend on the
    // header's own length — which depends on the digit-count of those offsets.
    // Resolve the circularity with a tiny fixpoint (converges in 1–2 steps).
    let header = |h: usize| -> String {
        let fwd_off = h;
        let rev_off = h + l_fwd;
        let paths_off = h + l_fwd + l_rev;
        format!(
            "{MAGIC}\n\
             mtime:{}\n\
             tree:{}\n\
             section:forward:{fwd_off}:{l_fwd}\n\
             section:reverse:{rev_off}:{l_rev}\n\
             section:paths:{paths_off}:{l_paths}\n\
             \n",
            data.mtime, data.tree,
        )
    };
    let mut h = header(0).len();
    loop {
        let candidate = header(h);
        if candidate.len() == h {
            break;
        }
        h = candidate.len();
    }

    let mut out = header(h).into_bytes();
    out.extend_from_slice(forward_body.as_bytes());
    out.extend_from_slice(reverse_body.as_bytes());
    out.extend_from_slice(paths_body.as_bytes());
    out
}

/// A reader over an mmap'd (or otherwise borrowed) index image.
pub struct Reader<'a> {
    bytes: &'a [u8],
    /// section name → (offset, length).
    sections: HashMap<String, (usize, usize)>,
    /// Build mtime from the header (Unix seconds).
    pub mtime: u64,
    /// Git tree SHA from the header (may be empty).
    pub tree: String,
}

impl<'a> Reader<'a> {
    /// Parse the header of an index image. Refuses an unknown magic version
    /// rather than misparsing it.
    pub fn parse(bytes: &'a [u8]) -> Result<Reader<'a>, String> {
        let text =
            std::str::from_utf8(bytes).map_err(|_| "index is not valid UTF-8".to_string())?;
        let mut lines = text.split_inclusive('\n');

        let magic = next_line(&mut lines)?;
        if magic != MAGIC {
            return Err(format!(
                "unrecognized index format {magic:?} (expected {MAGIC:?})"
            ));
        }
        let mtime = next_line(&mut lines)?
            .strip_prefix("mtime:")
            .ok_or("missing mtime header")?
            .parse::<u64>()
            .map_err(|_| "malformed mtime header".to_string())?;
        let tree = next_line(&mut lines)?
            .strip_prefix("tree:")
            .ok_or("missing tree header")?
            .to_string();

        let mut sections = HashMap::new();
        loop {
            let line = next_line(&mut lines)?;
            if line.is_empty() {
                break; // blank line terminates the section table
            }
            let rest = line
                .strip_prefix("section:")
                .ok_or_else(|| format!("malformed section line: {line:?}"))?;
            let mut parts = rest.rsplitn(3, ':');
            let len: usize = parts
                .next()
                .and_then(|s| s.parse().ok())
                .ok_or("malformed section length")?;
            let off: usize = parts
                .next()
                .and_then(|s| s.parse().ok())
                .ok_or("malformed section offset")?;
            let name = parts.next().ok_or("malformed section name")?.to_string();
            sections.insert(name, (off, len));
        }

        Ok(Reader {
            bytes,
            sections,
            mtime,
            tree,
        })
    }

    fn section(&self, name: &str) -> &'a [u8] {
        match self.sections.get(name) {
            Some(&(off, len)) => &self.bytes[off..off + len],
            None => &[],
        }
    }

    /// Resolve an anchor through the forward map. Returns every matching
    /// location (zero = not found, one = unique, more = a collision).
    pub fn forward_lookup(&self, anchor: &str) -> Vec<String> {
        let slice = self.section("forward");
        let lines: Vec<&[u8]> = split_records(slice);
        let target = anchor.as_bytes();
        // Binary search: `forward` records are sorted by the key after `fwd:`,
        // so the predicate "key < target" is monotonic. This builds an explicit
        // line index for clarity; a future version can bisect the raw bytes in
        // place without materializing it.
        let start = lines.partition_point(|l| fwd_key(l) < target);
        let mut out = Vec::new();
        for line in &lines[start..] {
            if fwd_key(line) != target {
                break;
            }
            if let Some(loc) = fwd_location(line) {
                out.push(loc.to_string());
            }
        }
        out
    }

    /// Every in-scope path recorded in the index (the freshness/dangling set).
    pub fn paths(&self) -> Vec<&'a str> {
        split_records(self.section("paths"))
            .into_iter()
            .filter_map(|l| std::str::from_utf8(l).ok())
            .collect()
    }
}

fn next_line<'a, I: Iterator<Item = &'a str>>(lines: &mut I) -> Result<&'a str, String> {
    lines
        .next()
        .map(|l| l.strip_suffix('\n').unwrap_or(l))
        .ok_or_else(|| "index truncated in header".to_string())
}

/// Split a section body into its newline-terminated records (dropping the
/// trailing empty element after the final `\n`).
fn split_records(slice: &[u8]) -> Vec<&[u8]> {
    slice
        .split(|&b| b == b'\n')
        .filter(|l| !l.is_empty())
        .collect()
}

/// The binary-search key of a `forward` record: the anchor between `fwd:` and
/// the first tab.
fn fwd_key(line: &[u8]) -> &[u8] {
    let body = line.strip_prefix(b"fwd:").unwrap_or(line);
    match body.iter().position(|&b| b == b'\t') {
        Some(t) => &body[..t],
        None => body,
    }
}

/// The location body of a `forward` record: everything after the tab.
fn fwd_location(line: &[u8]) -> Option<&str> {
    let tab = line.iter().position(|&b| b == b'\t')?;
    std::str::from_utf8(&line[tab + 1..]).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> IndexData {
        IndexData {
            mtime: 1_718_660_000,
            tree: "abc123".to_string(),
            forward: vec![
                ForwardEntry {
                    anchor: "a/one.rs".into(),
                    location: "a/one.rs:1-10".into(),
                },
                ForwardEntry {
                    anchor: "b/two.rs".into(),
                    location: "b/two.rs:1-3".into(),
                },
            ],
            paths: vec!["a/one.rs".into(), "b/two.rs".into()],
        }
    }

    #[test]
    fn roundtrip_header_and_lookups() {
        let bytes = serialize(&sample());
        assert!(bytes.starts_with(b"refidx v1\n"));
        let r = Reader::parse(&bytes).unwrap();
        assert_eq!(r.mtime, 1_718_660_000);
        assert_eq!(r.tree, "abc123");
        assert_eq!(
            r.forward_lookup("a/one.rs"),
            vec!["a/one.rs:1-10".to_string()]
        );
        assert_eq!(
            r.forward_lookup("b/two.rs"),
            vec!["b/two.rs:1-3".to_string()]
        );
        assert!(r.forward_lookup("missing.rs").is_empty());
        assert_eq!(r.paths(), vec!["a/one.rs", "b/two.rs"]);
    }

    #[test]
    fn section_offsets_point_at_real_data() {
        // Proves the fixpoint produced consistent offsets: the parsed sections
        // line up with the bytes the writer appended.
        let bytes = serialize(&sample());
        let r = Reader::parse(&bytes).unwrap();
        assert!(r.section("reverse").is_empty());
        let fwd = std::str::from_utf8(r.section("forward")).unwrap();
        assert_eq!(
            fwd,
            "fwd:a/one.rs\ta/one.rs:1-10\nfwd:b/two.rs\tb/two.rs:1-3\n"
        );
    }

    #[test]
    fn collision_returns_all_definitions() {
        let mut data = sample();
        data.forward = vec![
            ForwardEntry {
                anchor: "dup".into(),
                location: "x.rs:1-1".into(),
            },
            ForwardEntry {
                anchor: "dup".into(),
                location: "y.rs:2-2".into(),
            },
        ];
        let bytes = serialize(&data);
        let r = Reader::parse(&bytes).unwrap();
        assert_eq!(r.forward_lookup("dup").len(), 2);
    }

    #[test]
    fn rejects_unknown_magic() {
        let bad = b"refidx v999\nmtime:0\ntree:\n\n";
        assert!(Reader::parse(bad).is_err());
    }
}
