/*!
The on-disk index format, `refidx v2`.

Flat, sorted, newline-terminated UTF-8 records, with a self-describing section
table. The writer ([`serialize`]) produces the bytes; the reader ([`Reader`])
borrows an mmap'd `&[u8]` and binary-searches a section in place. Three
sections: `forward` (each anchor's definition locations), `mentions` (where
prose writes paths, `[[rr:AD-5]]`), and `paths` (every in-scope file, the
freshness set). The section table lists every core section even when it is
zero-length, so the reader can locate each by name.

A format change must bump `MAGIC`, so an old reader rejects a new index rather
than misparsing it.
*/

use std::collections::HashMap;

/// Magic line pinning the format version; [`Reader::parse`] rejects any other
/// value.
pub const MAGIC: &str = "refidx v2";

/// One forward-map entry: an anchor and a location where it is defined.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForwardEntry {
    pub anchor: String,
    /// The location body, exactly as printed: `file:start-end`.
    pub location: String,
}

/// One mention-table entry: a path token and the location prose writes it at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MentionEntry {
    pub token: String,
    /// Where the mention sits: `file:line-line`.
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
    /// Forward map. [`serialize`] sorts it (total order: anchor, then
    /// location) before writing, so [`Reader::forward_lookup`] can
    /// binary-search the on-disk image; callers need not pre-sort.
    pub forward: Vec<ForwardEntry>,
    /// The mention table (`[[rr:AD-5]]`). Sorted like `forward`.
    pub mentions: Vec<MentionEntry>,
    /// Every in-scope path; backs the freshness check. [`serialize`] sorts it
    /// before writing.
    pub paths: Vec<String>,
}

