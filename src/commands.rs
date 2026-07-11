/*!
Command implementations for the five verbs of `[[rr:AD-3]]`: the single
writer (`index`), the index readers (`read`, `at`), the lexical lister
(`search`), and the gate (`verify`).

Each returns `Ok(exit_code)` for a normal outcome (including adverse and
stale, which are non-zero but not errors) or `Err(message)` for a
usage-level failure the caller reports as exit `2`. Output shapes and exit
codes follow `[[rr:AD-4]]`.
*/

use std::path::{Path, PathBuf};

use memmap2::Mmap;

use crate::atomic;
use crate::cli::{self, LowArgs, OutputFormat};
use crate::config;
use crate::exit;
use crate::indexer;
use crate::marker;
use crate::refidx::{self, AnchorHit, Reader};
use crate::scan::{self, What};

/// `rr index` — build/refresh the index from the working tree: anchors and
/// path mentions.
pub fn run_index(args: &LowArgs) -> Result<u8, String> {
    let root = Path::new(".");
    let index_path = PathBuf::from(cli::index_path(args));
    let cfg = config::load(root);
    let scope = config::scope_matcher(root, &cfg)?;

    let data = indexer::build(root, &index_path, &scope)
        .map_err(|e| format!("failed to walk the working tree: {e}"))?;
    let bytes = refidx::serialize(&data);

    if let Some(parent) = index_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create {}: {e}", parent.display()))?;
        }
    }
    // Atomic replace, not a truncate-in-place: a reader (which mmaps this
    // file) must see either the whole old index or the whole new one, never
    // a torn half-write. See `crate::atomic`.
    atomic::atomic_write(&index_path, &bytes)
        .map_err(|e| format!("failed to write {}: {e}", index_path.display()))?;

    match args.format {
        OutputFormat::Json => println!(
            "{}",
            envelope(
                "index",
                &format!(
                    r#"{{"anchors":{},"mentions":{},"files":{}}}"#,
                    data.forward.len(),
                    data.mentions.len(),
                    data.paths.len()
                ),
            )
        ),
        OutputFormat::Text if args.quiet => {}
        OutputFormat::Text => println!(
            "indexed {} anchors and {} path mentions across {} files",
            data.forward.len(),
            data.mentions.len(),
            data.paths.len()
        ),
    }
    Ok(exit::OK)
}

/// Open the index, mmap it, parse the header, and gate on freshness before
/// handing a [`Reader`] to `f`. This is the shared preamble of every reading
/// verb: a missing or stale index short-circuits to `exit::STALE` without
/// ever calling `f`. The `Reader` borrows the mmap, so it cannot be returned
/// past its backing buffer — a closure keeps both alive for the call.
fn with_fresh_reader<F>(
    index_path: &Path,
    root: &Path,
    skip_freshness: bool,
    f: F,
) -> Result<u8, String>
where
    F: FnOnce(&Reader) -> Result<u8, String>,
{
    match load_index(index_path, root, skip_freshness)? {
        IndexState::Missing => {
            eprintln!("no index at {} — run `rr index`", index_path.display());
            Ok(exit::STALE)
        }
        IndexState::Stale => {
            eprintln!("index is stale — rebuild with `rr index`");
            Ok(exit::STALE)
        }
        IndexState::Fresh(bytes) => {
            let reader = Reader::parse(&bytes).map_err(|e| format!("corrupt index: {e}"))?;
            f(&reader)
        }
    }
}

/// The outcome of trying to load a usable index.
enum IndexState {
    /// Present, parseable, and fresh: the owned image bytes.
    Fresh(Vec<u8>),
    /// No index file exists yet.
    Missing,
    /// Present but stale (the working tree moved on since the build).
    Stale,
}

