/*!
Command implementations: the single writer (`index`) and one reader (`read`).

Each returns `Ok(exit_code)` for a normal outcome (including findings/stale,
which are non-zero but not errors) or `Err(message)` for a usage-level failure
the caller reports as exit `2`.
*/

use std::io::Write;
use std::path::{Path, PathBuf};

use memmap2::Mmap;

use crate::atomic;
use crate::cli::{self, LowArgs, OutputFormat, Reference, Sigil};
use crate::exit;
use crate::git;
use crate::indexer;
use crate::refidx::{self, AnchorHit, Reader};
use crate::sidecar::{self, Content, Kind, Manifest, Pin, Record, ResolveError, Sidecar, Tomb};

/// `rr index` — build/refresh the index from the working tree.
pub fn run_index(args: &LowArgs) -> Result<u8, String> {
    let root = Path::new(".");
    let index_path = PathBuf::from(cli::index_path(args));

    let data = indexer::build(root, &index_path)
        .map_err(|e| format!("failed to walk the working tree: {e}"))?;
    let bytes = refidx::serialize(&data);

    if let Some(parent) = index_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create {}: {e}", parent.display()))?;
        }
    }
    // Atomic replace, not a truncate-in-place: a reader (which mmaps this file)
    // must see either the whole old index or the whole new one, never a torn
    // half-write. See `crate::atomic`.
    atomic::atomic_write(&index_path, &bytes)
        .map_err(|e| format!("failed to write {}: {e}", index_path.display()))?;

    if !args.quiet {
        println!(
            "indexed {} anchors across {} files",
            data.forward.len(),
            data.paths.len()
        );
    }
    Ok(exit::OK)
}

