//! Python: `def` and `class` definitions as the symbol anchors of
//! `[[rr:AD-1#Decision outcome]]`, via the `tree-sitter-python` grammar.
//! Editing the query below changes which definitions anchor, nothing else.

use std::sync::OnceLock;

use crate::languages::{Language, Mode, Source};

pub(crate) static LANGUAGE: Language = Language {
    extensions: &["py", "pyi", "pyw"],
    source: Source::Grammar {
        grammar: tree_sitter_python::LANGUAGE,
        anchors_query: ANCHORS,
    },
    mode: Mode::Symbols,
    level: |_| u32::MAX,
    records: false,
    compiled: OnceLock::new(),
};

// `function_definition` also matches methods and `async def`, so neither
// needs its own pattern. A decorated definition is deliberately not matched:
// `decorated_definition` wraps the same name, so a second pattern would index
// every decorated function twice and make it ambiguous.
const ANCHORS: &str = r"
(function_definition name: (identifier) @anchor) @span
(class_definition name: (identifier) @anchor) @span
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn functions_classes_methods_and_async_are_anchors() {
        let src = concat!(
            "def top():\n",
            "    pass\n",
            "\n",
            "class Engine:\n",
            "    def move(self):\n",
            "        pass\n",
            "\n",
            "async def fetch():\n",
            "    pass\n",
        );
        let got: Vec<(String, String)> = LANGUAGE
            .extract_from_str("x.py", src)
            .into_iter()
            .map(|e| (e.anchor, e.location))
            .collect();
        assert_eq!(got.len(), 4, "{got:?}");
        assert!(got.contains(&("top".to_string(), "x.py:1-2".to_string())));
        assert!(got.contains(&("Engine".to_string(), "x.py:4-6".to_string())));
        assert!(got.contains(&("move".to_string(), "x.py:5-6".to_string())));
        assert!(got.contains(&("fetch".to_string(), "x.py:8-9".to_string())));
    }
}
