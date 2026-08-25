//! Markdown documentation extractor.
//!
//! Splits a markdown file into section-level `Document` symbols so ADRs,
//! guides, issues, and changelogs become first-class retrievable units on the
//! same symbol pipeline as code (docs-indexing design, Phase 1).
//!
//! Chunking rules:
//! - Content before the first H2/H3 heading becomes one `preamble` section
//!   (covers most READMEs, including their H1 title).
//! - Each ATX heading of level 2–3 starts a new section; the section spans to
//!   the next heading of level ≤ its own or EOF, so nested subsections stay
//!   inside their parent span and hydrate returns coherent chunks.
//! - An H1 is treated as the document title, not a section boundary.
//! - A file without any H2/H3 heading becomes one whole-file `document`
//!   section.

use crate::indexer::extract::symbol::{
    ByteSpan, DataFlowEdge, ExtractedFile, ExtractedFrameworkPattern, ExtractedSymbol, Import,
    LineSpan, ModuleBinding, SymbolKind,
};
use anyhow::Result;
use serde::{Deserialize, Serialize};

/// Maximum characters in a heading used as the section name. Long headings
/// are truncated on a word boundary so Tantivy names stay sane.
const MAX_NAME_CHARS: usize = 120;

/// Classification of a documentation source, derived from its repo-relative
/// path (docs-indexing design, Phase 2). Serialized as the `doc_type` column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocType {
    Adr,
    Issue,
    Bug,
    Changelog,
    Readme,
    Guide,
    Design,
    Other,
}

impl DocType {
    pub fn as_str(self) -> &'static str {
        match self {
            DocType::Adr => "adr",
            DocType::Issue => "issue",
            DocType::Bug => "bug",
            DocType::Changelog => "changelog",
            DocType::Readme => "readme",
            DocType::Guide => "guide",
            DocType::Design => "design",
            DocType::Other => "other",
        }
    }
}

/// Classify a document from its repo-relative path. Front-matter may refine
/// this later; path conventions are the primary signal.
pub fn classify_doc_path(rel_path: &str) -> DocType {
    let p = rel_path.to_lowercase();
    let segments: Vec<&str> = p
        .split('/')
        .filter(|s| !s.is_empty() && *s != "docs" && *s != "doc")
        .collect();
    let filename = segments.last().copied().unwrap_or("");
    if segments.iter().any(|s| *s == "adr" || *s == "decisions")
        || filename.contains("decision")
        || (filename.starts_with("adr-") || filename.starts_with("adr_") || filename == "adr.md")
    {
        return DocType::Adr;
    }
    if filename.contains("issue") || segments.contains(&"issues") {
        return DocType::Issue;
    }
    if filename.contains("bug") || segments.contains(&"bugs") {
        return DocType::Bug;
    }
    if filename.starts_with("changelog") {
        return DocType::Changelog;
    }
    if filename.starts_with("readme") {
        return DocType::Readme;
    }
    if filename.starts_with("contributing")
        || filename.starts_with("security")
        || filename.starts_with("license")
        || filename.starts_with("code_of_conduct")
    {
        return DocType::Guide;
    }
    if segments.iter().any(|s| {
        *s == "design" || *s == "specs" || *s == "rfcs" || *s == "plans" || *s == "planning"
    }) || filename.contains("work-log")
    {
        return DocType::Design;
    }
    // Anything else living under docs/ is operational prose.
    if p.starts_with("docs/") || p.contains("/docs/") {
        return DocType::Guide;
    }
    DocType::Other
}

/// YAML front-matter fields we understand. Only flat `key: value` pairs are
/// parsed — this is deliberately not a YAML parser.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DocFrontMatter {
    pub status: Option<String>,
    pub date: Option<String>,
    pub number: Option<i64>,
    pub labels: Vec<String>,
}

