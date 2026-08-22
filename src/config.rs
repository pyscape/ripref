/*!
The layered profile: compiled-in defaults from rr.toml, merged under a
project's `.rr.toml` (`[[rr:AD-1]]` puts kinds and scope in configuration).

This is a deliberate subset of TOML, hand-rolled per the crate's no-new-crates
ethos: section headers, quoted strings, and string arrays (possibly
multiline). It reads only the keys the binary consumes; unknown keys pass
through unread, so the shipped rr.toml can document more than the code yet
honors.
*/

use std::path::Path;

/// The compiled-in defaults: the same rr.toml that documents them.
const DEFAULTS: &str = include_str!("../rr.toml");

/// The keys the binary reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// `[verify] in-scope`: the globs whose files the gate and the scanners
    /// read.
    pub verify_in_scope: Vec<String>,
    /// `[verify] exclude`: globs subtracted from the scope.
    pub verify_exclude: Vec<String>,
    /// `[verify] rules`: which of the six finding kinds this profile
    /// reports. `[[rr:AD-3]]` fixes the six; a profile picks among them, and
    /// an empty list disables the gate.
    pub verify_rules: Vec<String>,
    /// `[scan.<lang>] eligible`, one entry per language named so far, in the
    /// order first declared. A later layer's `eligible` for the same
    /// language replaces the entry wholesale rather than appending.
    pub scan: Vec<(String, Vec<String>)>,
}

/// Load the profile: defaults, then the project's `.rr.toml` merged over
/// them, key by key.
pub fn load(root: &Path) -> Config {
    let mut cfg = Config {
        verify_in_scope: Vec::new(),
        verify_exclude: Vec::new(),
        verify_rules: Vec::new(),
        scan: Vec::new(),
    };
    apply(DEFAULTS, &mut cfg);
    if let Ok(text) = std::fs::read_to_string(root.join(".rr.toml")) {
        apply(&text, &mut cfg);
    }
    cfg
}

/// Fold one TOML text into `cfg`. A key present in `text` replaces the value
/// wholesale; a key absent leaves the lower layer's value standing.
fn apply(text: &str, cfg: &mut Config) {
    let mut section = String::new();
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line.strip_prefix('[') {
            if let Some(name) = rest.split(']').next() {
                section = name.trim().to_string();
            }
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        // Stripped per line, before joining: once lines are joined there's
        // no line boundary left to stop a later line's content from being
        // read as part of an earlier line's "# ...".
        let mut value = strip_comment(value).trim().to_string();
        while open_brackets(&value) > 0 {
            let Some(next) = lines.next() else { break };
            value.push(' ');
            value.push_str(strip_comment(next).trim());
        }
        if section == "verify" {
            match key {
                "in-scope" => cfg.verify_in_scope = strings_in(&value),
                "exclude" => cfg.verify_exclude = strings_in(&value),
                "rules" => cfg.verify_rules = strings_in(&value),
                _ => {}
            }
        } else if let Some(lang) = section.strip_prefix("scan.") {
            // A quoted table key (`[scan."python"]`), valid TOML, still
            // names the language "python".
            let lang = lang.trim_matches(|c| c == '"' || c == '\'');
            if key == "eligible" {
                let eligible = strings_in(&value);
                match cfg.scan.iter_mut().find(|(l, _)| l == lang) {
                    Some(entry) => entry.1 = eligible,
                    None => cfg.scan.push((lang.to_string(), eligible)),
                }
            }
        }
    }
}

fn strip_comment(line: &str) -> &str {
    let mut in_str = false;
    for (i, c) in line.char_indices() {
        match c {
            '"' => in_str = !in_str,
            '#' if !in_str => return &line[..i],
            _ => {}
        }
    }
    line
}

/// `value` is always already comment-free (`apply` strips each line
/// before it's kept), so unlike `strip_comment` this never needs to watch
/// for `#`.
fn open_brackets(value: &str) -> i32 {
    let mut depth = 0;
    let mut in_str = false;
    for c in value.chars() {
        match c {
            '"' => in_str = !in_str,
            '[' if !in_str => depth += 1,
            ']' if !in_str => depth -= 1,
            _ => {}
        }
    }
    depth
}

/// Every double-quoted string in `value`, in order.
fn strings_in(value: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = value;
    while let Some(open) = rest.find('"') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('"') else { break };
        out.push(after[..close].to_string());
        rest = &after[close + 1..];
    }
    out
}

/// Build the scope matcher for the verify/search/index scanners from the
/// profile globs, rooted at `root`.
pub fn scope_matcher(root: &Path, cfg: &Config) -> Result<ignore::overrides::Override, String> {
    let mut b = ignore::overrides::OverrideBuilder::new(root);
    for glob in &cfg.verify_in_scope {
        b.add(glob)
            .map_err(|e| format!("bad in-scope glob {glob:?}: {e}"))?;
    }
    for glob in &cfg.verify_exclude {
        b.add(&format!("!{glob}"))
            .map_err(|e| format!("bad exclude glob {glob:?}: {e}"))?;
    }
    b.build().map_err(|e| format!("bad scope globs: {e}"))
}

