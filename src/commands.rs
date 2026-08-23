/*!
Command implementations for four of the five verbs of `[[rr:AD-3]]`: the
single writer (`index`), the index readers (`read`, `at`), and the lexical
lister (`search`). The gate is [`crate::verify`].

Each returns `Ok(exit_code)` for a normal outcome (including adverse and
stale, which are non-zero but not errors) or `Err(message)` for a
usage-level failure the caller reports as exit `2`. Output shapes and exit
codes follow `[[rr:AD-4]]`.
*/

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use memmap2::Mmap;

use crate::atomic;
use crate::cli::{self, LowArgs, OutputFormat};
use crate::config;
use crate::exit;
use crate::indexer;
use crate::messages;
use crate::output::{
    at_json, at_text, emit, envelope, push_json_str, push_location, SearchSink,
};
use crate::refidx::{self, AnchorHit, Location, Reader};
use crate::scan::{self, What};

/// Build or refresh the index from the working tree.
/// `[[rr:help_text]]`
pub(crate) fn run_index(args: &LowArgs) -> Result<u8, String> {
    let root = Path::new(".");
    let index_path = PathBuf::from(cli::index_path(args));
    let cfg = config::load(root)?;
    let scope = config::scope_matcher(root, &cfg)?;

    let data = indexer::build(root, &index_path, &scope, &cfg);
    let bytes = refidx::serialize(&data);

    if let Some(parent) = index_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| {
                format!("failed to create {}: {e}", parent.display())
            })?;
        }
    }
    // A reader mmaps this file, so it must see either the whole old index or
    // the whole new one, never a torn half-write. [[rr:atomic_write]]
    atomic::atomic_write(&index_path, &bytes).map_err(|e| {
        format!("failed to write {}: {e}", index_path.display())
    })?;

    emit(exit::OK, |w| match args.format {
        OutputFormat::Json => writeln!(
            w,
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
        OutputFormat::Text if args.quiet => Ok(()),
        OutputFormat::Text => writeln!(
            w,
            "indexed {} anchors and {} path mentions across {} files",
            data.forward.len(),
            data.mentions.len(),
            data.paths.len()
        ),
    })
}

/// The `Reader` borrows the mmap, so it cannot be returned past its backing
/// buffer; a closure keeps both alive for the call.
pub(crate) fn with_fresh_reader<F>(
    index_path: &Path,
    root: &Path,
    skip_freshness: bool,
    f: F,
) -> Result<u8, String>
where
    F: FnOnce(&Reader) -> Result<u8, String>,
{
    let Some(bytes) = read_index(index_path)? else {
        messages::warn(format_args!(
            "no index at {}: run `rr index`",
            index_path.display()
        ));
        return Ok(exit::STALE);
    };
    let reader =
        Reader::parse(&bytes).map_err(|e| format!("corrupt index: {e}"))?;
    if !fresh(&reader, root, skip_freshness) {
        messages::warn("index is stale: rebuild with `rr index`");
        return Ok(exit::STALE);
    }
    f(&reader)
}

/// The mapping is released (copied into a `Vec`) before the caller checks
/// freshness, because that check may spawn `git status` and, on Windows, a
/// concurrent `rr index` replaces this file; holding a mapping across that is
/// the fragile case. The atomic write in `rr index` is what actually prevents
/// a torn read; copying out is defense in depth, plus an `fs::read` fallback
/// for the rare platform where mmap of a valid file fails to open.
fn read_index(index_path: &Path) -> Result<Option<Vec<u8>>, String> {
    let file = match std::fs::File::open(index_path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(format!(
                "failed to open {}: {e}",
                index_path.display()
            ))
        }
    };
    // SAFETY: the index is a regular file we just opened; `rr index`
    // publishes new contents with an atomic rename, so the mapped inode
    // is always a complete image and is never mutated under us.
    #[allow(unsafe_code)]
    let bytes = match unsafe { Mmap::map(&file) } {
        Ok(mmap) => mmap.to_vec(),
        Err(_) => std::fs::read(index_path).map_err(|e| {
            format!("failed to read {}: {e}", index_path.display())
        })?,
    };
    Ok(Some(bytes))
}

