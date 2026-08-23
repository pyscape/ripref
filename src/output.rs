/*!
Rendering, and the one write to stdout: the `rr-json` envelope of
`[[rr:AD-4]]` and its escaping, the bodies `at` and `search` build, and
[`emit`], which every verb prints through.
*/

use std::fmt::Write as _;
use std::io::{BufWriter, Write};

use crate::cli::OutputFormat;
use crate::marker;
use crate::refidx::AnchorHit;

pub(crate) fn emit(
    code: u8,
    write: impl FnOnce(&mut dyn Write) -> std::io::Result<()>,
) -> Result<u8, String> {
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
pub(crate) fn envelope(command: &str, data: &str) -> String {
    format!(
        r#"{{"format":"rr-json","version":1,"command":"{command}","data":{data}}}"#
    )
}

/// Append a structured location object.
pub(crate) fn push_location(
    out: &mut String,
    file: &str,
    start: u64,
    end: u64,
) {
    out.push_str("{\"file\":");
    push_json_str(out, file);
    out.push_str(&format!(",\"start_line\":{start},\"end_line\":{end}}}"));
}

/// `[[rr:AD-2#Decision drivers]]`
pub(crate) fn push_json_str(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32))
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Text rendering for `rr at`: one marker per line, the document form a
/// person pastes `[[rr:AD-4]]`. Returned rather than printed so it is
/// unit-testable; `run_at` does the I/O.
pub(crate) fn at_text(forms: &[(String, &AnchorHit)]) -> String {
    forms
        .iter()
        .map(|(form, _)| marker::wrap(form))
        .collect::<Vec<_>>()
        .join("\n")
}

/// JSON `data` for `rr at` (`[[rr:AD-4#Decision outcome]]`), returned like
/// [`at_text`] above it.
pub(crate) fn at_json(forms: &[(String, &AnchorHit)]) -> String {
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

/// `[[rr:AD-4#Decision outcome]]`
pub(crate) struct SearchSink {
    buf: String,
    json: bool,
    count: usize,
}

impl SearchSink {
    pub(crate) fn new(format: OutputFormat) -> Self {
        let json = format == OutputFormat::Json;
        let mut buf = String::new();
        if json {
            buf.push_str(r#"{"matches":["#);
        }
        Self {
            buf,
            json,
            count: 0,
        }
    }

    pub(crate) fn marker(
        &mut self,
        rel: &str,
        line: u64,
        anchor: &str,
        raw: &str,
    ) {
        if self.json {
            self.open(rel, line);
            self.buf.push_str(",\"anchor\":");
            push_json_str(&mut self.buf, anchor);
            self.buf.push_str(",\"marker\":");
            push_json_str(&mut self.buf, raw);
            self.buf.push('}');
        } else {
            self.text(rel, line, raw);
        }
        self.count += 1;
    }

    pub(crate) fn mention(&mut self, rel: &str, line: u64, token: &str) {
        if self.json {
            self.open(rel, line);
            self.buf.push_str(",\"mention\":");
            push_json_str(&mut self.buf, token);
            self.buf.push('}');
        } else {
            self.text(rel, line, token);
        }
        self.count += 1;
    }

    fn open(&mut self, rel: &str, line: u64) {
        if self.count > 0 {
            self.buf.push(',');
        }
        self.buf.push_str("{\"file\":");
        push_json_str(&mut self.buf, rel);
        write!(self.buf, ",\"line\":{line}")
            .expect("a String never fails to write");
    }

    fn text(&mut self, rel: &str, line: u64, what: &str) {
        writeln!(self.buf, "{rel}:{line}: {what}")
            .expect("a String never fails to write");
    }

    pub(crate) fn count(&self) -> usize {
        self.count
    }

    pub(crate) fn finish(mut self) -> String {
        if self.json {
            self.buf.push_str("]}");
        }
        self.buf
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::exit;

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

    fn hit(
        anchor: &str,
        file: &str,
        start_line: u64,
        end_line: u64,
    ) -> AnchorHit {
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
