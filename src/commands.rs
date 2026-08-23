/*!
Command implementations for the five verbs of `[[rr:AD-3]]`: the single
writer (`index`), the index readers (`read`, `at`), the lexical lister
(`search`), and the gate (`verify`).

Each returns `Ok(exit_code)` for a normal outcome (including adverse and
stale, which are non-zero but not errors) or `Err(message)` for a
usage-level failure the caller reports as exit `2`. Output shapes and exit
codes follow `[[rr:AD-4]]`.
*/

use std::collections::HashSet;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use memmap2::Mmap;

use crate::atomic;
use crate::cli::{self, LowArgs, OutputFormat};
use crate::config;
use crate::exit;
use crate::indexer;
use crate::marker;
use crate::messages;
use crate::refidx::{self, AnchorHit, Reader};
use crate::scan::{self, What};

/// Build or refresh the index from the working tree.
/// `[[rr:help_text]]`
pub fn run_index(args: &LowArgs) -> Result<u8, String> {
    let root = Path::new(".");
    let index_path = PathBuf::from(cli::index_path(args));
    let cfg = config::load(root)?;
    let scope = config::scope_matcher(root, &cfg)?;

    let data = indexer::build(root, &index_path, &scope, &cfg)
        .map_err(|e| format!("failed to walk the working tree: {e}"))?;
    let bytes = refidx::serialize(&data);

    if let Some(parent) = index_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create {}: {e}", parent.display()))?;
        }
    }
    // A reader mmaps this file, so it must see either the whole old index or
    // the whole new one, never a torn half-write. [[rr:atomic_write]]
    atomic::atomic_write(&index_path, &bytes)
        .map_err(|e| format!("failed to write {}: {e}", index_path.display()))?;

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
            eprintln!("no index at {}: run `rr index`", index_path.display());
            Ok(exit::STALE)
        }
        IndexState::Stale => {
            eprintln!("index is stale: rebuild with `rr index`");
            Ok(exit::STALE)
        }
        IndexState::Fresh(bytes) => {
            let reader = Reader::parse(&bytes).map_err(|e| format!("corrupt index: {e}"))?;
            f(&reader)
        }
    }
}

enum IndexState {
    Fresh(Vec<u8>),
    Missing,
    /// `[[rr:ripref (rr)#Freshness]]`
    Stale,
}

/// The mapping is released (copied into a `Vec`) before `fresh` runs,
/// because `fresh` may spawn `git status` and, on Windows, a concurrent
/// `rr index` replaces this file; holding a mapping across that is the
/// fragile case. The atomic write in `rr index` is what actually prevents a
/// torn read; copying out is defense in depth, plus an `fs::read` fallback
/// for the rare platform where mmap of a valid file fails to open.
fn load_index(index_path: &Path, root: &Path, skip_freshness: bool) -> Result<IndexState, String> {
    let file = match std::fs::File::open(index_path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(IndexState::Missing);
        }
        Err(e) => return Err(format!("failed to open {}: {e}", index_path.display())),
    };
    let bytes: Vec<u8> = {
        // SAFETY: the index is a regular file we just opened; `rr index`
        // publishes new contents with an atomic rename, so the mapped inode
        // is always a complete image and is never mutated under us.
        #[allow(unsafe_code)]
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

type Location = (String, u64, u64);

fn parse_all(locs: Vec<String>) -> Vec<Location> {
    locs.iter()
        .filter_map(|l| refidx::parse_location(l))
        .map(|(f, s, e)| (f.to_string(), s, e))
        .collect()
}

/// `[[rr:AD-6#Decision outcome]]`
fn resolve(reader: &Reader, anchor: &str) -> Vec<Location> {
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
        .filter(|(f, _, _)| f == qualifier)
        .cloned()
        .collect();
    if !by_path.is_empty() {
        return by_path;
    }
    let scope = parse_all(reader.forward_lookup(qualifier));
    let [(file, start, end)] = scope.as_slice() else {
        return Vec::new();
    };
    definitions
        .into_iter()
        .filter(|(f, s, e)| f == file && start <= s && e <= end && (s, e) != (start, end))
        .collect()
}

/// `[[rr:AD-6#Decision outcome]]`
fn minimal_form(reader: &Reader, hit: &AnchorHit) -> String {
    // A candidate that resolves to exactly one definition is not enough: the
    // one it lands on has to be this hit.
    let target: Location = (hit.file.clone(), hit.start_line, hit.end_line);
    if reader.forward_lookup(&hit.anchor).len() == 1 {
        return hit.anchor.clone();
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
        .find(|form| resolve(reader, form).as_slice() == [target.clone()]);
    enclosing.unwrap_or_else(|| format!("{}#{}", hit.file, hit.anchor))
}

/// `[[rr:help_text]]`. The reader strips a pasted marker's wrapper and
/// unescapes before resolving `[[rr:AD-2]]`; a token that opens like a
/// marker but is not one is a usage error, never a silent reparse.
pub fn run_read(args: &LowArgs) -> Result<u8, String> {
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
                    push_location(&mut data, &loc.0, loc.1, loc.2);
                }
                data.push_str("]}");
                writeln!(w, "{}", envelope("read", &data))
            } else {
                for (file, start, end) in &locations {
                    writeln!(w, "{file}:{start}-{end}")?;
                }
                Ok(())
            }
        })?;
        match locations.len() {
            0 => eprintln!("no such anchor: {anchor}"),
            1 => {}
            n => eprintln!(
                "ambiguous anchor: {anchor} resolves to {n} definitions (add a qualifier)"
            ),
        }
        Ok(code)
    })
}

