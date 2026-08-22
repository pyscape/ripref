/*!
The layered profile: compiled-in defaults from rr.toml, merged under a
project's `.rr.toml` (`[[rr:AD-1]]` puts kinds and scope in configuration).

This is a deliberate subset of TOML, hand-rolled per the crate's no-new-crates
ethos: section headers, quoted keys and strings, and string arrays (possibly
multiline). It reads only the keys the binary consumes; unknown keys pass
through unread, so the shipped rr.toml can document more than the code yet
honors.
*/

use std::path::Path;

const DEFAULTS: &str = include_str!("../rr.toml");

/// The keys the binary reads; `[[rr:Configuration]]` is what they mean
/// to a user, and `rr.toml` is the shipped default for each.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// `[verify] in-scope`
    pub verify_in_scope: Vec<String>,
    /// `[verify] exclude`
    pub verify_exclude: Vec<String>,
    /// `[verify] rules`
    pub verify_rules: Vec<String>,
    /// `[[rr:Configuration]]`
    pub scan: Vec<(String, Vec<String>)>,
}

/// `[[rr:AD-1#Decision outcome]]`
pub fn load(root: &Path) -> Result<Config, String> {
    let mut cfg = Config {
        verify_in_scope: Vec::new(),
        verify_exclude: Vec::new(),
        verify_rules: Vec::new(),
        scan: Vec::new(),
    };
    apply(DEFAULTS, &mut cfg).map_err(|e| format!("built-in rr.toml: {e}"))?;
    if let Ok(text) = std::fs::read_to_string(root.join(".rr.toml")) {
        apply(&text, &mut cfg).map_err(|e| format!(".rr.toml: {e}"))?;
    }
    Ok(cfg)
}

fn apply(text: &str, cfg: &mut Config) -> Result<(), String> {
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
        let key = unquote(key.trim());
        // Stripped per line, before joining: once lines are joined there's
        // no line boundary left to stop a later line's content from being
        // read as part of an earlier line's "# ...".
        let mut value = strip_comment(value).trim().to_string();
        loop {
            let (depth, quote) = tally(&value);
            // A truncated value reads as a deliberately short list, so it
            // is rejected like an unknown name. [[rr:Configuration]]
            if quote != Quote::Outside {
                return Err(format!("unterminated string in value for {key:?}"));
            }
            if depth <= 0 {
                break;
            }
            let Some(next) = lines.next() else {
                return Err(format!("unterminated array in value for {key:?}"));
            };
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
            let lang = unquote(lang);
            if key == "eligible" {
                let eligible = strings_in(&value);
                match cfg.scan.iter_mut().find(|(l, _)| l == lang) {
                    Some(entry) => entry.1 = eligible,
                    None => cfg.scan.push((lang.to_string(), eligible)),
                }
            }
        }
    }
    Ok(())
}

fn unquote(s: &str) -> &str {
    for q in ['"', '\''] {
        if let Some(inner) = s.strip_prefix(q).and_then(|rest| rest.strip_suffix(q)) {
            return inner;
        }
    }
    s
}

/// Which string a scan is inside, if any. TOML quotes with `"` or `'`, and
/// `#`, `[`, `]` inside either are literal, so every scan here tracks both:
/// reading only `"` makes `rules = ['path-line']` an empty list, silently
/// disabling the gate.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Quote {
    Outside,
    Double,
    Single,
}

impl Quote {
    fn step(self, c: char) -> Quote {
        match (self, c) {
            (Quote::Outside, '"') => Quote::Double,
            (Quote::Outside, '\'') => Quote::Single,
            (Quote::Double, '"') | (Quote::Single, '\'') => Quote::Outside,
            _ => self,
        }
    }
}

fn strip_comment(line: &str) -> &str {
    let mut quote = Quote::Outside;
    for (i, c) in line.char_indices() {
        if c == '#' && quote == Quote::Outside {
            return &line[..i];
        }
        quote = quote.step(c);
    }
    line
}

/// Bracket depth and quote state at the end of `value`.
///
/// `value` is always already comment-free (`apply` strips each line
/// before it's kept), so unlike `strip_comment` this never needs to watch
/// for `#`.
fn tally(value: &str) -> (i32, Quote) {
    let mut depth = 0;
    let mut quote = Quote::Outside;
    for c in value.chars() {
        match c {
            '[' if quote == Quote::Outside => depth += 1,
            ']' if quote == Quote::Outside => depth -= 1,
            _ => {}
        }
        quote = quote.step(c);
    }
    (depth, quote)
}