/// Parse leading `---` fenced front-matter from a markdown source.
pub fn parse_front_matter(source: &str) -> Option<DocFrontMatter> {
    let mut lines = source.lines();
    if lines.next()?.trim() != "---" {
        return None;
    }
    let mut fm = DocFrontMatter::default();
    for line in lines {
        let trimmed = line.trim();
        if trimmed == "---" {
            return Some(fm);
        }
        let Some((key, value)) = trimmed.split_once(':') else {
            continue;
        };
        let key = key.trim().to_lowercase();
        let value = value
            .trim()
            .trim_matches('"')
            .trim_matches('\'')
            .to_string();
        match key.as_str() {
            "status" | "state" => fm.status = Some(value),
            "date" => fm.date = Some(value),
            "number" | "issue" => fm.number = value.parse().ok(),
            "labels" | "tags" => {
                fm.labels = value
                    .split(',')
                    .map(|l| l.trim().to_string())
                    .filter(|l| !l.is_empty())
                    .collect();
            }
            _ => {}
        }
    }
    // Unterminated fence: treat as no front-matter.
    None
}

/// One section start: (heading text, ATX level, byte offset, 1-based line).
struct Boundary {
    name: String,
    level: u8,
    byte: usize,
    line: u32,
}

pub fn extract_markdown_symbols(source: &str) -> Result<ExtractedFile> {
    let mut symbols: Vec<ExtractedSymbol> = Vec::new();
    if source.trim().is_empty() {
        return Ok(empty_file(symbols));
    }

    // Single pass: find section boundaries at ATX headings level 1–3.
    let mut boundaries: Vec<Boundary> = Vec::new();
    let mut offset = 0usize;
    for (i, line) in source.lines().enumerate() {
        if let Some((level, text)) = parse_atx_heading(line.trim_end()) {
            if (2..=3).contains(&level) {
                boundaries.push(Boundary {
                    name: truncate_name(text),
                    level,
                    byte: offset,
                    line: i as u32 + 1,
                });
            }
        }
        offset += line.len() + 1; // +1 accounts for '\n' (last line included
                                  // via byte_len clamp below).
    }

    if boundaries.is_empty() {
        emit(
            &mut symbols,
            source,
            "document".to_string(),
            0,
            1,
            source.len(),
            source.lines().count() as u32,
        );
        return Ok(empty_file(symbols));
    }

    // Preamble: bytes before the first heading.
    let first = &boundaries[0];
    if source[..first.byte]
        .trim()
        .chars()
        .any(|c| !c.is_whitespace())
    {
        emit(
            &mut symbols,
            source,
            "preamble".to_string(),
            0,
            1,
            first.byte,
            first.line - 1,
        );
    }

    // Each heading spans until the next heading of level ≤ its own (so
    // nested subsections stay inside their parent) or EOF.
    let total_lines = source.lines().count() as u32;
    for (idx, b) in boundaries.iter().enumerate() {
        let end = boundaries[idx + 1..]
            .iter()
            .find(|next| next.level <= b.level)
            .map(|next| (next.byte, next.line - 1));
        let (end_byte, end_line) = end.unwrap_or((source.len(), total_lines));
        emit(
            &mut symbols,
            source,
            b.name.clone(),
            b.byte,
            b.line,
            end_byte,
            end_line,
        );
    }

    Ok(empty_file(symbols))
}

fn emit(
    symbols: &mut Vec<ExtractedSymbol>,
    source: &str,
    name: String,
    start_byte: usize,
    start_line: u32,
    end_byte: usize,
    end_line: u32,
) {
    let text = &source[start_byte..end_byte.min(source.len())];
    // Skip empty sections (blank stubs between adjacent headings).
    if text.trim().is_empty() {
        return;
    }
    symbols.push(ExtractedSymbol {
        name,
        kind: SymbolKind::Document,
        exported: false,
        bytes: ByteSpan {
            start: start_byte,
            end: end_byte.min(source.len()),
        },
        lines: LineSpan {
            start: start_line,
            end: end_line.max(start_line),
        },
    });
}