/// `[[rr:help_text]]`. The inverse of `run_read`.
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
        let forms: Vec<(String, &AnchorHit)> = emitted
            .iter()
            .map(|h| (minimal_form(reader, h), *h))
            .collect();

        // `minimal_form` tests each candidate, but returns its path fallback
        // unchecked, so the printed form may still not invert.
        // [[rr:AD-4#Decision outcome]]
        let uninvertible: Vec<(&str, usize)> = forms
            .iter()
            .filter_map(|(form, h)| {
                let target: Location = (h.file.clone(), h.start_line, h.end_line);
                let found = resolve(reader, form);
                (found.as_slice() != [target]).then_some((form.as_str(), found.len()))
            })
            .collect();

        let code = if forms.is_empty() || (!args.all && forms.len() > 1) || !uninvertible.is_empty()
        {
            exit::ADVERSE
        } else {
            exit::OK
        };
        let code = emit(code, |w| match args.format {
            OutputFormat::Json => writeln!(w, "{}", envelope("at", &at_json(&forms))),
            OutputFormat::Text if forms.is_empty() => Ok(()),
            OutputFormat::Text => writeln!(w, "{}", at_text(&forms)),
        })?;
        if forms.is_empty() {
            eprintln!("no anchor covers {file}:{line}");
        } else if !args.all && forms.len() > 1 {
            eprintln!(
                "ambiguous: {} anchors tie on the innermost span",
                forms.len()
            );
        } else {
            for (form, n) in &uninvertible {
                eprintln!(
                    "ambiguous marker for {file}:{line}: {form} resolves to \
                     {n} definitions (retitle one)"
                );
            }
        }
        Ok(code)
    })
}

/// Text rendering for `rr at`: one marker per line, the document form a
/// person pastes `[[rr:AD-4]]`. Returned rather than printed so it is
/// unit-testable; `run_at` does the I/O.
fn at_text(forms: &[(String, &AnchorHit)]) -> String {
    forms
        .iter()
        .map(|(form, _)| marker::wrap(form))
        .collect::<Vec<_>>()
        .join("\n")
}

/// JSON `data` for `rr at`
/// (`[[rr:AD-4#Decision outcome]]`). Returned (not
/// printed) so the exact document can be asserted in tests.
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

struct ScopedFile {
    rel: String,
    content: String,
    host: scan::Host,
}