fn strings_in(value: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = value;
    while let Some(open) = rest.find(['"', '\'']) {
        let delim = rest[open..].chars().next().unwrap_or('"');
        let after = &rest[open + delim.len_utf8()..];
        let Some(close) = after.find(delim) else {
            break;
        };
        out.push(after[..close].to_string());
        rest = &after[close + delim.len_utf8()..];
    }
    out
}

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
        apply(DEFAULTS, &mut cfg).unwrap();
        assert_eq!(cfg.verify_in_scope, vec!["**/*.md"]);
        assert!(cfg.verify_exclude.is_empty());
    }

    #[test]
    fn literal_strings_parse_like_basic_ones() {
        assert_eq!(strings_in(r#"['a', 'b']"#), ["a", "b"]);
        assert_eq!(strings_in(r#"["a", 'b']"#), ["a", "b"]);
        assert_eq!(strings_in(r#"["it's"]"#), ["it's"]);
        assert_eq!(strings_in(r#"['say "hi"']"#), [r#"say "hi""#]);
        assert_eq!(strip_comment("k = ['a#b'] # c"), "k = ['a#b'] ");
        assert_eq!(tally("k = ['a['").0, 1);

        let mut cfg = Config {
            verify_in_scope: Vec::new(),
            verify_exclude: Vec::new(),
            verify_rules: Vec::new(),
            scan: Vec::new(),
        };
        apply(
            "[verify]\nin-scope = ['**/*.md']\nrules = ['path-line']\n",
            &mut cfg,
        )
        .unwrap();
        assert_eq!(cfg.verify_in_scope, ["**/*.md"]);
        assert_eq!(cfg.verify_rules, ["path-line"]);
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
        apply(DEFAULTS, &mut cfg).unwrap();
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
        apply(DEFAULTS, &mut cfg).unwrap();
        apply("[verify]\nrules = []\n", &mut cfg).unwrap();
        assert!(cfg.verify_rules.is_empty());

        let mut cfg = blank();
        apply(DEFAULTS, &mut cfg).unwrap();
        apply(
            "[verify]\nrules = [\"dangling-marker\", \"stale-mention\"]\n",
            &mut cfg,
        )
        .unwrap();
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
        apply("[verify]\nexclude = [\"tests/data/**\"]\n", &mut cfg).unwrap();
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
        apply(text, &mut cfg).unwrap();
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
        apply("[scan.python]\neligible = [\"comments\"]\n", &mut cfg).unwrap();
        assert_eq!(
            cfg.scan,
            vec![("python".to_string(), vec!["comments".to_string()])]
        );
        apply("[scan.python]\neligible = [\"prose\"]\n", &mut cfg).unwrap();
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
        apply("[scan.\"python\"]\neligible = [\"comments\"]\n", &mut cfg).unwrap();
        assert_eq!(
            cfg.scan,
            vec![("python".to_string(), vec!["comments".to_string()])]
        );
    }

    #[test]
    fn a_quoted_key_names_the_same_key() {
        assert_eq!(unquote("\"rules\""), "rules");
        assert_eq!(unquote("'rules'"), "rules");
        assert_eq!(unquote("rules"), "rules");
        assert_eq!(unquote("\"say \"hi\"\""), "say \"hi\"", "one pair only");

        let mut cfg = Config {
            verify_in_scope: Vec::new(),
            verify_exclude: Vec::new(),
            verify_rules: Vec::new(),
            scan: Vec::new(),
        };
        apply(
            "[verify]\n\"in-scope\" = [\"a/**\"]\n'rules' = [\"path-line\"]\n\n\
             [scan.python]\n\"eligible\" = [\"comments\"]\n",
            &mut cfg,
        )
        .unwrap();
        assert_eq!(cfg.verify_in_scope, ["a/**"]);
        assert_eq!(cfg.verify_rules, ["path-line"]);
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
        )
        .unwrap();
        assert_eq!(
            cfg.scan,
            vec![("rust".to_string(), vec!["comments".to_string()])]
        );
    }

    #[test]
    fn an_unterminated_value_is_an_error_not_a_short_list() {
        let blank = || Config {
            verify_in_scope: Vec::new(),
            verify_exclude: Vec::new(),
            verify_rules: Vec::new(),
            scan: Vec::new(),
        };

        let err = apply("[verify]\nrules = [\"path-line", &mut blank()).unwrap_err();
        assert!(err.contains("unterminated string"), "{err}");

        let err = apply(
            "[verify]\nrules = [\"path-line\", 'dangling-marker\"]\n",
            &mut blank(),
        )
        .unwrap_err();
        assert!(err.contains("unterminated string"), "{err}");

        let err = apply("[verify]\nrules = [\n  \"path-line\",\n", &mut blank()).unwrap_err();
        assert!(err.contains("unterminated array"), "{err}");
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
