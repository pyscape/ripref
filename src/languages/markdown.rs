//! Markdown: ATX headings (`# ...` through `###### ...`) as titled regions,
//! via the `tree-sitter-md` block grammar. The engine turns each title into a
//! record or heading anchor spanning its whole section (`[[rr:AD-1]]`);
//! editing the query below changes what counts as a title, nothing else.

use crate::languages::{Language, Mode};

pub const LANGUAGE: Language = Language {
    name: "markdown",
    extensions: &["md", "markdown"],
    grammar: tree_sitter_md::LANGUAGE,
    // In tree-sitter-md, `heading_content` is a field of `atx_heading` whose
    // node is `inline` (the heading text). Capture that node.
    anchors_query: "(atx_heading heading_content: (inline) @anchor)",
    mode: Mode::Sections,
    level: heading_level,
    titles: None,
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
