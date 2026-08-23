//! Markdown: ATX headings (`# ...` through `###### ...`) as the titled
//! regions of `[[rr:AD-1#Decision outcome]]`, via the
//! `tree-sitter-md` block grammar. Editing the query below changes what
//! counts as a title, nothing else.

use std::sync::OnceLock;

use crate::languages::{Language, Mode};

pub(crate) static LANGUAGE: Language = Language {
    extensions: &["md", "markdown"],
    grammar: tree_sitter_md::LANGUAGE,
    // In tree-sitter-md, `heading_content` is a field of `atx_heading` whose
    // node is `inline` (the heading text). Capture that node.
    anchors_query: "(atx_heading heading_content: (inline) @anchor)",
    mode: Mode::Sections,
    level: heading_level,
    titles: None,
    records: true,
    compiled: OnceLock::new(),
};

fn heading_level(line: &str) -> u32 {
    let trimmed = line.trim_start();
    let hashes = trimmed.bytes().take_while(|&b| b == b'#').count();
    if (1..=6).contains(&hashes) {
        hashes as u32
    } else {
        u32::MAX
    }
}