/// `[[rr:AD-3]]`, shown to a user in `[[rr:Quick examples]]`.
fn scoped_files(
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
            let abs = root.join(&given);
            if !abs.is_file() {
                return Err(format!("not a file: {given}"));
            }
            // Canonicalize to fold away `./` and `..` before the bound check,
            // so no spelling of a path reaches past the root.
            let abs = abs
                .canonicalize()
                .map_err(|e| format!("cannot read {given}: {e}"))?;
            let rel = abs
                .strip_prefix(&root_abs)
                .map_err(|_| format!("outside the tree: {given}"))?
                .to_string_lossy()
                .replace('\\', "/");
            if !seen.insert(rel.clone()) {
                continue;
            }
            let content =
                std::fs::read_to_string(&abs).map_err(|e| format!("cannot read {given}: {e}"))?;
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
pub fn run_search(args: &LowArgs) -> Result<u8, String> {
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

    let mut lines = Vec::new();
    let mut json = String::from(r#"{"matches":["#);
    let mut count = 0usize;
    for file in scoped_files(root, &matcher, &cfg, &paths)? {
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

    let code = if count > 0 { exit::OK } else { exit::ADVERSE };
    emit(code, |w| {
        if args.format == OutputFormat::Json {
            writeln!(w, "{}", envelope("search", &json))
        } else {
            for line in &lines {
                writeln!(w, "{line}")?;
            }
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

/// One of the six finding kinds of `[[rr:AD-3]]`, selected by the name a
/// profile writes in `[[rr:Configuration]]`, beside the text a person reads.
#[derive(Clone, Copy)]
struct Rule {
    name: &'static str,
    text: &'static str,
}

const MALFORMED: Rule = Rule {
    name: "malformed-marker",
    text: "malformed marker",
};
const DANGLING: Rule = Rule {
    name: "dangling-marker",
    text: "dangling marker",
};
const AMBIGUOUS: Rule = Rule {
    name: "ambiguous-marker",
    text: "ambiguous marker",
};
const PATH_ONLY: Rule = Rule {
    name: "path-only-marker",
    text: "path-only marker",
};
const PATH_LINE: Rule = Rule {
    name: "path-line",
    text: "bare path:line reference",
};
const STALE_MENTION: Rule = Rule {
    name: "stale-mention",
    text: "stale path mention",
};
const RULES: &[Rule] = &[
    MALFORMED,
    DANGLING,
    AMBIGUOUS,
    PATH_ONLY,
    PATH_LINE,
    STALE_MENTION,
];

/// One `verify` finding.
struct Finding {
    file: String,
    line: u64,
    rule: Rule,
    detail: String,
}

/// `[[rr:help_text]]`, reporting the six kinds of `[[rr:AD-3]]`. Resolution
/// judgments need the index, so a stale index exits 3 rather than judging
/// from stale data; mention judgments run against the live tree.
pub fn run_verify(args: &LowArgs) -> Result<u8, String> {
    let root = Path::new(".");
    let index_path = PathBuf::from(cli::index_path(args));
    let cfg = config::load(root)?;
    let matcher = config::scope_matcher(root, &cfg)?;
    let paths: Vec<String> = args
        .positional
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    // Before the index is touched, so a typo is a usage error rather than
    // whatever the index's state would have reported. [[rr:Configuration]]
    if let Some(unknown) = cfg
        .verify_rules
        .iter()
        .find(|n| !RULES.iter().any(|r| r.name == n.as_str()))
    {
        return Err(format!("unknown verify rule: {unknown}"));
    }

    with_fresh_reader(&index_path, root, args.no_freshness, |reader| {
        let mut findings: Vec<Finding> = Vec::new();
        for file in scoped_files(root, &matcher, &cfg, &paths)? {
            for found in scan::scan(&file.content, file.host) {
                let (rule, detail) = match &found.what {
                    What::Malformed { reason } => (MALFORMED, reason.clone()),
                    What::Marker { raw, anchor } => {
                        if !anchor.contains('#') && scan::is_path_shaped(anchor) {
                            (PATH_ONLY, raw.clone())
                        } else {
                            match resolve(reader, anchor).len() {
                                0 => (DANGLING, raw.clone()),
                                1 => continue,
                                n => (AMBIGUOUS, format!("{raw} resolves to {n} definitions")),
                            }
                        }
                    }
                    What::Mention { token, line_ref } => {
                        // [[rr:AD-5#Decision outcome]]
                        let first = token.split('/').next().unwrap_or("");
                        if first.is_empty() || !root.join(first).is_dir() {
                            continue;
                        }
                        if *line_ref {
                            (PATH_LINE, token.clone())
                        } else if !root.join(token).exists() {
                            (STALE_MENTION, token.clone())
                        } else {
                            continue;
                        }
                    }
                };
                if !cfg.verify_rules.iter().any(|n| n == rule.name) {
                    continue;
                }
                findings.push(Finding {
                    file: file.rel.clone(),
                    line: found.line,
                    rule,
                    detail,
                });
            }
        }

        let code = if findings.is_empty() {
            exit::OK
        } else {
            exit::ADVERSE
        };
        emit(code, |w| {
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
                    push_json_str(&mut data, f.rule.text);
                    data.push('}');
                }
                data.push_str("]}");
                writeln!(w, "{}", envelope("verify", &data))
            } else {
                for f in &findings {
                    writeln!(w, "{}:{}: {}: {}", f.file, f.line, f.rule.text, f.detail)?;
                }
                if args.quiet {
                    return Ok(());
                }
                writeln!(w, "{} findings", findings.len())
            }
        })
    })
}

fn emit(code: u8, write: impl FnOnce(&mut dyn Write) -> std::io::Result<()>) -> Result<u8, String> {
    let stdout = std::io::stdout();
    emit_to(BufWriter::new(stdout.lock()), code, write)
}

/// The `pipefail` contract of `[[rr:Shared options]]`: a reader that closed
/// the pipe has what it asked for, so `BrokenPipe` keeps `code`. Flushed
/// explicitly because `BufWriter`'s drop discards the error this exists to
/// catch. Generic over the writer so both branches are reachable without a
/// real pipe.
fn emit_to<W: Write>(
    mut out: W,
    code: u8,
    write: impl FnOnce(&mut dyn Write) -> std::io::Result<()>,
) -> Result<u8, String> {
    match write(&mut out).and_then(|()| out.flush()) {
        Ok(()) => Ok(code),
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => Ok(code),
        Err(e) => Err(format!("cannot write to stdout: {e}")),
    }
}

/// The one `rr-json` envelope every verb prints under `--format json`
/// `[[rr:AD-4]]`. Hand-rolled because the crate has no serde dependency
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

/// `[[rr:AD-2#Decision drivers]]`
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

    /// Fails one operation with one kind, so every `emit_to` branch is
    /// reachable without a real pipe. `on_flush` writes cleanly and fails
    /// only at the flush, which is where a buffered error actually surfaces.
    struct FailingWriter {
        kind: std::io::ErrorKind,
        writes_ok: bool,
    }

    impl FailingWriter {
        fn on_write(kind: std::io::ErrorKind) -> Self {
            FailingWriter {
                kind,
                writes_ok: false,
            }
        }
        fn on_flush(kind: std::io::ErrorKind) -> Self {
            FailingWriter {
                kind,
                writes_ok: true,
            }
        }
    }

    impl Write for FailingWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            if self.writes_ok {
                Ok(buf.len())
            } else {
                Err(std::io::Error::from(self.kind))
            }
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Err(std::io::Error::from(self.kind))
        }
    }

    #[test]
    fn emit_keeps_the_code_on_broken_pipe_and_errors_otherwise() {
        use std::io::ErrorKind::{BrokenPipe, PermissionDenied};

        let mut sink = Vec::new();
        assert_eq!(
            emit_to(&mut sink, exit::ADVERSE, |w| writeln!(w, "one")),
            Ok(exit::ADVERSE),
            "the answer's code survives a whole write"
        );
        assert_eq!(sink, b"one\n");

        for hung_up in [
            FailingWriter::on_write(BrokenPipe),
            FailingWriter::on_flush(BrokenPipe),
        ] {
            assert_eq!(
                emit_to(hung_up, exit::ADVERSE, |w| writeln!(w, "one")),
                Ok(exit::ADVERSE),
                "a reader that stopped reading still got its answer"
            );
        }

        for refused in [
            FailingWriter::on_write(PermissionDenied),
            FailingWriter::on_flush(PermissionDenied),
        ] {
            let err = emit_to(refused, exit::ADVERSE, |w| writeln!(w, "one"));
            assert!(err.is_err(), "any other kind is a real failure: {err:?}");
        }
    }

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
        // [[rr:AD-2#Decision drivers]]
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
