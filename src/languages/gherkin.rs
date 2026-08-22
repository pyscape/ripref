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
};

const KEYWORDS: &[&str] = &[
    "Feature:",
    "Rule:",
    "Scenario Outline:",
    "Scenario:",
    "Example:",
];

fn titles(content: &str) -> Vec<(String, u64)> {
    content
        .lines()
        .enumerate()
        .filter_map(|(row, line)| {
            let trimmed = line.trim_start();
            KEYWORDS
                .iter()
                .find_map(|kw| trimmed.strip_prefix(kw).map(|rest| (rest.trim(), row)))
        })
        .map(|(text, row)| (text.to_string(), row as u64))
        .collect()
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
}