/// Whether a repo-relative file path is in scanning scope.
pub fn in_scope(matcher: &ignore::overrides::Override, rel: &str) -> bool {
    matcher.matched(rel, false).is_whitelist()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_scope_markdown() {
        let mut cfg = Config {
            verify_in_scope: Vec::new(),
            verify_exclude: Vec::new(),
            verify_rules: Vec::new(),
            scan: Vec::new(),
        };
        apply(DEFAULTS, &mut cfg);
        assert_eq!(cfg.verify_in_scope, vec!["**/*.md"]);
        assert!(cfg.verify_exclude.is_empty());
    }

    #[test]
    fn verify_rules_default_to_the_six_and_are_replaced_wholesale() {
        let blank = || Config {
            verify_in_scope: Vec::new(),
            verify_exclude: Vec::new(),
            verify_rules: Vec::new(),
            scan: Vec::new(),
        };
        let mut cfg = blank();
        apply(DEFAULTS, &mut cfg);
        assert_eq!(
            cfg.verify_rules,
            [
                "malformed-marker",
                "dangling-marker",
                "ambiguous-marker",
                "path-only-marker",
                "path-line",
                "stale-mention",
            ],
            "the six kinds rr.toml lists"
        );

        // Empty is a value, not an absence: it disables the gate.
        let mut cfg = blank();
        apply(DEFAULTS, &mut cfg);
        apply("[verify]\nrules = []\n", &mut cfg);
        assert!(cfg.verify_rules.is_empty());

        let mut cfg = blank();
        apply(DEFAULTS, &mut cfg);
        apply(
            "[verify]\nrules = [\"dangling-marker\", \"stale-mention\"]\n",
            &mut cfg,
        );
        assert_eq!(cfg.verify_rules, ["dangling-marker", "stale-mention"]);
    }

    #[test]
    fn project_layer_replaces_per_key() {
        let mut cfg = Config {
            verify_in_scope: vec!["**/*.md".into()],
            verify_exclude: Vec::new(),
            verify_rules: Vec::new(),
            scan: Vec::new(),
        };
        apply("[verify]\nexclude = [\"tests/data/**\"]\n", &mut cfg);
        assert_eq!(cfg.verify_in_scope, vec!["**/*.md"], "untouched key stands");
        assert_eq!(cfg.verify_exclude, vec!["tests/data/**"]);
    }

    #[test]
    fn multiline_arrays_and_comments_parse() {
        let text = "[verify]\nin-scope = [\n  \"a/**\", # docs\n  \"b/**\",\n]\n";
        let mut cfg = Config {
            verify_in_scope: Vec::new(),
            verify_exclude: Vec::new(),
            verify_rules: Vec::new(),
            scan: Vec::new(),
        };
        apply(text, &mut cfg);
        assert_eq!(cfg.verify_in_scope, vec!["a/**", "b/**"]);
    }

    #[test]
    fn scan_table_parses_and_second_layer_replaces() {
        let mut cfg = Config {
            verify_in_scope: Vec::new(),
            verify_exclude: Vec::new(),
            verify_rules: Vec::new(),
            scan: Vec::new(),
        };
        apply("[scan.python]\neligible = [\"comments\"]\n", &mut cfg);
        assert_eq!(
            cfg.scan,
            vec![("python".to_string(), vec!["comments".to_string()])]
        );
        apply("[scan.python]\neligible = [\"prose\"]\n", &mut cfg);
        assert_eq!(
            cfg.scan,
            vec![("python".to_string(), vec!["prose".to_string()])]
        );
    }

    #[test]
    fn quoted_table_key_names_the_language() {
        let mut cfg = Config {
            verify_in_scope: Vec::new(),
            verify_exclude: Vec::new(),
            verify_rules: Vec::new(),
            scan: Vec::new(),
        };
        apply("[scan.\"python\"]\neligible = [\"comments\"]\n", &mut cfg);
        assert_eq!(
            cfg.scan,
            vec![("python".to_string(), vec!["comments".to_string()])]
        );
    }

    #[test]
    fn trailing_comment_after_the_array_is_not_parsed_as_more_strings() {
        let mut cfg = Config {
            verify_in_scope: Vec::new(),
            verify_exclude: Vec::new(),
            verify_rules: Vec::new(),
            scan: Vec::new(),
        };
        apply(
            "[scan.rust]\neligible = [\"comments\"] # not \"all\"\n",
            &mut cfg,
        );
        assert_eq!(
            cfg.scan,
            vec![("rust".to_string(), vec!["comments".to_string()])]
        );
    }

    #[test]
    fn matcher_whitelists_scope_minus_excludes() {
        let cfg = Config {
            verify_in_scope: vec!["**/*.md".into()],
            verify_exclude: vec!["tests/data/**".into()],
            verify_rules: Vec::new(),
            scan: Vec::new(),
        };
        let m = scope_matcher(Path::new("."), &cfg).unwrap();
        assert!(in_scope(&m, "README.md"));
        assert!(in_scope(&m, "doc/ad/0001-domain-model.md"));
        assert!(!in_scope(&m, "src/cli.rs"));
        assert!(!in_scope(&m, "tests/data/marker-violations.md"));
    }
}
