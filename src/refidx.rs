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

/// One anchor whose definition span covers a queried position, returned by
/// [`Reader::covering`]. Unlike [`ForwardEntry`] — the on-disk record whose
/// `location` is unparsed text — the span is split into line numbers so the
/// caller can sort by it and emit structured (JSON) output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnchorHit {
    pub anchor: String,
    pub file: String,
    pub start_line: u64,
    pub end_line: u64,
}

/// The logical contents of an index, before/after (de)serialization.
#[derive(Clone, Debug, Default)]
pub struct IndexData {
    /// Build time, Unix seconds — the freshness baseline.
    pub mtime: u64,
    /// Git tree SHA, or empty when the tree is dirty / not a git repo.
    pub tree: String,
    /// Forward map. [`serialize`] sorts it (total order: anchor, then location)
    /// before writing, so [`Reader::forward_lookup`] can binary-search the
    /// on-disk image; callers need not pre-sort.
    pub forward: Vec<ForwardEntry>,
    /// Every in-scope path; backs the dangling/freshness checks. [`serialize`]
    /// sorts it before writing, like `forward`.
    pub paths: Vec<String>,
}

/// Serialize an [`IndexData`] into the `refidx v1` byte layout.
///
/// The order of records on disk is canonicalized here, not trusted from the
/// caller: `forward` is sorted by a **total** order (anchor, then the full
/// `location`) and `paths` lexicographically. This makes the image a pure
/// function of the logical contents, so two builds of the same tree are
/// byte-identical even though the parallel walk hands records back in a
/// scheduling-dependent order, and colliding anchors (same anchor, different
/// location) serialize identically across input permutations — an anchor-only
/// sort would leave their relative order input-defined. The total order is a
/// refinement of the anchor order [`Reader::forward_lookup`] binary-searches, so
/// the search invariant is preserved.
pub fn serialize(data: &IndexData) -> Vec<u8> {
    let mut forward: Vec<&ForwardEntry> = data.forward.iter().collect();
    forward.sort_by(|a, b| {
        a.anchor
            .cmp(&b.anchor)
            .then_with(|| a.location.cmp(&b.location))
    });
    let mut paths: Vec<&String> = data.paths.iter().collect();
    paths.sort();

    let mut forward_body = String::new();
    for e in forward {
        forward_body.push_str("fwd:");
        forward_body.push_str(&e.anchor);
        forward_body.push('\t');
        forward_body.push_str(&e.location);
        forward_body.push('\n');
    }
    let reverse_body = String::new(); // no reverse entries yet; section stays present but empty
    let mut paths_body = String::new();
    for p in paths {
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

        // Bounds-check every section against the actual image length, so a torn
        // or truncated index is rejected here as a corrupt index rather than
        // panicking later when `section` slices past the end (the offsets are
        // taken from the header, but the bytes behind them may not exist).
        for (name, &(off, len)) in &sections {
            let end = off
                .checked_add(len)
                .ok_or_else(|| format!("section {name:?} offset+length overflows"))?;
            if end > bytes.len() {
                return Err(format!(
                    "section {name:?} extends past end of index ({end} > {})",
                    bytes.len()
                ));
            }
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
            // `parse` already bounds-checked every section, so the slice is in
            // range; `get(..).unwrap_or(&[])` keeps this total even if a future
            // caller builds a `Reader` by hand without that check.
            Some(&(off, len)) => self.bytes.get(off..off.saturating_add(len)).unwrap_or(&[]),
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

    /// Every anchor whose definition span covers `file:line`, ordered
    /// outermost-first: widest span first, ties broken by start line, then by
    /// anchor name (a total order, so the output is deterministic).
    ///
    /// This is a linear scan of the `forward` section. Positions live only
    /// inside the location text there, and that section is sorted by anchor —
    /// not by file — so there is no binary-search shortcut for the inverse
    /// query. A position-keyed section could replace this with a bisect later
    /// (see this module's format docs); the scan keeps the walking skeleton
    /// format-compatible.
    ///
    /// # Examples
    ///
    /// ```
    /// use ripref::refidx::{serialize, ForwardEntry, IndexData, Reader};
    ///
    /// // One file with two overlapping anchors: the whole-file path anchor
    /// // (lines 1-20) and a function defined inside it (lines 5-12). `forward`
    /// // is kept sorted by anchor — the writer's invariant.
    /// let data = IndexData {
    ///     mtime: 0,
    ///     tree: String::new(),
    ///     forward: vec![
    ///         ForwardEntry { anchor: "handle".into(), location: "src/api.rs:5-12".into() },
    ///         ForwardEntry { anchor: "src/api.rs".into(), location: "src/api.rs:1-20".into() },
    ///     ],
    ///     paths: vec!["src/api.rs".into()],
    /// };
    ///
    /// // Round-trip through the on-disk format, exactly as a reader does:
    /// // serialize to bytes, then parse them back (here from a slice, in the
    /// // binary from an mmap).
    /// let bytes = serialize(&data);
    /// let reader = Reader::parse(&bytes).unwrap();
    ///
    /// // Line 8 falls inside both spans; `covering` returns them outermost-first.
    /// let hits = reader.covering("src/api.rs", 8);
    /// let names: Vec<&str> = hits.iter().map(|h| h.anchor.as_str()).collect();
    /// assert_eq!(names, ["src/api.rs", "handle"]);
    /// ```
    pub fn covering(&self, file: &str, line: u64) -> Vec<AnchorHit> {
        let mut hits = Vec::new();
        for record in split_records(self.section("forward")) {
            let Ok(anchor) = std::str::from_utf8(fwd_key(record)) else {
                continue;
            };
            let Some((loc_file, start, end)) = fwd_location(record).and_then(parse_location) else {
                continue;
            };
            // Exact file match (not a prefix): `a/b.rs` must not answer for `b.rs`.
            if loc_file == file && start <= line && line <= end {
                hits.push(AnchorHit {
                    anchor: anchor.to_string(),
                    file: loc_file.to_string(),
                    start_line: start,
                    end_line: end,
                });
            }
        }
        hits.sort_by(|a, b| {
            (b.end_line - b.start_line)
                .cmp(&(a.end_line - a.start_line))
                .then(a.start_line.cmp(&b.start_line))
                .then_with(|| a.anchor.cmp(&b.anchor))
        });
        hits
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

/// Split a `file:start-end` location into `(file, start, end)`. Parses from the
/// right so a colon in the path keeps its prefix; returns `None` if the trailing
/// span is not `<u64>-<u64>`.
fn parse_location(loc: &str) -> Option<(&str, u64, u64)> {
    let (file, span) = loc.rsplit_once(':')?;
    let (start, end) = span.split_once('-')?;
    Some((file, start.parse().ok()?, end.parse().ok()?))
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

    /// `serialize` must be a pure function of the logical contents: feeding the
    /// same records in two different input orders (including colliding anchors,
    /// where an anchor-only sort would leave their order input-defined) must
    /// produce byte-identical images. This is what lets the parallel walk hand
    /// records back in any order and still yield a deterministic index, and it
    /// catches the false-confidence in a build-twice test that would also pass
    /// on a non-total sort.
    #[test]
    fn serialize_is_order_independent_for_collisions() {
        let mtime = 7;
        let one = IndexData {
            mtime,
            tree: "t".into(),
            forward: vec![
                ForwardEntry {
                    anchor: "dup".into(),
                    location: "z.rs:9-9".into(),
                },
                ForwardEntry {
                    anchor: "alpha".into(),
                    location: "a.rs:1-2".into(),
                },
                ForwardEntry {
                    anchor: "dup".into(),
                    location: "a.rs:1-1".into(),
                },
            ],
            paths: vec!["z.rs".into(), "a.rs".into()],
        };
        // The exact same records, permuted (the two `dup` collisions swapped,
        // `alpha` moved) and `paths` reversed.
        let two = IndexData {
            mtime,
            tree: "t".into(),
            forward: vec![
                ForwardEntry {
                    anchor: "dup".into(),
                    location: "a.rs:1-1".into(),
                },
                ForwardEntry {
                    anchor: "dup".into(),
                    location: "z.rs:9-9".into(),
                },
                ForwardEntry {
                    anchor: "alpha".into(),
                    location: "a.rs:1-2".into(),
                },
            ],
            paths: vec!["a.rs".into(), "z.rs".into()],
        };
        assert_eq!(serialize(&one), serialize(&two));

        // And the canonical order is by anchor, then location, so the colliding
        // `dup` records come out in location order regardless of input.
        let bytes = serialize(&one);
        let r = Reader::parse(&bytes).unwrap();
        assert_eq!(r.forward_lookup("dup"), vec!["a.rs:1-1", "z.rs:9-9"]);
    }

    /// A truncated image whose header still parses but whose section bytes are
    /// gone must be rejected as corrupt, never sliced out of bounds (the
    /// historical `refidx.rs` panic). `parse` bounds-checks, so the error
    /// surfaces here instead of as an OOB panic in `section`.
    #[test]
    fn parse_rejects_truncated_section_bytes() {
        let full = serialize(&sample());
        // Cut the body off but keep the whole header (through the blank line).
        let header_end = full
            .windows(2)
            .position(|w| w == b"\n\n")
            .map(|p| p + 2)
            .expect("header has a terminating blank line");
        assert!(header_end < full.len(), "sample has a non-empty body");
        let truncated = &full[..header_end];
        let err = Reader::parse(truncated)
            .err()
            .expect("truncated index must be rejected");
        assert!(
            err.contains("past end") || err.contains("overflow"),
            "expected a bounds error, got {err:?}"
        );
    }

    /// The names of the covering anchors, in returned order.
    fn covering_names(r: &Reader, file: &str, line: u64) -> Vec<String> {
        r.covering(file, line)
            .into_iter()
            .map(|h| h.anchor)
            .collect()
    }

    #[test]
    fn covering_returns_nested_anchors_outermost_first() {
        // `forward` MUST be sorted by anchor (the writer's invariant), so the
        // fixture is in anchor order, not span order.
        let data = IndexData {
            mtime: 0,
            tree: String::new(),
            forward: vec![
                ForwardEntry {
                    anchor: "file".into(),
                    location: "f.rs:1-40".into(),
                },
                ForwardEntry {
                    anchor: "inner".into(),
                    location: "f.rs:12-18".into(),
                },
                ForwardEntry {
                    anchor: "other".into(),
                    location: "g.rs:1-5".into(),
                },
                ForwardEntry {
                    anchor: "outer".into(),
                    location: "f.rs:8-30".into(),
                },
            ],
            paths: vec!["f.rs".into(), "g.rs".into()],
        };
        let bytes = serialize(&data);
        let r = Reader::parse(&bytes).unwrap();

        // Outermost-first: file (1-40) ⊃ outer (8-30) ⊃ inner (12-18).
        assert_eq!(
            covering_names(&r, "f.rs", 15),
            vec!["file", "outer", "inner"]
        );
        assert_eq!(
            r.covering("f.rs", 15)[0],
            AnchorHit {
                anchor: "file".into(),
                file: "f.rs".into(),
                start_line: 1,
                end_line: 40,
            }
        );
        // Line 9 sits in file + outer but above inner's 12-18.
        assert_eq!(covering_names(&r, "f.rs", 9), vec!["file", "outer"]);
        // Past every span on f.rs → nothing (drives the exit-1 path).
        assert!(r.covering("f.rs", 50).is_empty());
        // Exact-file match: f.rs records must not leak into a g.rs query.
        assert_eq!(covering_names(&r, "g.rs", 3), vec!["other"]);
    }

    #[test]
    fn covering_breaks_equal_spans_lexicographically() {
        let data = IndexData {
            mtime: 0,
            tree: String::new(),
            forward: vec![
                ForwardEntry {
                    anchor: "alpha".into(),
                    location: "f.rs:20-20".into(),
                },
                ForwardEntry {
                    anchor: "beta".into(),
                    location: "f.rs:20-20".into(),
                },
            ],
            paths: vec!["f.rs".into()],
        };
        let bytes = serialize(&data);
        let r = Reader::parse(&bytes).unwrap();
        // Same width, same start → anchor name breaks the tie.
        assert_eq!(covering_names(&r, "f.rs", 20), vec!["alpha", "beta"]);
    }
}