/// Open the index, copy it into owned bytes, and report whether it is fresh.
/// A corrupt index is an error (mapped to `corrupt index` by the caller); a
/// missing or stale one is a non-error state the caller maps to its own
/// exit.
///
/// The mapping is released (copied into a `Vec`) before `fresh` runs,
/// because `fresh` may spawn `git status` and, on Windows, a concurrent
/// `rr index` replaces this file; holding a mapping across that is the
/// fragile case. The atomic write in `rr index` is what actually prevents a
/// torn read; copying out is defense in depth, plus an `fs::read` fallback
/// for the rare platform where mmap of a valid file fails to open.
fn load_index(index_path: &Path, root: &Path, skip_freshness: bool) -> Result<IndexState, String> {
    let file = match std::fs::File::open(index_path) {
        Ok(f) => f,
        Err(_) => return Ok(IndexState::Missing),
    };
    let bytes: Vec<u8> = {
        // SAFETY: the index is a regular file we just opened; `rr index`
        // publishes new contents with an atomic rename, so the mapped inode
        // is always a complete image and is never mutated under us.
        #[allow(unsafe_code)] // the one justified unsafe in the crate (posture: src/lib.rs)
        match unsafe { Mmap::map(&file) } {
            Ok(mmap) => mmap.to_vec(),
            Err(_) => std::fs::read(index_path)
                .map_err(|e| format!("failed to read {}: {e}", index_path.display()))?,
        }
    };
    {
        let reader = Reader::parse(&bytes).map_err(|e| format!("corrupt index: {e}"))?;
        if !fresh(&reader, root, skip_freshness) {
            return Ok(IndexState::Stale);
        }
    }
    Ok(IndexState::Fresh(bytes))
}

/// Whether the index may answer this query. Cheap-first: (1)
/// `--no-freshness` trusts unconditionally; (2) a clean tree whose HEAD SHA
/// matches the stamp is provably what we indexed (no stat-walk needed); (3)
/// else fall back to the parallel mtime walk. The git probe spawns one
/// `git status`, so it runs only after the free checks and only when a
/// clean-tree stamp exists to match.
fn fresh(reader: &Reader, root: &Path, skip_freshness: bool) -> bool {
    if skip_freshness {
        return true;
    }
    if !reader.tree.is_empty() && indexer::git_tree(root) == reader.tree {
        return true;
    }
    indexer::newest_mtime(&reader.paths(), root) <= reader.mtime
}

/// Resolve one anchor to its definition locations, structured. The whole
/// token is tried as an identity first, so an identity that itself contains
/// `#` resolves literally; only then does the path qualifier of
/// `[[rr:AD-1]]` split, and the identity's definitions filter to the
/// qualifying file.
fn resolve(reader: &Reader, anchor: &str) -> Vec<(String, u64, u64)> {
    let parse_all = |locs: Vec<String>| -> Vec<(String, u64, u64)> {
        locs.iter()
            .filter_map(|l| refidx::parse_location(l))
            .map(|(f, s, e)| (f.to_string(), s, e))
            .collect()
    };
    let direct = parse_all(reader.forward_lookup(anchor));
    if !direct.is_empty() {
        return direct;
    }
    if let Some((path, identity)) = cli::split_qualifier(anchor) {
        return parse_all(reader.forward_lookup(identity))
            .into_iter()
            .filter(|(f, _, _)| f == path)
            .collect();
    }
    Vec::new()
}

