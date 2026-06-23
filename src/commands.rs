/*!
Command implementations: the single writer (`index`) and one reader (`read`).

Each returns `Ok(exit_code)` for a normal outcome (including findings/stale,
which are non-zero but not errors) or `Err(message)` for a usage-level failure
the caller reports as exit `2`.
*/

use std::path::{Path, PathBuf};

use memmap2::Mmap;

use crate::cli::{self, LowArgs, OutputFormat};
use crate::exit;
use crate::indexer;
use crate::refidx::{self, AnchorHit, Reader};

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
    std::fs::write(&index_path, &bytes)
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
fn with_fresh_reader<F>(index_path: &Path, root: &Path, f: F) -> Result<u8, String>
where
    F: FnOnce(&Reader) -> Result<u8, String>,
{
    let file = match std::fs::File::open(index_path) {
        Ok(f) => f,
        Err(_) => {
            eprintln!("no index at {} — run `rr index`", index_path.display());
            return Ok(exit::STALE);
        }
    };
    // SAFETY: the index is a regular file we just opened; concurrent writers go
    // through `rr index`, which writes a fresh file rather than mutating bytes.
    #[allow(unsafe_code)] // the one justified unsafe in the crate (posture: src/lib.rs)
    let mmap = unsafe { Mmap::map(&file) }
        .map_err(|e| format!("failed to mmap {}: {e}", index_path.display()))?;

    let reader = Reader::parse(&mmap).map_err(|e| format!("corrupt index: {e}"))?;

    // Freshness gates the answer: a reader exits 3 rather than answer stale.
    if indexer::newest_mtime(&reader.paths(), root) > reader.mtime {
        eprintln!("index is stale — rebuild with `rr index`");
        return Ok(exit::STALE);
    }

    f(&reader)
}

/// `rr read <anchor>` — dereference one anchor through the forward map.
pub fn run_read(args: &LowArgs) -> Result<u8, String> {
    let root = Path::new(".");
    let index_path = PathBuf::from(cli::index_path(args));
    let anchor = args.positional[0].to_string_lossy().into_owned();

    with_fresh_reader(&index_path, root, |reader| {
        let hits = reader.forward_lookup(&anchor);
        match hits.len() {
            0 => {
                eprintln!("no such anchor: {anchor}");
                Ok(exit::FINDINGS)
            }
            1 => {
                // Only the location line is printed today; the source-body print
                // and `-C`/`--context` are not yet implemented.
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
    })
}

/// `rr at <file>:<line>` — list every anchor whose span covers the position,
/// outermost-first. The inverse of `read`: a `file:line` in, anchors out.
pub fn run_at(args: &LowArgs) -> Result<u8, String> {
    let root = Path::new(".");
    let index_path = PathBuf::from(cli::index_path(args));
    // `validate` already accepted this; re-parsing here keeps the position in
    // one place rather than threading a parsed field through `LowArgs`.
    let (file, line) = cli::parse_position(&args.positional[0].to_string_lossy())?;

    with_fresh_reader(&index_path, root, |reader| {
        let hits = reader.covering(&file, line);
        match args.format {
            // JSON always emits the envelope; `found`/`anchors` carry the result,
            // the exit code signals it (see doc/JSON.md).
            OutputFormat::Json => println!("{}", at_json(&file, line, &hits)),
            OutputFormat::Text if hits.is_empty() => eprintln!("no anchor covers {file}:{line}"),
            OutputFormat::Text => println!("{}", at_text(&hits)),
        }
        if hits.is_empty() {
            Ok(exit::FINDINGS)
        } else {
            Ok(exit::OK)
        }
    })
}

/// Text rendering for `rr at`: one anchor per line, `anchor\tfile:start-end`, so
/// a line round-trips straight into `rr read` and greps cleanly. Returned rather
/// than printed so it is unit-testable; `run_at` does the printing.
fn at_text(hits: &[AnchorHit]) -> String {
    hits.iter()
        .map(|h| format!("{}\t{}:{}-{}", h.anchor, h.file, h.start_line, h.end_line))
        .collect::<Vec<_>>()
        .join("\n")
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
    fn at_text_is_outermost_first_tab_separated() {
        let hits = vec![
            hit("src/handlers.py", "src/handlers.py", 1, 40),
            hit("handle_request", "src/handlers.py", 8, 30),
        ];
        assert_eq!(
            at_text(&hits),
            "src/handlers.py\tsrc/handlers.py:1-40\nhandle_request\tsrc/handlers.py:8-30"
        );
    }
}
