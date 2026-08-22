//! [[rr:AD-1]]. `grammar` and `anchors_query` are required by `Language`
//! but unused here; they point at the Markdown grammar and an empty
//! query.

use crate::languages::{Language, Mode};

pub const LANGUAGE: Language = Language {
    name: "gherkin",
    extensions: &["feature"],
    grammar: tree_sitter_md::LANGUAGE,
    anchors_query: "",
    mode: Mode::Sections,
    level,
    titles: Some(titles),
    records: false,
};

const KEYWORDS: &[&str] = &[
    "Feature:",
    "Rule:",
    "Scenario Outline:",
    "Scenario:",
    "Example:",
];

/// Gherkin 6.0 added ``` beside `"""`; either delimits a docstring.
const DOCSTRINGS: &[&str] = &["\"\"\"", "```"];

fn titles(content: &str) -> Vec<(String, u64)> {
    let mut out = Vec::new();
    let mut docstring: Option<&str> = None;
    for (row, line) in content.lines().enumerate() {
        let trimmed = line.trim_start();
        // Only the delimiter that opened it closes it, so a ``` inside a
        // `"""` docstring is content. An opener may carry a media type
        // (`"""json`); a closer never does.
        if let Some(open) = docstring {
            if trimmed.starts_with(open) {
                docstring = None;
            }
            continue;
        }
        if let Some(open) = DOCSTRINGS.iter().find(|d| trimmed.starts_with(**d)) {
            docstring = Some(open);
            continue;
        }
        if let Some(text) = KEYWORDS.iter().find_map(|kw| trimmed.strip_prefix(kw)) {
            out.push((text.trim().to_string(), row as u64));
        }
    }
    out
}

fn level(line: &str) -> u32 {
    let trimmed = line.trim_start();
    if trimmed.starts_with("Feature:") {
        1
    } else if trimmed.starts_with("Rule:") {
        2
    } else {
        3
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feature_with_two_scenarios_yields_three_anchors() {
        let src = concat!(
            "Feature: Login\n",
            "\n",
            "  Scenario: Valid credentials\n",
            "    Given a user\n",
            "\n",
            "  Scenario: Invalid credentials\n",
            "    Given a user\n",
        );
        let got: Vec<(String, String)> = LANGUAGE
            .extract_from_str("x.feature", src)
            .into_iter()
            .map(|e| (e.anchor, e.location))
            .collect();
        assert_eq!(got.len(), 3, "{got:?}");
        assert!(got.contains(&("Login".to_string(), "x.feature:1-7".to_string())));
        assert!(got.contains(&("Valid credentials".to_string(), "x.feature:3-5".to_string())));
        assert!(got.contains(&(
            "Invalid credentials".to_string(),
            "x.feature:6-7".to_string()
        )));
    }

    #[test]
    fn a_keyword_inside_a_docstring_is_not_a_title() {
        for (open, close) in [("\"\"\"json", "\"\"\""), ("```json", "```")] {
            let src = format!(
                "Feature: Real\n  Scenario: One\n    Given a doc\n      {open}\n      \
                 Scenario: not a title\n      {close}\n    And more\n  Scenario: Two\n    \
                 Given x\n"
            );
            let got: Vec<(String, String)> = LANGUAGE
                .extract_from_str("x.feature", &src)
                .into_iter()
                .map(|e| (e.anchor, e.location))
                .collect();
            assert_eq!(got.len(), 3, "{open}: {got:?}");
            assert!(
                got.contains(&("One".to_string(), "x.feature:2-7".to_string())),
                "the docstring stays inside the scenario: {open}: {got:?}"
            );
            assert!(
                got.contains(&("Two".to_string(), "x.feature:8-9".to_string())),
                "{open}: {got:?}"
            );
        }
    }

    // Only the delimiter that opened it closes it, so the other one is
    // content and the docstring runs on to its real closer.
    #[test]
    fn the_other_delimiter_inside_a_docstring_is_content() {
        let src = concat!(
            "Feature: Real\n",
            "  Scenario: One\n",
            "    Given a doc\n",
            "      \"\"\"\n",
            "      ```\n",
            "      Scenario: not a title\n",
            "      ```\n",
            "      \"\"\"\n",
            "    And more\n",
        );
        let got: Vec<(String, String)> = LANGUAGE
            .extract_from_str("x.feature", src)
            .into_iter()
            .map(|e| (e.anchor, e.location))
            .collect();
        assert_eq!(got.len(), 2, "{got:?}");
        assert!(
            got.contains(&("One".to_string(), "x.feature:2-9".to_string())),
            "{got:?}"
        );
    }
}