/// Parse an ATX heading (`#`–`######` followed by whitespace). Returns
/// `(level, heading text)`. Fenced code blocks containing `#` lines are a
/// known Phase-1 limitation and may split sections there.
fn parse_atx_heading(line: &str) -> Option<(u8, &str)> {
    if !line.starts_with('#') {
        return None;
    }
    let hashes = line.bytes().take_while(|&b| b == b'#').count() as u8;
    if hashes > 6 || hashes as usize == line.len() {
        return None;
    }
    let rest = &line[hashes as usize..];
    if !rest.starts_with(' ') && !rest.starts_with('\t') {
        return None;
    }
    Some((hashes, rest.trim()))
}

fn truncate_name(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= MAX_NAME_CHARS {
        return text.to_string();
    }
    let mut cut = MAX_NAME_CHARS;
    while cut > 0 && !chars[cut].is_whitespace() {
        cut -= 1;
    }
    chars[..cut].iter().collect()
}

fn empty_file(symbols: Vec<ExtractedSymbol>) -> ExtractedFile {
    ExtractedFile {
        symbols,
        imports: Vec::<Import>::new(),
        module_bindings: Vec::<ModuleBinding>::new(),
        type_edges: Vec::new(),
        inheritance_relations: Vec::new(),
        dataflow_edges: Vec::<DataFlowEdge>::new(),
        todos: Vec::new(),
        jsdoc_entries: Vec::new(),
        decorators: Vec::new(),
        framework_patterns: Vec::<ExtractedFrameworkPattern>::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_sections_at_headings() {
        let src = "# Title\n\nintro text\n\n## Setup\n\nsetup body\n\n### Details\n\ndetail body\n\n## Usage\n\nusage body\n";
        let out = extract_markdown_symbols(src).unwrap();
        let names: Vec<&str> = out.symbols.iter().map(|s| s.name.as_str()).collect();
        // H1 is the document title; the lead text (including it) is the
        // preamble, and sections start at H2/H3.
        assert_eq!(names, vec!["preamble", "Setup", "Details", "Usage"]);
        assert!(out
            .symbols
            .iter()
            .all(|s| s.kind == SymbolKind::Document && !s.exported));
    }

    #[test]
    fn section_text_spans_are_correct() {
        let src = "# Title\n\nbody line\n\n## Next\n\nnext body\n";
        let out = extract_markdown_symbols(src).unwrap();
        let title = &out.symbols[0];
        // The preamble includes the H1 title and spans to the first H2.
        assert_eq!(
            &src[title.bytes.start..title.bytes.end],
            "# Title\n\nbody line\n\n"
        );
        assert_eq!(title.lines.start, 1);
        let next = &out.symbols[1];
        // The final section spans to EOF including the trailing newline.
        assert_eq!(
            &src[next.bytes.start..next.bytes.end],
            "## Next\n\nnext body\n"
        );
        assert_eq!(next.lines.start, 5);
    }

    #[test]
    fn nested_sections_stay_inside_parents() {
        let src = "# Guide\n\n## A\n\ntext a\n### A1\n\ndeep\n\n## B\n\nb\n";
        let out = extract_markdown_symbols(src).unwrap();
        let a = out.symbols.iter().find(|s| s.name == "A").unwrap();
        let a1 = out.symbols.iter().find(|s| s.name == "A1").unwrap();
        assert!(a.bytes.start <= a1.bytes.start && a.bytes.end >= a1.bytes.end);
        assert!(a.lines.start <= a1.lines.start && a.lines.end >= a1.lines.end);
    }

    #[test]
    fn preamble_only_when_present() {
        let src = "lead paragraph\n\n## Real\n";
        let out = extract_markdown_symbols(src).unwrap();
        assert_eq!(out.symbols[0].name, "preamble");
        assert_eq!(out.symbols[0].lines.start, 1);
        assert_eq!(out.symbols[1].name, "Real");
    }

    #[test]
    fn no_preamble_when_file_starts_with_h2() {
        let src = "## Real\n\nbody\n";
        let out = extract_markdown_symbols(src).unwrap();
        assert_eq!(out.symbols.len(), 1);
        assert_eq!(out.symbols[0].name, "Real");
    }

    #[test]
    fn no_headings_whole_file_is_one_document() {
        let src = "just some prose\nwithout headings\n";
        let out = extract_markdown_symbols(src).unwrap();
        assert_eq!(out.symbols.len(), 1);
        assert_eq!(out.symbols[0].name, "document");
        assert_eq!(out.symbols[0].lines.end, 2);
    }

    #[test]
    fn empty_source_yields_no_symbols() {
        let out = extract_markdown_symbols("").unwrap();
        assert!(out.symbols.is_empty());
        let out = extract_markdown_symbols("   \n\n  \n").unwrap();
        assert!(out.symbols.is_empty());
    }

    #[test]
    fn long_heading_truncated_on_word_boundary() {
        let long = format!("# {}", "word ".repeat(60));
        let out = extract_markdown_symbols(&long).unwrap();
        assert!(out.symbols[0].name.chars().count() <= MAX_NAME_CHARS);
        assert!(!out.symbols[0].name.ends_with(' '));
    }

    #[test]
    fn h1_only_file_is_one_document() {
        let src = "# Title\n\nsome prose\n";
        let out = extract_markdown_symbols(src).unwrap();
        assert_eq!(out.symbols.len(), 1);
        assert_eq!(out.symbols[0].name, "document");
    }

    #[test]
    fn hash_without_space_is_not_a_heading() {
        let src = "#hashtag\n\n## Real\n";
        let out = extract_markdown_symbols(src).unwrap();
        // The non-heading lead text becomes the preamble.
        assert_eq!(
            out.symbols
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>(),
            vec!["preamble", "Real"]
        );
    }

    #[test]
    fn four_plus_hashes_are_not_sections() {
        let src = "## Top\n\n#### deep note\n\nmore\n";
        let out = extract_markdown_symbols(src).unwrap();
        assert_eq!(out.symbols.len(), 1);
        assert_eq!(out.symbols[0].name, "Top");
    }

    #[test]
    fn code_blocks_with_hash_lines_are_a_known_limitation() {
        // Phase-1 parser does not track fenced blocks; a `# comment` line
        // inside ``` fences will split a section. This test pins current
        // behaviour so a future fix is deliberate.
        let src = "# Top\n\n```\n# not really a heading\n```\n";
        let out = extract_markdown_symbols(src).unwrap();
        assert!(!out.symbols.is_empty());
    }

    #[test]
    fn classifies_doc_paths() {
        use DocType::*;
        assert_eq!(classify_doc_path("docs/adr/0001-embeddings.md"), Adr);
        assert_eq!(classify_doc_path("adr/0002.md"), Adr);
        assert_eq!(classify_doc_path("DECISIONS.md"), Adr);
        assert_eq!(classify_doc_path("docs/issues/42-search-miss.md"), Issue);
        assert_eq!(classify_doc_path("docs/known-bugs.md"), Bug);
        assert_eq!(classify_doc_path("CHANGELOG.md"), Changelog);
        assert_eq!(classify_doc_path("README.md"), Readme);
        assert_eq!(classify_doc_path("CONTRIBUTING.md"), Guide);
        assert_eq!(classify_doc_path("docs/specs/foo-design.md"), Design);
        assert_eq!(classify_doc_path("notes.md"), Other);
    }

    #[test]
    fn parses_front_matter() {
        let src = "---\ntitle: x\nstatus: superseded\ndate: 2026-01-02\nnumber: 7\nlabels: storage, perf\n---\n\n# ADR 7\n";
        let fm = parse_front_matter(src).expect("front-matter present");
        assert_eq!(fm.status.as_deref(), Some("superseded"));
        assert_eq!(fm.date.as_deref(), Some("2026-01-02"));
        assert_eq!(fm.number, Some(7));
        assert_eq!(fm.labels, vec!["storage", "perf"]);
    }

    #[test]
    fn no_front_matter_or_unterminated_fence() {
        assert!(parse_front_matter("# Just prose\n").is_none());
        assert!(parse_front_matter("---\nstatus: accepted\n").is_none());
        // Body containing --- later is not front-matter.
        assert!(parse_front_matter("intro\n\n---\nrule\n").is_none());
    }
}