/// Whether the index may answer this query (`[[rr:ripref (rr)#Freshness]]`).
/// The git probe spawns one `git status`, so it runs only after the free
/// checks and only when a clean-tree stamp exists to match.
fn fresh(reader: &Reader, root: &Path, skip_freshness: bool) -> bool {
    if skip_freshness {
        return true;
    }
    if !reader.tree.is_empty() && indexer::git_tree(root) == reader.tree {
        return true;
    }
    indexer::newest_mtime(&reader.paths(), root) <= reader.mtime
}

fn parse_all<'a>(locs: Vec<&'a str>) -> Vec<Location<'a>> {
    locs.into_iter()
        .filter_map(refidx::parse_location)
        .collect()
}

/// `[[rr:AD-6#Decision outcome]]`
pub(crate) fn resolve<'a>(
    reader: &Reader<'a>,
    anchor: &str,
) -> Vec<Location<'a>> {
    let direct = parse_all(reader.forward_lookup(anchor));
    if !direct.is_empty() {
        return direct;
    }
    let Some((qualifier, identity)) = cli::split_qualifier(anchor) else {
        return Vec::new();
    };
    let definitions = parse_all(reader.forward_lookup(identity));
    let by_path: Vec<Location> = definitions
        .iter()
        .filter(|loc| loc.file == qualifier)
        .copied()
        .collect();
    if !by_path.is_empty() {
        return by_path;
    }
    let scope = parse_all(reader.forward_lookup(qualifier));
    let [scope] = scope.as_slice() else {
        return Vec::new();
    };
    definitions
        .into_iter()
        .filter(|loc| {
            loc.file == scope.file
                && scope.start_line <= loc.start_line
                && loc.end_line <= scope.end_line
                && (loc.start_line, loc.end_line)
                    != (scope.start_line, scope.end_line)
        })
        .collect()
}

/// The form to print, and how many definitions it lands on when that is not
/// this hit; `None` is the checked case, so `run_at` never resolves again.
/// `[[rr:AD-6#Decision outcome]]`
fn minimal_form(reader: &Reader, hit: &AnchorHit) -> (String, Option<usize>) {
    let target = Location {
        file: &hit.file,
        start_line: hit.start_line,
        end_line: hit.end_line,
    };
    if resolve(reader, &hit.anchor).as_slice() == [target] {
        return (hit.anchor.clone(), None);
    }
    let enclosing = reader
        .covering(&hit.file, hit.start_line)
        .into_iter()
        .filter(|q| {
            q.start_line <= hit.start_line
                && hit.end_line <= q.end_line
                && (q.start_line, q.end_line) != (hit.start_line, hit.end_line)
        })
        .filter(|q| reader.forward_lookup(&q.anchor).len() == 1)
        .map(|q| format!("{}#{}", q.anchor, hit.anchor))
        .find(|form| resolve(reader, form).as_slice() == [target]);
    if let Some(form) = enclosing {
        return (form, None);
    }
    let fallback = format!("{}#{}", hit.file, hit.anchor);
    let found = resolve(reader, &fallback);
    let stray = (found.as_slice() != [target]).then_some(found.len());
    (fallback, stray)
}