/// `rr read <ref>` — resolve a marker, or a bare anchor, to the anchor's
/// definition locations. The reader strips a pasted `[[rr:...]]` wrapper and
/// unescapes before resolving (`[[rr:AD-2]]`); a token that opens like a
/// marker but is not one is a usage error, never a silent reparse.
pub fn run_read(args: &LowArgs) -> Result<u8, String> {
    let root = Path::new(".");
    let index_path = PathBuf::from(cli::index_path(args));
    let token = args.positional[0].to_string_lossy().into_owned();
    let anchor = cli::parse_reference(&token)?;

    with_fresh_reader(&index_path, root, args.no_freshness, |reader| {
        let locations = resolve(reader, &anchor);
        if args.format == OutputFormat::Json {
            let mut data = String::from(r#"{"anchor":"#);
            push_json_str(&mut data, &anchor);
            data.push_str(",\"locations\":[");
            for (i, loc) in locations.iter().enumerate() {
                if i > 0 {
                    data.push(',');
                }
                push_location(&mut data, &loc.0, loc.1, loc.2);
            }
            data.push_str("]}");
            println!("{}", envelope("read", &data));
        } else {
            for (file, start, end) in &locations {
                println!("{file}:{start}-{end}");
            }
        }
        match locations.len() {
            0 => {
                eprintln!("no such anchor: {anchor}");
                Ok(exit::ADVERSE)
            }
            1 => Ok(exit::OK),
            n => {
                eprintln!(
                    "ambiguous anchor: {anchor} resolves to {n} definitions (add a path qualifier)"
                );
                Ok(exit::ADVERSE)
            }
        }
    })
}

/// `rr at <file>:<line>` — the marker for the innermost anchor whose
/// definition covers the line; the inverse of `read`. `--all` reports the
/// whole covering nest, outermost first.
pub fn run_at(args: &LowArgs) -> Result<u8, String> {
    let root = Path::new(".");
    let index_path = PathBuf::from(cli::index_path(args));
    // `validate` already accepted this; re-parsing here keeps the position
    // in one place rather than threading a parsed field through `LowArgs`.
    let (file, line) = cli::parse_position(&args.positional[0].to_string_lossy())?;

    with_fresh_reader(&index_path, root, args.no_freshness, |reader| {
        let hits = reader.covering(&file, line);
        // The innermost tie set: every hit sharing the tightest span.
        let emitted: Vec<&AnchorHit> = if args.all {
            hits.iter().collect()
        } else if let Some(last) = hits.last() {
            hits.iter()
                .filter(|h| h.start_line == last.start_line && h.end_line == last.end_line)
                .collect()
        } else {
            Vec::new()
        };
        // The minimal unambiguous form: unqualified while the identity
        // resolves uniquely, path-qualified when it does not.
        let forms: Vec<(String, &AnchorHit)> = emitted
            .iter()
            .map(|h| {
                let form = if reader.forward_lookup(&h.anchor).len() > 1 {
                    format!("{}#{}", h.file, h.anchor)
                } else {
                    h.anchor.clone()
                };
                (form, *h)
            })
            .collect();

        match args.format {
            OutputFormat::Json => println!("{}", envelope("at", &at_json(&forms))),
            OutputFormat::Text if forms.is_empty() => {
                eprintln!("no anchor covers {file}:{line}");
            }
            OutputFormat::Text => println!("{}", at_text(&forms)),
        }
        if forms.is_empty() {
            if args.format == OutputFormat::Json {
                eprintln!("no anchor covers {file}:{line}");
            }
            Ok(exit::ADVERSE)
        } else if !args.all && forms.len() > 1 {
            eprintln!(
                "ambiguous: {} anchors tie on the innermost span",
                forms.len()
            );
            Ok(exit::ADVERSE)
        } else {
            Ok(exit::OK)
        }
    })
}

/// Text rendering for `rr at`: one marker per line, the document form a
/// person pastes (`[[rr:AD-4]]`). Returned rather than printed so it is
/// unit-testable; `run_at` does the I/O.
fn at_text(forms: &[(String, &AnchorHit)]) -> String {
    forms
        .iter()
        .map(|(form, _)| marker::wrap(form))
        .collect::<Vec<_>>()
        .join("\n")
}

/// JSON `data` for `rr at`: `anchors`, always a list, each entry the bare
/// `anchor` (minimal unambiguous form), the composed `marker`, and the
/// definition's `location`. Returned (not printed) so the exact document can
/// be asserted in tests.
fn at_json(forms: &[(String, &AnchorHit)]) -> String {
    let mut out = String::from(r#"{"anchors":["#);
    for (i, (form, hit)) in forms.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str("{\"anchor\":");
        push_json_str(&mut out, form);
        out.push_str(",\"marker\":");
        push_json_str(&mut out, &marker::wrap(form));
        out.push_str(",\"location\":");
        push_location(&mut out, &hit.file, hit.start_line, hit.end_line);
        out.push('}');
    }
    out.push_str("]}");
    out
}

/// One scoped, readable text file: its repo-relative path and content.
struct ScopedFile {
    rel: String,
    content: String,
    host: scan::Host,
}

/// Walk the working tree and collect the files the profile's scope selects.
/// Shared by `search` and `verify`; `index` applies the same matcher inside
/// its own walk.
fn scoped_files(root: &Path, matcher: &ignore::overrides::Override) -> Vec<ScopedFile> {
    let mut out = Vec::new();
    let walker = ignore::WalkBuilder::new(root)
        .hidden(true)
        .git_ignore(true)
        .git_exclude(true)
        .parents(true)
        .build();
    for dent in walker.flatten() {
        if !dent.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let rel = dent
            .path()
            .strip_prefix(root)
            .unwrap_or(dent.path())
            .to_string_lossy()
            .replace('\\', "/");
        if !config::in_scope(matcher, &rel) {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(dent.path()) else {
            continue; // binary or unreadable: not scoped text
        };
        let ext = rel.rsplit('.').next();
        out.push(ScopedFile {
            host: scan::host_for(ext),
            rel,
            content,
        });
    }
    out.sort_by(|a, b| a.rel.cmp(&b.rel));
    out
}

/// `rr search [<anchor>]` — list the markers scoped text writes, each with
/// the location it sits at; under `--mentions`, the path mentions instead.
/// Purely lexical: no index is read, so it never returns stale.
pub fn run_search(args: &LowArgs) -> Result<u8, String> {
    let root = Path::new(".");
    let cfg = config::load(root);
    let matcher = config::scope_matcher(root, &cfg)?;
    let filter = args.positional.first().map(|a| {
        let token = a.to_string_lossy().into_owned();
        cli::parse_reference(&token)
    });
    let filter = match filter {
        Some(Ok(anchor)) => Some(anchor),
        Some(Err(why)) => return Err(why),
        None => None,
    };

    let mut lines = Vec::new();
    let mut json = String::from(r#"{"matches":["#);
    let mut count = 0usize;
    for file in scoped_files(root, &matcher) {
        for found in scan::scan(&file.content, file.host) {
            match (&found.what, args.mentions) {
                (What::Marker { raw, anchor }, false) => {
                    if let Some(want) = &filter {
                        if !filter_matches(want, anchor) {
                            continue;
                        }
                    }
                    lines.push(format!("{}:{}: {raw}", file.rel, found.line));
                    if count > 0 {
                        json.push(',');
                    }
                    json.push_str("{\"file\":");
                    push_json_str(&mut json, &file.rel);
                    json.push_str(&format!(",\"line\":{}", found.line));
                    json.push_str(",\"anchor\":");
                    push_json_str(&mut json, anchor);
                    json.push_str(",\"marker\":");
                    push_json_str(&mut json, raw);
                    json.push('}');
                    count += 1;
                }
                (What::Mention { token, .. }, true) => {
                    lines.push(format!("{}:{}: {token}", file.rel, found.line));
                    if count > 0 {
                        json.push(',');
                    }
                    json.push_str("{\"file\":");
                    push_json_str(&mut json, &file.rel);
                    json.push_str(&format!(",\"line\":{}", found.line));
                    json.push_str(",\"mention\":");
                    push_json_str(&mut json, token);
                    json.push('}');
                    count += 1;
                }
                _ => {}
            }
        }
    }
    json.push_str("]}");

    if args.format == OutputFormat::Json {
        println!("{}", envelope("search", &json));
    } else {
        for line in &lines {
            println!("{line}");
        }
        println!(
            "{count} {}",
            if args.mentions { "mentions" } else { "markers" }
        );
    }
    Ok(if count > 0 { exit::OK } else { exit::ADVERSE })
}

/// Whether a search filter matches a decoded marker anchor: an unqualified
/// argument matches every marker whose identity equals it, path-qualified or
/// not; a qualified argument matches exactly (`[[rr:AD-3]]`).
fn filter_matches(want: &str, anchor: &str) -> bool {
    if want == anchor {
        return true;
    }
    if !want.contains('#') {
        if let Some((_, identity)) = cli::split_qualifier(anchor) {
            return identity == want;
        }
    }
    false
}

/// One `verify` finding.
struct Finding {
    file: String,
    line: u64,
    rule: &'static str,
    detail: String,
}

/// `rr verify` — the gate: judge the references scoped text writes and
/// report findings of the six kinds of `[[rr:AD-3]]`. Resolution judgments
/// need the index, so a stale index exits 3 rather than judging from stale
/// data; mention judgments run against the live tree.
pub fn run_verify(args: &LowArgs) -> Result<u8, String> {
    let root = Path::new(".");
    let index_path = PathBuf::from(cli::index_path(args));
    let cfg = config::load(root);
    let matcher = config::scope_matcher(root, &cfg)?;

    with_fresh_reader(&index_path, root, args.no_freshness, |reader| {
        let mut findings: Vec<Finding> = Vec::new();
        for file in scoped_files(root, &matcher) {
            for found in scan::scan(&file.content, file.host) {
                let (rule, detail) = match &found.what {
                    What::Malformed { reason } => ("malformed marker", reason.clone()),
                    What::Marker { raw, anchor } => {
                        if !anchor.contains('#') && scan::is_path_shaped(anchor) {
                            ("path-only marker", raw.clone())
                        } else {
                            match resolve(reader, anchor).len() {
                                0 => ("dangling marker", raw.clone()),
                                1 => continue,
                                n => (
                                    "ambiguous marker",
                                    format!("{raw} resolves to {n} definitions"),
                                ),
                            }
                        }
                    }
                    What::Mention { token, line_ref } => {
                        // The judgment guard of AD-5: only a token whose
                        // first segment names a real directory (or scope
                        // root) is judged, so prose compounds never reach a
                        // finding.
                        let first = token.split('/').next().unwrap_or("");
                        if first.is_empty() || !root.join(first).is_dir() {
                            continue;
                        }
                        if *line_ref {
                            ("bare path:line reference", token.clone())
                        } else if !root.join(token).exists() {
                            ("stale path mention", token.clone())
                        } else {
                            continue;
                        }
                    }
                };
                findings.push(Finding {
                    file: file.rel.clone(),
                    line: found.line,
                    rule,
                    detail,
                });
            }
        }

        if args.format == OutputFormat::Json {
            let mut data = String::from(r#"{"findings":["#);
            for (i, f) in findings.iter().enumerate() {
                if i > 0 {
                    data.push(',');
                }
                data.push_str("{\"file\":");
                push_json_str(&mut data, &f.file);
                data.push_str(&format!(",\"line\":{}", f.line));
                data.push_str(",\"rule\":");
                push_json_str(&mut data, f.rule);
                data.push('}');
            }
            data.push_str("]}");
            println!("{}", envelope("verify", &data));
        } else {
            for f in &findings {
                println!("{}:{}: {}: {}", f.file, f.line, f.rule, f.detail);
            }
            println!("{} findings", findings.len());
        }
        Ok(if findings.is_empty() {
            exit::OK
        } else {
            exit::ADVERSE
        })
    })
}

/// The one `rr-json` envelope every verb prints under `--format json`
/// (`[[rr:AD-4]]`). Hand-rolled because the crate has no serde dependency
/// and the schema is a hand-written source of truth.
fn envelope(command: &str, data: &str) -> String {
    format!(r#"{{"format":"rr-json","version":1,"command":"{command}","data":{data}}}"#)
}

/// Append a structured location object.
fn push_location(out: &mut String, file: &str, start: u64, end: u64) {
    out.push_str("{\"file\":");
    push_json_str(out, file);
    out.push_str(&format!(",\"start_line\":{start},\"end_line\":{end}}}"));
}

/// Append `s` to `out` as a quoted, escaped JSON string. Escapes `"`, `\`,
/// and C0 control characters — anchors legitimately contain quotes and stray
/// control bytes must not break the document.
fn push_json_str(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(anchor: &str, file: &str, start_line: u64, end_line: u64) -> AnchorHit {
        AnchorHit {
            anchor: anchor.to_string(),
            file: file.to_string(),
            start_line,
            end_line,
        }
    }

    #[test]
    fn at_text_wraps_each_form() {
        let a = hit("Guide", "docs/guide.md", 1, 40);
        let b = hit("Configuration", "docs/guide.md", 12, 30);
        let forms = vec![
            ("Guide".to_string(), &a),
            ("docs/guide.md#Configuration".to_string(), &b),
        ];
        assert_eq!(
            at_text(&forms),
            "[[rr:Guide]]\n[[rr:docs/guide.md#Configuration]]"
        );
    }

    #[test]
    fn at_json_is_an_anchors_list() {
        let h = hit("handle_request", "src/handlers.py", 8, 30);
        let forms = vec![("handle_request".to_string(), &h)];
        assert_eq!(
            envelope("at", &at_json(&forms)),
            r#"{"format":"rr-json","version":1,"command":"at","data":{"anchors":[{"anchor":"handle_request","marker":"[[rr:handle_request]]","location":{"file":"src/handlers.py","start_line":8,"end_line":30}}]}}"#
        );
    }

    #[test]
    fn at_json_empty_list_still_shapes() {
        assert_eq!(at_json(&[]), r#"{"anchors":[]}"#);
    }

    #[test]
    fn filter_matches_identity_through_qualifier() {
        assert!(filter_matches("parse_reference", "parse_reference"));
        assert!(filter_matches(
            "parse_reference",
            "src/cli.rs#parse_reference"
        ));
        assert!(filter_matches(
            "src/cli.rs#parse_reference",
            "src/cli.rs#parse_reference"
        ));
        assert!(!filter_matches(
            "src/cli.rs#parse_reference",
            "parse_reference"
        ));
        assert!(!filter_matches("other", "src/cli.rs#parse_reference"));
    }

    #[test]
    fn push_json_str_escapes_quotes_backslashes_and_controls() {
        let mut quoted = String::new();
        push_json_str(&mut quoted, r#"a"b\c"#);
        assert_eq!(quoted, r#""a\"b\\c""#);

        let mut whitespace = String::new();
        push_json_str(&mut whitespace, "tab\tnl\n");
        assert_eq!(whitespace, r#""tab\tnl\n""#);

        // A C0 control char becomes a \uXXXX escape; the raw byte must not
        // survive.
        let mut control = String::new();
        push_json_str(&mut control, "\u{1}");
        assert!(
            control.contains("u0001"),
            "control char should escape: {control}"
        );
        assert!(
            !control.contains(char::from_u32(1).unwrap()),
            "raw control byte must not survive"
        );
    }

    #[test]
    fn at_json_escapes_anchor_text() {
        // A scenario-style anchor may carry quotes; the marker composes
        // wrap() with JSON escaping.
        let h = hit(r#"x.feature#say "hi""#, "x.feature", 3, 3);
        let forms = vec![(h.anchor.clone(), &h)];
        let doc = at_json(&forms);
        assert!(doc.contains(r#""anchor":"x.feature#say \"hi\"""#), "{doc}");
        assert!(
            doc.contains(r#""marker":"[[rr:x.feature#say \"hi\"]]""#),
            "{doc}"
        );
    }
}