/// Serialize an [`IndexData`] into the `refidx v2` byte layout.
///
/// The order of records on disk is canonicalized here, not trusted from the
/// caller: `forward` and `mentions` sort by a **total** order (key, then the
/// full location) and `paths` lexicographically. This makes the image a pure
/// function of the logical contents, so two builds of the same tree are
/// byte-identical even though the parallel walk hands records back in a
/// scheduling-dependent order, and colliding keys serialize identically
/// across input permutations. The total order is a refinement of the key
/// order [`Reader::forward_lookup`] binary-searches, so the search invariant
/// is preserved.
pub fn serialize(data: &IndexData) -> Vec<u8> {
    let mut forward: Vec<&ForwardEntry> = data.forward.iter().collect();
    forward.sort_by(|a, b| {
        a.anchor
            .cmp(&b.anchor)
            .then_with(|| a.location.cmp(&b.location))
    });
    let mut mentions: Vec<&MentionEntry> = data.mentions.iter().collect();
    mentions.sort_by(|a, b| {
        a.token
            .cmp(&b.token)
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
    let mut mentions_body = String::new();
    for m in mentions {
        mentions_body.push_str("men:");
        mentions_body.push_str(&m.token);
        mentions_body.push('\t');
        mentions_body.push_str(&m.location);
        mentions_body.push('\n');
    }
    let mut paths_body = String::new();
    for p in paths {
        paths_body.push_str(p);
        paths_body.push('\n');
    }

    let (l_fwd, l_men, l_paths) = (forward_body.len(), mentions_body.len(), paths_body.len());

    // The section offsets are absolute from file start, so they depend on the
    // header's own length — which depends on the digit-count of those
    // offsets. Resolve the circularity with a tiny fixpoint (converges in 1-2
    // steps).
    let header = |h: usize| -> String {
        let fwd_off = h;
        let men_off = h + l_fwd;
        let paths_off = h + l_fwd + l_men;
        format!(
            "{MAGIC}\n\
             mtime:{}\n\
             tree:{}\n\
             section:forward:{fwd_off}:{l_fwd}\n\
             section:mentions:{men_off}:{l_men}\n\
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
    out.extend_from_slice(mentions_body.as_bytes());
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

        // Bounds-check every section against the actual image length, so a
        // torn or truncated index is rejected here as a corrupt index rather
        // than panicking later when `section` slices past the end (the
        // offsets are taken from the header, but the bytes behind them may
        // not exist).
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
            // `parse` already bounds-checked every section, so the slice is
            // in range; `get(..).unwrap_or(&[])` keeps this total even if a
            // future caller builds a `Reader` by hand without that check.
            Some(&(off, len)) => self.bytes.get(off..off.saturating_add(len)).unwrap_or(&[]),
            None => &[],
        }
    }

    /// Resolve an anchor through the forward map. Returns every matching
    /// location (zero = not found, one = unique, more = ambiguous).
    pub fn forward_lookup(&self, anchor: &str) -> Vec<String> {
        let slice = self.section("forward");
        let lines: Vec<&[u8]> = split_records(slice);
        let target = anchor.as_bytes();
        // Binary search: `forward` records are sorted by the key after
        // `fwd:`, so the predicate "key < target" is monotonic.
        let start = lines.partition_point(|l| record_key(l, b"fwd:") < target);
        let mut out = Vec::new();
        for line in &lines[start..] {
            if record_key(line, b"fwd:") != target {
                break;
            }
            if let Some(loc) = record_value(line) {
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
    /// inside the location text there, and that section is sorted by anchor,
    /// not by file, so there is no binary-search shortcut for the inverse
    /// query; a position-keyed section could replace this with a bisect.
    ///
    /// # Examples
    ///
    /// ```
    /// use ripref::refidx::{serialize, ForwardEntry, IndexData, Reader};
    ///
    /// // One Markdown file with two nested section anchors: the title's
    /// // record spans the document, a heading spans its own section.
    /// let data = IndexData {
    ///     forward: vec![
    ///         ForwardEntry { anchor: "AD-9".into(), location: "doc/x.md:1-20".into() },
    ///         ForwardEntry { anchor: "Consequences".into(), location: "doc/x.md:12-20".into() },
    ///     ],
    ///     paths: vec!["doc/x.md".into()],
    ///     ..Default::default()
    /// };
    ///
    /// // Round-trip through the on-disk format, exactly as a reader does.
    /// let bytes = serialize(&data);
    /// let reader = Reader::parse(&bytes).unwrap();
    ///
    /// // Line 15 falls inside both spans; `covering` returns outermost first.
    /// let hits = reader.covering("doc/x.md", 15);
    /// let names: Vec<&str> = hits.iter().map(|h| h.anchor.as_str()).collect();
    /// assert_eq!(names, ["AD-9", "Consequences"]);
    /// ```
    pub fn covering(&self, file: &str, line: u64) -> Vec<AnchorHit> {
        let mut hits = Vec::new();
        for record in split_records(self.section("forward")) {
            let Ok(anchor) = std::str::from_utf8(record_key(record, b"fwd:")) else {
                continue;
            };
            let Some((loc_file, start, end)) = record_value(record).and_then(parse_location) else {
                continue;
            };
            // Exact file match (not a prefix): `a/b.rs` must not answer for
            // `b.rs`.
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

    /// Every mention-table entry, as `(token, location)` pairs in on-disk
    /// (token-sorted) order. The table serves completion and rename tooling
    /// (`[[rr:AD-5]]`); the scanners never read it.
    pub fn mentions(&self) -> Vec<(String, String)> {
        split_records(self.section("mentions"))
            .into_iter()
            .filter_map(|l| {
                let key = std::str::from_utf8(record_key(l, b"men:")).ok()?;
                let loc = record_value(l)?;
                Some((key.to_string(), loc.to_string()))
            })
            .collect()
    }

    /// Every in-scope path recorded in the index (the freshness set).
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

/// The binary-search key of a record: the text between its tag prefix and
/// the first tab.
fn record_key<'r>(line: &'r [u8], tag: &[u8]) -> &'r [u8] {
    let body = line.strip_prefix(tag).unwrap_or(line);
    match body.iter().position(|&b| b == b'\t') {
        Some(t) => &body[..t],
        None => body,
    }
}

/// The value body of a record: everything after the tab.
fn record_value(line: &[u8]) -> Option<&str> {
    let tab = line.iter().position(|&b| b == b'\t')?;
    std::str::from_utf8(&line[tab + 1..]).ok()
}

/// Split a `file:start-end` location into `(file, start, end)`. Parses from
/// the right so a colon in the path keeps its prefix (`[[rr:AD-1]]`); returns
/// `None` if the trailing span is not `<u64>-<u64>`.
pub fn parse_location(loc: &str) -> Option<(&str, u64, u64)> {
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
                    anchor: "Alpha".into(),
                    location: "a/one.md:1-10".into(),
                },
                ForwardEntry {
                    anchor: "two".into(),
                    location: "b/two.rs:1-3".into(),
                },
            ],
            mentions: vec![MentionEntry {
                token: "src/cli.rs".into(),
                location: "a/one.md:4-4".into(),
            }],
            paths: vec!["a/one.md".into(), "b/two.rs".into()],
        }
    }

    #[test]
    fn roundtrip_header_and_lookups() {
        let bytes = serialize(&sample());
        assert!(bytes.starts_with(b"refidx v2\n"));
        let r = Reader::parse(&bytes).unwrap();
        assert_eq!(r.mtime, 1_718_660_000);
        assert_eq!(r.tree, "abc123");
        assert_eq!(r.forward_lookup("Alpha"), vec!["a/one.md:1-10".to_string()]);
        assert_eq!(r.forward_lookup("two"), vec!["b/two.rs:1-3".to_string()]);
        assert!(r.forward_lookup("missing").is_empty());
        assert_eq!(
            r.mentions(),
            vec![("src/cli.rs".to_string(), "a/one.md:4-4".to_string())]
        );
        assert_eq!(r.paths(), vec!["a/one.md", "b/two.rs"]);
    }

    #[test]
    fn section_offsets_point_at_real_data() {
        // Proves the fixpoint produced consistent offsets: the parsed
        // sections line up with the bytes the writer appended.
        let bytes = serialize(&sample());
        let r = Reader::parse(&bytes).unwrap();
        let fwd = std::str::from_utf8(r.section("forward")).unwrap();
        assert_eq!(fwd, "fwd:Alpha\ta/one.md:1-10\nfwd:two\tb/two.rs:1-3\n");
        let men = std::str::from_utf8(r.section("mentions")).unwrap();
        assert_eq!(men, "men:src/cli.rs\ta/one.md:4-4\n");
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
        let bad = b"refidx v1\nmtime:0\ntree:\n\n";
        assert!(Reader::parse(bad).is_err());
    }

    /// `serialize` must be a pure function of the logical contents: feeding
    /// the same records in two different input orders (including colliding
    /// anchors, where an anchor-only sort would leave their order
    /// input-defined) must produce byte-identical images.
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
            mentions: vec![
                MentionEntry {
                    token: "b/n.md".into(),
                    location: "a.md:2-2".into(),
                },
                MentionEntry {
                    token: "a/m.md".into(),
                    location: "a.md:1-1".into(),
                },
            ],
            paths: vec!["z.rs".into(), "a.rs".into()],
        };
        // The exact same records, permuted, and `paths` reversed.
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
            mentions: vec![
                MentionEntry {
                    token: "a/m.md".into(),
                    location: "a.md:1-1".into(),
                },
                MentionEntry {
                    token: "b/n.md".into(),
                    location: "a.md:2-2".into(),
                },
            ],
            paths: vec!["a.rs".into(), "z.rs".into()],
        };
        assert_eq!(serialize(&one), serialize(&two));

        // And the canonical order is by key, then location, so colliding
        // records come out in location order regardless of input.
        let bytes = serialize(&one);
        let r = Reader::parse(&bytes).unwrap();
        assert_eq!(r.forward_lookup("dup"), vec!["a.rs:1-1", "z.rs:9-9"]);
    }

    /// A truncated image whose header still parses but whose section bytes
    /// are gone must be rejected as corrupt, never sliced out of bounds.
    #[test]
    fn parse_rejects_truncated_section_bytes() {
        let full = serialize(&sample());
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
            forward: vec![
                ForwardEntry {
                    anchor: "doc".into(),
                    location: "f.md:1-40".into(),
                },
                ForwardEntry {
                    anchor: "inner".into(),
                    location: "f.md:12-18".into(),
                },
                ForwardEntry {
                    anchor: "other".into(),
                    location: "g.md:1-5".into(),
                },
                ForwardEntry {
                    anchor: "outer".into(),
                    location: "f.md:8-30".into(),
                },
            ],
            paths: vec!["f.md".into(), "g.md".into()],
            ..Default::default()
        };
        let bytes = serialize(&data);
        let r = Reader::parse(&bytes).unwrap();

        // Outermost-first: doc (1-40) contains outer (8-30) contains inner
        // (12-18).
        assert_eq!(
            covering_names(&r, "f.md", 15),
            vec!["doc", "outer", "inner"]
        );
        assert_eq!(
            r.covering("f.md", 15)[0],
            AnchorHit {
                anchor: "doc".into(),
                file: "f.md".into(),
                start_line: 1,
                end_line: 40,
            }
        );
        // Line 9 sits in doc + outer but above inner's 12-18.
        assert_eq!(covering_names(&r, "f.md", 9), vec!["doc", "outer"]);
        // Past every span on f.md → nothing (drives the adverse exit).
        assert!(r.covering("f.md", 50).is_empty());
        // Exact-file match: f.md records must not leak into a g.md query.
        assert_eq!(covering_names(&r, "g.md", 3), vec!["other"]);
    }

    #[test]
    fn covering_breaks_equal_spans_lexicographically() {
        let data = IndexData {
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
            ..Default::default()
        };
        let bytes = serialize(&data);
        let r = Reader::parse(&bytes).unwrap();
        // Same width, same start → anchor name breaks the tie.
        assert_eq!(covering_names(&r, "f.rs", 20), vec!["alpha", "beta"]);
    }
}