/// `[[rr:help_text]]`. The reader strips a pasted marker's wrapper and
/// unescapes before resolving `[[rr:AD-2]]`; a token that opens like a
/// marker but is not one is a usage error, never a silent reparse.
pub(crate) fn run_read(args: &LowArgs) -> Result<u8, String> {
    let root = Path::new(".");
    let index_path = PathBuf::from(cli::index_path(args));
    let token = args.positional[0].to_string_lossy().into_owned();
    let anchor = cli::parse_reference(&token)?;

    with_fresh_reader(&index_path, root, args.no_freshness, |reader| {
        let locations = resolve(reader, &anchor);
        let code = if locations.len() == 1 {
            exit::OK
        } else {
            exit::ADVERSE
        };
        let code = emit(code, |w| {
            if args.format == OutputFormat::Json {
                let mut data = String::from(r#"{"anchor":"#);
                push_json_str(&mut data, &anchor);
                data.push_str(",\"locations\":[");
                for (i, loc) in locations.iter().enumerate() {
                    if i > 0 {
                        data.push(',');
                    }
                    push_location(
                        &mut data,
                        loc.file,
                        loc.start_line,
                        loc.end_line,
                    );
                }
                data.push_str("]}");
                writeln!(w, "{}", envelope("read", &data))
            } else {
                for loc in &locations {
                    writeln!(
                        w,
                        "{}:{}-{}",
                        loc.file, loc.start_line, loc.end_line
                    )?;
                }
                Ok(())
            }
        })?;
        match locations.len() {
            0 => messages::warn(format_args!("no such anchor: '{anchor}'")),
            1 => {}
            n => messages::warn(format_args!(
                "ambiguous anchor: '{anchor}' resolves to {n} definitions (add a qualifier)"
            )),
        }
        Ok(code)
    })
}

/// `[[rr:help_text]]`. The inverse of `run_read`.
pub(crate) fn run_at(args: &LowArgs) -> Result<u8, String> {
    let root = Path::new(".");
    let index_path = PathBuf::from(cli::index_path(args));
    // `validate` already accepted this; re-parsing here keeps the position
    // in one place rather than threading a parsed field through `LowArgs`.
    let (file, line) =
        cli::parse_position(&args.positional[0].to_string_lossy())?;

    with_fresh_reader(&index_path, root, args.no_freshness, |reader| {
        let hits = reader.covering(&file, line);
        // The innermost tie set: every hit sharing the tightest span.
        let emitted: Vec<&AnchorHit> = if args.all {
            hits.iter().collect()
        } else if let Some(last) = hits.last() {
            hits.iter()
                .filter(|h| {
                    h.start_line == last.start_line
                        && h.end_line == last.end_line
                })
                .collect()
        } else {
            Vec::new()
        };
        let (forms, strays): (Vec<(String, &AnchorHit)>, Vec<Option<usize>>) =
            emitted
                .iter()
                .map(|h| {
                    let (form, stray) = minimal_form(reader, h);
                    ((form, *h), stray)
                })
                .unzip();

        // [[rr:AD-4#Decision outcome]]
        let uninvertible: Vec<(&str, usize)> = forms
            .iter()
            .zip(&strays)
            .filter_map(|((form, _), stray)| stray.map(|n| (form.as_str(), n)))
            .collect();

        let code = if forms.is_empty()
            || (!args.all && forms.len() > 1)
            || !uninvertible.is_empty()
        {
            exit::ADVERSE
        } else {
            exit::OK
        };
        let code = emit(code, |w| match args.format {
            OutputFormat::Json => {
                writeln!(w, "{}", envelope("at", &at_json(&forms)))
            }
            OutputFormat::Text if forms.is_empty() => Ok(()),
            OutputFormat::Text => writeln!(w, "{}", at_text(&forms)),
        })?;
        if forms.is_empty() {
            messages::warn(format_args!("no anchor covers {file}:{line}"));
        } else if !args.all && forms.len() > 1 {
            messages::warn(format_args!(
                "ambiguous: {} anchors tie on the innermost span",
                forms.len()
            ));
        } else {
            for (form, n) in &uninvertible {
                messages::warn(format_args!(
                    "ambiguous marker for {file}:{line}: '{form}' resolves to \
                     {n} definitions (retitle one)"
                ));
            }
        }
        Ok(code)
    })
}

pub(crate) struct ScopedFile {
    pub rel: String,
    pub content: String,
    pub host: scan::Host,
}

/// `[[rr:AD-3]]`, shown to a user in `[[rr:Quick examples]]`.
/// Never asks the filesystem, so a named symlink keeps the spelling the
/// caller wrote rather than its target's. `None` when the path climbs past
/// the root: that is the bound check.
fn normalize_lexically(rel: &str) -> Option<String> {
    let mut parts: Vec<&str> = Vec::new();
    for part in rel.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            part => parts.push(part),
        }
    }
    Some(parts.join("/"))
}

/// Only the directory chain is resolved, because `/tmp` and `/private/tmp`
/// must meet; the last component keeps its name, so a symlinked file still
/// reports as itself.
fn absolute_to_tree_path(
    given: &str,
    root_abs: &Path,
) -> Result<String, String> {
    let path = Path::new(given);
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
        return Err(format!("outside the tree: {given}"));
    };
    let parent = parent
        .canonicalize()
        .map_err(|e| format!("cannot read {given}: {e}"))?;
    Ok(parent
        .join(name)
        .strip_prefix(root_abs)
        .map_err(|_| format!("outside the tree: {given}"))?
        .to_string_lossy()
        .replace('\\', "/"))
}