/// Open the index, mmap it, parse the header, and gate on freshness before
/// handing a [`Reader`] to `f`. This is the shared preamble of every reader
/// command: a missing or stale index short-circuits to `exit::STALE` without
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
/// missing or stale one is a non-error state the caller maps to its own exit.
///
/// The mapping is released (copied into a `Vec`) before `fresh` runs, because
/// `fresh` may spawn `git status` and, on Windows, a concurrent `rr index`
/// replaces this file; holding a mapping across that is the fragile case. The
/// atomic write in `rr index` is what actually prevents a torn read; copying out
/// is defense in depth, plus an `fs::read` fallback for the rare platform where
/// mmap of a valid file fails to open.
fn load_index(index_path: &Path, root: &Path, skip_freshness: bool) -> Result<IndexState, String> {
    let file = match std::fs::File::open(index_path) {
        Ok(f) => f,
        Err(_) => return Ok(IndexState::Missing),
    };
    let bytes: Vec<u8> = {
        // SAFETY: the index is a regular file we just opened; `rr index`
        // publishes new contents with an atomic rename, so the mapped inode is
        // always a complete image and is never mutated under us.
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

/// Whether the index may answer this query. Cheap-first: (1) `--no-freshness`
/// trusts unconditionally; (2) a clean tree whose HEAD SHA matches the stamp is
/// provably what we indexed (no stat-walk needed); (3) else fall back to the
/// (now parallel) mtime walk. The git probe spawns one `git status`, so it runs
/// only after the free checks and only when a clean-tree stamp exists to match.
fn fresh(reader: &Reader, root: &Path, skip_freshness: bool) -> bool {
    if skip_freshness {
        return true;
    }
    if !reader.tree.is_empty() && indexer::git_tree(root) == reader.tree {
        return true;
    }
    indexer::newest_mtime(&reader.paths(), root) <= reader.mtime
}

/// `rr read <ref>` — dereference a live anchor, or a pinned `anchor@commit`
/// (snapshot) / `anchor~commit` (tracking) reference.
///
/// Resolution is **known-anchor-wins**: if the whole token is a live anchor it is
/// read literally (so an email heading `support@example.com`, an `@scope` path,
/// or a `~/path` heading still resolves), and only otherwise is the token split
/// on its last `@`/`~` into a pin. A snapshot recovers from `.rr/` independently
/// of the index and of git GC; tracking measures the current file against its
/// stored baseline.
pub fn run_read(args: &LowArgs) -> Result<u8, String> {
    let root = Path::new(".");
    let index_path = PathBuf::from(cli::index_path(args));
    let token = args.positional[0].to_string_lossy().into_owned();

    match cli::parse_reference(&token) {
        // No pin sigil: a plain live anchor read, gated on freshness as before.
        Reference::Plain(anchor) => {
            with_fresh_reader(&index_path, root, args.no_freshness, |reader| {
                live_read(reader, anchor)
            })
        }
        Reference::Pinned {
            anchor,
            sigil,
            commit,
        } => run_read_pinned(
            &index_path,
            root,
            args.no_freshness,
            &token,
            anchor,
            sigil,
            commit,
        ),
    }
}

/// Dereference one anchor through the forward map (the live read). Prints the
/// location for a unique hit; reports not-found / ambiguous otherwise.
fn live_read(reader: &Reader, anchor: &str) -> Result<u8, String> {
    let hits = reader.forward_lookup(anchor);
    match hits.len() {
        0 => {
            eprintln!("no such anchor: {anchor}");
            Ok(exit::FINDINGS)
        }
        1 => {
            // Only the location line is printed today; the source-body print and
            // `-C`/`--context` are not yet implemented for the live read.
            println!("{}", hits[0]);
            Ok(exit::OK)
        }
        n => {
            eprintln!("ambiguous anchor: {anchor} resolves to {n} definitions");
            for loc in &hits {
                eprintln!("  {loc}");
            }
            eprintln!("(use `rr search` to see the collisions)");
            Ok(exit::FINDINGS)
        }
    }
}

/// Resolve a `@`/`~` reference. Applies known-anchor-wins first (a stale or
/// missing index must not block a pin, so a freshness failure there only means
/// "could not confirm a live anchor" and we fall through), then resolves the pin
/// from the committed sidecar.
fn run_read_pinned(
    index_path: &Path,
    root: &Path,
    skip_freshness: bool,
    token: &str,
    anchor: &str,
    sigil: Sigil,
    commit: &str,
) -> Result<u8, String> {
    // known-anchor-wins: a fresh index in which the whole token is a live anchor
    // wins over any split. A missing/stale index leaves this unconfirmed.
    let index_was_usable = match load_index(index_path, root, skip_freshness)? {
        IndexState::Fresh(bytes) => {
            let reader = Reader::parse(&bytes).map_err(|e| format!("corrupt index: {e}"))?;
            if !reader.forward_lookup(token).is_empty() {
                return live_read(&reader, token);
            }
            true
        }
        IndexState::Missing | IndexState::Stale => false,
    };

    let sc = Sidecar::at(root);
    let manifest = sc.load()?;
    let kind = match sigil {
        Sigil::Snapshot => Kind::Snapshot,
        Sigil::Tracking => Kind::Track,
    };
    match manifest.resolve(kind, anchor, commit) {
        Ok(pin) => match sigil {
            Sigil::Snapshot => read_snapshot(&sc, pin),
            Sigil::Tracking => read_tracking(root, pin),
        },
        Err(ResolveError::NotFound) if !index_was_usable => {
            // The token might be a live anchor we could not confirm; ask for a
            // rebuild rather than calling it broken.
            eprintln!("index is stale — rebuild with `rr index`");
            Ok(exit::STALE)
        }
        Err(ResolveError::NotFound) => {
            eprintln!(
                "broken reference: no {} of {anchor} at {commit} (re-point with `rr {}`)",
                kind_noun(kind),
                repin_verb(kind),
            );
            Ok(exit::BROKEN)
        }
        Err(ResolveError::Ambiguous(shorts)) => {
            eprintln!(
                "broken reference: {anchor}{}{commit} is ambiguous ({} commits match: {})",
                sigil_char(sigil),
                shorts.len(),
                shorts.join(", "),
            );
            Ok(exit::BROKEN)
        }
    }
}

/// Print the frozen source a snapshot pin recovered from `.rr/`. Self-verifying:
/// the recovered bytes are re-hashed against the object name, so a corrupt or
/// missing object is reported broken rather than shown as evidence. No git, no
/// live tree, no reachable commit.
fn read_snapshot(sc: &Sidecar, pin: &Pin) -> Result<u8, String> {
    let Some(bytes) = sc.read_object(&pin.oid) else {
        eprintln!(
            "broken reference: snapshot object {} is missing from .rr/objects",
            pin.oid
        );
        return Ok(exit::BROKEN);
    };
    if sidecar::oid_of(&bytes) != pin.oid {
        eprintln!(
            "broken reference: snapshot object {} is corrupt (re-hash does not match its name)",
            pin.oid
        );
        return Ok(exit::BROKEN);
    }
    let header = format!("{}@{}:{}-{}", pin.path, pin.short, pin.start, pin.end);
    println!("{header}");
    match sidecar::classify(&bytes) {
        Content::Binary => {
            println!("[binary object, {} bytes — not shown]", bytes.len());
        }
        Content::Text => {
            // Write the recovered range as raw bytes, so a CRLF file recovers as
            // CRLF (the stored working-tree form), not normalized to LF.
            let slice = sidecar::slice_lines(&bytes, pin.start, pin.end);
            std::io::stdout()
                .write_all(slice)
                .map_err(|e| format!("failed to write snapshot output: {e}"))?;
        }
    }
    Ok(exit::OK)
}

/// Report whether a tracked anchor's file still matches its baseline. Drift is a
/// content comparison (`git hash-object` of the current file vs the stored
/// baseline OID), never `git diff` (which a skip-worktree / racy-clean / EOL
/// quirk can silently make lie). A renamed-but-identical file reads as moved, not
/// drifted.
fn read_tracking(root: &Path, pin: &Pin) -> Result<u8, String> {
    match track_status(root, pin) {
        TrackStatus::Clean => {
            println!("{}~{}: OK ({} unchanged)", pin.anchor, pin.short, pin.path);
            Ok(exit::OK)
        }
        TrackStatus::Moved(to) => {
            println!(
                "{}~{}: OK (moved {} -> {to}, content unchanged)",
                pin.anchor, pin.short, pin.path
            );
            Ok(exit::OK)
        }
        TrackStatus::Drifted(detail) => {
            eprintln!("{}~{}: DRIFTED ({detail})", pin.anchor, pin.short);
            Ok(exit::DRIFTED)
        }
        TrackStatus::Broken(reason) => {
            eprintln!("{}~{}: BROKEN ({reason})", pin.anchor, pin.short);
            Ok(exit::BROKEN)
        }
    }
}

/// The drift verdict for a tracking pin, shared by `read ~` and `verify`.
enum TrackStatus {
    Clean,
    Moved(String),
    Drifted(String),
    Broken(String),
}

fn track_status(root: &Path, pin: &Pin) -> TrackStatus {
    if root.join(&pin.path).is_file() {
        match git::hash_object(root, &pin.path) {
            Some(current) if current == pin.oid => TrackStatus::Clean,
            Some(_) => TrackStatus::Drifted(format!("{} changed since the baseline", pin.path)),
            None => TrackStatus::Broken(format!("cannot hash {}", pin.path)),
        }
    } else {
        // The pinned path is gone: the content may have moved unchanged. The
        // search hashes candidate files in process (no git fork per file), so it
        // does not inflate `verify`'s git-call budget.
        match find_by_content(root, &pin.oid) {
            Some(moved_to) => TrackStatus::Moved(moved_to),
            None => TrackStatus::Drifted(format!(
                "{} is gone and its content was not found",
                pin.path
            )),
        }
    }
}

/// Search the working tree for a file whose git blob id equals `oid`, returning
/// its repo-relative path. Used only on the rename/move fallback, when a tracked
/// path has disappeared.
fn find_by_content(root: &Path, oid: &str) -> Option<String> {
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
        if let Ok(bytes) = std::fs::read(dent.path()) {
            if sidecar::oid_of(&bytes) == oid {
                let rel = dent.path().strip_prefix(root).unwrap_or(dent.path());
                return Some(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    None
}

/// `rr cite <anchor>` — freeze the anchor's current content as a snapshot.
pub fn run_cite(args: &LowArgs) -> Result<u8, String> {
    run_pin(args, Kind::Snapshot)
}

/// `rr track <anchor>` — record the anchor's current content as a tracking baseline.
pub fn run_track(args: &LowArgs) -> Result<u8, String> {
    run_pin(args, Kind::Track)
}

/// The shared producer for `cite` and `track`: resolve the anchor to a unique
/// location, require its file to be committed *by content* and free of a clean
/// filter / Git-LFS, store the working-tree bytes in `.rr/`, and append a pin.
fn run_pin(args: &LowArgs, kind: Kind) -> Result<u8, String> {
    let root = Path::new(".");
    let index_path = PathBuf::from(cli::index_path(args));
    let anchor = args.positional[0].to_string_lossy().into_owned();

    with_fresh_reader(&index_path, root, args.no_freshness, |reader| {
        let hits = reader.forward_lookup(&anchor);
        let loc = match hits.as_slice() {
            [one] => one.clone(),
            [] => {
                eprintln!("no such anchor: {anchor}");
                return Ok(exit::FINDINGS);
            }
            many => {
                eprintln!(
                    "ambiguous anchor: {anchor} resolves to {} definitions",
                    many.len()
                );
                for loc in many {
                    eprintln!("  {loc}");
                }
                return Ok(exit::FINDINGS);
            }
        };
        let (path, start, end) =
            parse_loc(&loc).ok_or_else(|| format!("unparseable location: {loc:?}"))?;

        // The manifest is TAB/newline-delimited; refuse a field that would
        // corrupt it before doing any work, so a refused cite never writes a
        // stray object. (The sidecar enforces this too, as defense in depth.)
        if [anchor.as_str(), path.as_str()]
            .iter()
            .any(|s| s.contains('\t') || s.contains('\n'))
        {
            eprintln!(
                "cannot {} {anchor}: the anchor or path contains a tab or newline, which the manifest cannot represent",
                kind_verb(kind)
            );
            return Ok(exit::USAGE);
        }

        // Gate 1: the file must be committed *as-is*. Compare by content
        // (`git hash-object` of the working tree vs `HEAD:<path>`), which sees an
        // edit even under `--skip-worktree`, where `git status`/`git diff` would
        // not. `hash_object` doubles as the content OID for the stored object.
        let head_blob = git::rev_parse_head_blob(root, &path);
        let wt_oid = git::hash_object(root, &path);
        let oid = match (head_blob.as_deref(), wt_oid.as_deref()) {
            (Some(h), Some(w)) if h == w => w.to_string(),
            _ => {
                eprintln!(
                    "cannot {} {anchor}: {path} is not committed as-is — commit it first",
                    kind_verb(kind)
                );
                return Ok(exit::USAGE);
            }
        };

        // Gate 2: refuse a clean-filter / Git-LFS path. LFS content lives
        // off-repo, and a clean filter's pre-clean bytes can re-leak what the
        // filter strips, so freezing them verbatim is unsafe.
        if git::is_filtered_path(root, &path) {
            eprintln!(
                "cannot {} {anchor}: {path} uses a clean filter or Git-LFS, so its stored bytes would be unfaithful",
                kind_verb(kind)
            );
            return Ok(exit::USAGE);
        }

        let short = git::short_head(root).ok_or("cannot resolve HEAD (not a git repository?)")?;
        let bytes =
            std::fs::read(root.join(&path)).map_err(|e| format!("failed to read {path}: {e}"))?;

        let sc = Sidecar::at(root);
        sc.write_object(&oid, &bytes)?;
        sc.append_pin(&Pin {
            kind,
            anchor: anchor.clone(),
            short: short.clone(),
            path,
            start,
            end,
            oid,
        })?;

        if !args.quiet {
            println!("{anchor}{}{short}", sigil_char_for(kind));
        }
        Ok(exit::OK)
    })
}

/// `rr verify [<ref>...]` — classify pinned references and return the worst exit
/// code seen (0 ok, 4 drifted, 5 broken). Fails closed on a tampered manifest.
pub fn run_verify(args: &LowArgs) -> Result<u8, String> {
    let root = Path::new(".");
    let sc = Sidecar::at(root);
    let manifest = sc.load()?;

    // Fail closed first: a committed pin silently removed from the working
    // manifest (no tomb) is tampering, regardless of the individual refs.
    if let Some(code) = tamper_check(root, &manifest)? {
        return Ok(code);
    }

    let targets: Vec<&Pin> = if args.positional.is_empty() {
        manifest
            .live_pins(Kind::Snapshot)
            .into_iter()
            .chain(manifest.live_pins(Kind::Track))
            .collect()
    } else {
        let mut out = Vec::new();
        for token in &args.positional {
            let token = token.to_string_lossy();
            match cli::parse_reference(&token) {
                Reference::Pinned {
                    anchor,
                    sigil,
                    commit,
                } => {
                    let kind = match sigil {
                        Sigil::Snapshot => Kind::Snapshot,
                        Sigil::Tracking => Kind::Track,
                    };
                    match manifest.resolve(kind, anchor, commit) {
                        Ok(pin) => out.push(pin),
                        Err(_) => {
                            eprintln!("{token}: BROKEN (no such pinned reference)");
                            return Ok(exit::BROKEN);
                        }
                    }
                }
                Reference::Plain(_) => {
                    eprintln!(
                        "{token}: not a pinned reference (expected anchor@commit or anchor~commit)"
                    );
                    return Ok(exit::USAGE);
                }
            }
        }
        out
    };

    let mut worst = exit::OK;
    for pin in targets {
        let code = match pin.kind {
            Kind::Snapshot => verify_snapshot(&sc, pin),
            Kind::Track => {
                let code = match track_status(root, pin) {
                    TrackStatus::Clean => {
                        println!("{}~{}: ok", pin.anchor, pin.short);
                        exit::OK
                    }
                    TrackStatus::Moved(to) => {
                        println!("{}~{}: moved -> {to}", pin.anchor, pin.short);
                        exit::OK
                    }
                    TrackStatus::Drifted(detail) => {
                        println!("{}~{}: drifted ({detail})", pin.anchor, pin.short);
                        exit::DRIFTED
                    }
                    TrackStatus::Broken(reason) => {
                        println!("{}~{}: broken ({reason})", pin.anchor, pin.short);
                        exit::BROKEN
                    }
                };
                code
            }
        };
        worst = worst.max(code);
    }
    Ok(worst)
}

/// Verify a snapshot pin by re-hashing its stored object against the recorded
/// name (snapshots are durable, so the only failures are a missing or corrupt
/// object). Prints an `ok`/`broken` status line and returns its exit code.
fn verify_snapshot(sc: &Sidecar, pin: &Pin) -> u8 {
    match sc.read_object(&pin.oid) {
        Some(bytes) if sidecar::oid_of(&bytes) == pin.oid => {
            println!("{}@{}: ok", pin.anchor, pin.short);
            exit::OK
        }
        Some(_) => {
            println!(
                "{}@{}: broken (object {} is corrupt)",
                pin.anchor, pin.short, pin.oid
            );
            exit::BROKEN
        }
        None => {
            println!(
                "{}@{}: broken (object {} is missing)",
                pin.anchor, pin.short, pin.oid
            );
            exit::BROKEN
        }
    }
}

/// Compare the working `.rr/refs` against its committed form. A snapshot/track
/// pin present at `HEAD` but absent from the working manifest, with no tomb, is a
/// silent deletion: `verify` fails closed (returns `Some(BROKEN)`). When the
/// manifest is not committed there is no baseline to check, so this is a no-op.
fn tamper_check(root: &Path, working: &Manifest) -> Result<Option<u8>, String> {
    let Some(committed_text) = git::show_head_file(root, ".rr/refs") else {
        return Ok(None);
    };
    let committed = Manifest::parse(&committed_text)?;
    for record in &committed.records {
        let Record::Pin(p) = record else { continue };
        let present = working.records.iter().any(|wr| {
            matches!(wr, Record::Pin(wp)
                if wp.kind == p.kind && wp.anchor == p.anchor && wp.short == p.short && wp.oid == p.oid)
        });
        if !present && !working.is_tombed(&p.anchor, &p.short) {
            eprintln!(
                "manifest tampered: committed {} {}@{} was removed from .rr/refs without a tomb",
                p.kind.tag(),
                p.anchor,
                p.short,
            );
            return Ok(Some(exit::BROKEN));
        }
    }
    Ok(None)
}

/// `rr uncite <anchor@commit>` — retire a snapshot with a tomb.
pub fn run_uncite(args: &LowArgs) -> Result<u8, String> {
    run_tomb(args, Kind::Snapshot)
}

/// `rr untrack <anchor~commit>` — retire a tracking baseline with a tomb.
pub fn run_untrack(args: &LowArgs) -> Result<u8, String> {
    run_tomb(args, Kind::Track)
}

fn run_tomb(args: &LowArgs, kind: Kind) -> Result<u8, String> {
    let root = Path::new(".");
    let token = args.positional[0].to_string_lossy().into_owned();
    let Reference::Pinned {
        anchor,
        sigil,
        commit,
    } = cli::parse_reference(&token)
    else {
        eprintln!("expected {}", pin_syntax(kind));
        return Ok(exit::USAGE);
    };
    let want = match kind {
        Kind::Snapshot => Sigil::Snapshot,
        Kind::Track => Sigil::Tracking,
    };
    if sigil != want {
        eprintln!("expected {}", pin_syntax(kind));
        return Ok(exit::USAGE);
    }

    let sc = Sidecar::at(root);
    let manifest = sc.load()?;
    match manifest.resolve(kind, anchor, commit) {
        Ok(pin) => {
            sc.append_tomb(&Tomb {
                anchor: pin.anchor.clone(),
                short: pin.short.clone(),
                reason: format!("un{}", kind_verb(kind)),
            })?;
            if !args.quiet {
                println!("retired {}{}{}", pin.anchor, sigil_char(want), pin.short);
            }
            Ok(exit::OK)
        }
        Err(_) => {
            eprintln!("no such pinned reference: {token}");
            Ok(exit::BROKEN)
        }
    }
}

/// Split a `file:start-end` location into `(file, start, end)`.
fn parse_loc(loc: &str) -> Option<(String, u64, u64)> {
    let (file, span) = loc.rsplit_once(':')?;
    let (start, end) = span.split_once('-')?;
    Some((file.to_string(), start.parse().ok()?, end.parse().ok()?))
}

fn kind_verb(kind: Kind) -> &'static str {
    match kind {
        Kind::Snapshot => "cite",
        Kind::Track => "track",
    }
}

fn repin_verb(kind: Kind) -> &'static str {
    match kind {
        Kind::Snapshot => "cite",
        Kind::Track => "track",
    }
}

fn kind_noun(kind: Kind) -> &'static str {
    match kind {
        Kind::Snapshot => "snapshot",
        Kind::Track => "tracking baseline",
    }
}

fn sigil_char(sigil: Sigil) -> char {
    match sigil {
        Sigil::Snapshot => '@',
        Sigil::Tracking => '~',
    }
}

fn sigil_char_for(kind: Kind) -> char {
    match kind {
        Kind::Snapshot => '@',
        Kind::Track => '~',
    }
}

fn pin_syntax(kind: Kind) -> &'static str {
    match kind {
        Kind::Snapshot => "<anchor>@<commit>",
        Kind::Track => "<anchor>~<commit>",
    }
}

/// `rr at <file>:<line>` — name the anchor to cite for the position. The inverse
/// of `read`: a `file:line` in, the anchor you would write out. Text prints the
/// single tightest (innermost) anchor by default, or the whole nest it sits in
/// with `--all`; JSON always carries the full list with spans.
pub fn run_at(args: &LowArgs) -> Result<u8, String> {
    let root = Path::new(".");
    let index_path = PathBuf::from(cli::index_path(args));
    // `validate` already accepted this; re-parsing here keeps the position in
    // one place rather than threading a parsed field through `LowArgs`.
    let (file, line) = cli::parse_position(&args.positional[0].to_string_lossy())?;

    with_fresh_reader(&index_path, root, args.no_freshness, |reader| {
        let hits = reader.covering(&file, line);
        match args.format {
            // JSON always emits the envelope; `found`/`anchors` carry the result,
            // the exit code signals it (see doc/JSON.md).
            OutputFormat::Json => println!("{}", at_json(&file, line, &hits)),
            OutputFormat::Text if hits.is_empty() => eprintln!("no anchor covers {file}:{line}"),
            OutputFormat::Text => println!("{}", at_text(&hits, args.all)),
        }
        if hits.is_empty() {
            Ok(exit::FINDINGS)
        } else {
            Ok(exit::OK)
        }
    })
}

/// Text rendering for `rr at`: the anchor a human would cite, by name, with no
/// line numbers — those are the fragile coordinate ripref exists to replace and
/// live only in `--format json`. The default is the single tightest anchor
/// covering the position (the reference to write); `--all` prints the whole nest
/// it sits in, outermost-first, for when the tightest is not the one you mean.
/// Each line is a bare anchor, so it round-trips straight into `rr read`.
/// Returned rather than printed so it is unit-testable; `run_at` does the I/O.
fn at_text(hits: &[AnchorHit], all: bool) -> String {
    if all {
        hits.iter()
            .map(|h| h.anchor.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        // `covering` is outermost-first, so the tightest anchor is the last one.
        hits.last().map_or(String::new(), |h| h.anchor.clone())
    }
}

/// JSON rendering for `rr at`: the `rr-json` envelope from doc/JSON.md. Hand-rolled
/// because the crate has no serde dependency and the schema is a hand-written
/// source of truth; the first command to actually emit `--format json`. Returned
/// (not printed) so the exact document can be asserted in tests.
fn at_json(file: &str, line: u64, hits: &[AnchorHit]) -> String {
    let mut out = String::from(r#"{"format":"rr-json","version":1,"command":"at","data":{"file":"#);
    push_json_str(&mut out, file);
    out.push_str(",\"line\":");
    out.push_str(&line.to_string());
    out.push_str(",\"found\":");
    out.push_str(if hits.is_empty() { "false" } else { "true" });
    out.push_str(",\"anchors\":[");
    for (i, h) in hits.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str("{\"anchor\":");
        push_json_str(&mut out, &h.anchor);
        out.push_str(",\"location\":{\"file\":");
        push_json_str(&mut out, &h.file);
        out.push_str(",\"start_line\":");
        out.push_str(&h.start_line.to_string());
        out.push_str(",\"end_line\":");
        out.push_str(&h.end_line.to_string());
        out.push_str("}}");
    }
    out.push_str("]}}");
    out
}

/// Append `s` to `out` as a quoted, escaped JSON string. Escapes `"`, `\`, and
/// C0 control characters — anchors legitimately contain quotes (a scenario
/// anchor is `file.feature#"Title"`) and stray control bytes must not break the
/// document.
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
    fn at_json_lists_anchors_outermost_first() {
        let hits = vec![
            hit("src/handlers.py", "src/handlers.py", 1, 40),
            hit("handle_request", "src/handlers.py", 8, 30),
        ];
        // Pins the whole envelope: field order, the nested `location`, and the
        // outermost-first ordering of `anchors`.
        assert_eq!(
            at_json("src/handlers.py", 15, &hits),
            r#"{"format":"rr-json","version":1,"command":"at","data":{"file":"src/handlers.py","line":15,"found":true,"anchors":[{"anchor":"src/handlers.py","location":{"file":"src/handlers.py","start_line":1,"end_line":40}},{"anchor":"handle_request","location":{"file":"src/handlers.py","start_line":8,"end_line":30}}]}}"#
        );
    }

    #[test]
    fn at_json_not_found_emits_false_and_empty_anchors() {
        assert_eq!(
            at_json("a.rs", 9, &[]),
            r#"{"format":"rr-json","version":1,"command":"at","data":{"file":"a.rs","line":9,"found":false,"anchors":[]}}"#
        );
    }

    #[test]
    fn push_json_str_escapes_quotes_backslashes_and_controls() {
        let mut quoted = String::new();
        push_json_str(&mut quoted, r#"a"b\c"#);
        assert_eq!(quoted, r#""a\"b\\c""#);

        let mut whitespace = String::new();
        push_json_str(&mut whitespace, "tab\tnl\n");
        assert_eq!(whitespace, r#""tab\tnl\n""#);

        // A C0 control char becomes a \uXXXX escape; the raw byte must not survive.
        // Asserted by property to keep the literal escape out of this test's source.
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
        // A scenario-style anchor carries a quoted title (`file.feature#"Title"`).
        // No extractor emits one yet, so only a unit test exercises the escape
        // path that keeps such an anchor from breaking the JSON document.
        let hits = vec![hit(r#"x.feature#"Title""#, "x.feature", 3, 3)];
        let doc = at_json("x.feature", 3, &hits);
        assert!(doc.contains(r#""anchor":"x.feature#\"Title\"""#), "{doc}");
    }

    #[test]
    fn at_text_default_is_the_tightest_anchor_by_name() {
        // `covering` order: outermost (whole file) first, tightest last.
        let hits = vec![
            hit("src/handlers.py", "src/handlers.py", 1, 40),
            hit("handle_request", "src/handlers.py", 8, 30),
        ];
        // Default: just the anchor a human would cite, no line numbers.
        assert_eq!(at_text(&hits, false), "handle_request");
        // `--all`: the whole nest, outermost-first, names only.
        assert_eq!(at_text(&hits, true), "src/handlers.py\nhandle_request");
    }
}
