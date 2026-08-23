/*!
The gate of `[[rr:AD-3]]`: its rule table and the pass that applies it.
*/

use std::path::{Path, PathBuf};

use crate::cli::{self, LowArgs, OutputFormat};
use crate::commands::{resolve, scoped_files, with_fresh_reader};
use crate::config;
use crate::exit;
use crate::output::{emit, envelope, push_json_str};
use crate::scan::{self, Kind};

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

struct Finding {
    file: String,
    line: u64,
    rule: Rule,
    detail: String,
}

/// `[[rr:help_text]]`, reporting the six kinds of `[[rr:AD-3]]`. Resolution
/// judgments need the index, so a stale index exits 3 rather than judging
/// from stale data; mention judgments run against the live tree.
pub(crate) fn run_verify(args: &LowArgs) -> Result<u8, String> {
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
            for hit in scan::scan(&file.content, file.host) {
                let (rule, detail) = match &hit.kind {
                    Kind::Malformed { reason } => (MALFORMED, reason.clone()),
                    Kind::Marker { raw, anchor } => {
                        if !anchor.contains('#')
                            && scan::is_path_shaped(anchor)
                        {
                            (PATH_ONLY, raw.clone())
                        } else {
                            match resolve(reader, anchor).len() {
                                0 => (DANGLING, raw.clone()),
                                1 => continue,
                                n => (
                                    AMBIGUOUS,
                                    format!(
                                        "{raw} resolves to {n} definitions"
                                    ),
                                ),
                            }
                        }
                    }
                    Kind::Mention { token, line_ref } => {
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
                    line: hit.line,
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
                    writeln!(
                        w,
                        "{}:{}: {}: {}",
                        f.file, f.line, f.rule.text, f.detail
                    )?;
                }
                if args.quiet {
                    return Ok(());
                }
                writeln!(w, "{} findings", findings.len())
            }
        })
    })
}