pub(crate) fn scoped_files(
    root: &Path,
    matcher: &ignore::overrides::Override,
    cfg: &config::Config,
    paths: &[String],
) -> Result<Vec<ScopedFile>, String> {
    if !paths.is_empty() {
        let root_abs = root
            .canonicalize()
            .map_err(|e| format!("cannot resolve {}: {e}", root.display()))?;
        let mut out = Vec::with_capacity(paths.len());
        let mut seen = HashSet::new();
        for raw in paths {
            let given = raw.replace('\\', "/");
            let named = if Path::new(&given).is_absolute() {
                absolute_to_tree_path(&given, &root_abs)?
            } else {
                given.clone()
            };
            let rel = normalize_lexically(&named)
                .ok_or_else(|| format!("outside the tree: {given}"))?;
            let abs = root.join(&rel);
            if !abs.is_file() {
                return Err(format!("not a file: {given}"));
            }
            if !seen.insert(rel.clone()) {
                continue;
            }
            let content = std::fs::read_to_string(&abs)
                .map_err(|e| format!("cannot read {given}: {e}"))?;
            let ext = rel.rsplit('.').next();
            out.push(ScopedFile {
                host: scan::host_for(ext, cfg),
                rel,
                content,
            });
        }
        return Ok(out);
    }

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
        let content = match std::fs::read_to_string(dent.path()) {
            Ok(content) => content,
            // A binary file fails as InvalidData, and is not scoped text.
            Err(e) if e.kind() == std::io::ErrorKind::InvalidData => continue,
            Err(e) => {
                messages::error(format_args!("{rel}: {e}"));
                continue;
            }
        };
        let ext = rel.rsplit('.').next();
        out.push(ScopedFile {
            host: scan::host_for(ext, cfg),
            rel,
            content,
        });
    }
    out.sort_by(|a, b| a.rel.cmp(&b.rel));
    Ok(out)
}

/// `[[rr:help_text]]`. Purely lexical: no index is read, so it never
/// returns stale.
pub(crate) fn run_search(args: &LowArgs) -> Result<u8, String> {
    let root = Path::new(".");
    let cfg = config::load(root)?;
    let matcher = config::scope_matcher(root, &cfg)?;
    // [[rr:AD-3#Decision outcome]]
    let takes_anchor = !(args.markers || args.mentions);
    let (filter, paths) = match args.positional.split_first() {
        Some((first, rest)) if takes_anchor => (
            Some(cli::parse_reference(&first.to_string_lossy())?),
            rest.to_vec(),
        ),
        _ => (None, args.positional.clone()),
    };
    let paths: Vec<String> = paths
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();

    let mut sink = SearchSink::new(args.format);
    for file in scoped_files(root, &matcher, &cfg, &paths)? {
        for found in scan::scan(&file.content, file.host) {
            match (&found.what, args.mentions) {
                (What::Marker { raw, anchor }, false) => {
                    if let Some(want) = &filter {
                        if !filter_matches(want, anchor) {
                            continue;
                        }
                    }
                    sink.marker(&file.rel, found.line, anchor, raw);
                }
                (What::Mention { token, .. }, true) => {
                    sink.mention(&file.rel, found.line, token);
                }
                _ => {}
            }
        }
    }
    let count = sink.count();
    let body = sink.finish();

    let code = if count > 0 { exit::OK } else { exit::ADVERSE };
    emit(code, |w| {
        if args.format == OutputFormat::Json {
            writeln!(w, "{}", envelope("search", &body))
        } else {
            w.write_all(body.as_bytes())?;
            if args.quiet {
                return Ok(());
            }
            writeln!(
                w,
                "{count} {}",
                if args.mentions { "mentions" } else { "markers" }
            )
        }
    })
}

/// Whether a search filter matches a decoded marker anchor: an unqualified
/// argument matches every marker whose identity equals it, path-qualified or
/// not; a qualified argument matches exactly `[[rr:AD-3]]`.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_lexically_folds_dots_and_bounds_at_the_root() {
        assert_eq!(normalize_lexically("a.md").unwrap(), "a.md");
        assert_eq!(normalize_lexically("./a.md").unwrap(), "a.md");
        assert_eq!(normalize_lexically("d/../a.md").unwrap(), "a.md");
        assert_eq!(normalize_lexically("d//e/./f.md").unwrap(), "d/e/f.md");
        assert!(normalize_lexically("../a.md").is_none());
        assert!(normalize_lexically("d/../../a.md").is_none());
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
}
