use super::{
    FileChunker,
    ast::greedy_merge_ranges,
    budget::{ChunkBudget, nws_size},
    ids,
    text::TextChunker,
    types::{Chunk, ChunkView},
};

pub struct MarkdownChunker {
    budget: ChunkBudget,
    fallback: TextChunker,
}

impl MarkdownChunker {
    pub fn new(budget: ChunkBudget) -> Self {
        Self {
            budget,
            fallback: TextChunker {
                budget,
                language_id: "markdown",
                doc_mode: true,
            },
        }
    }
}

impl FileChunker for MarkdownChunker {
    fn chunk_file(&self, file_path: &str, bytes: &[u8]) -> Vec<Chunk> {
        let text = String::from_utf8_lossy(bytes);
        if text.trim().is_empty() {
            return Vec::new();
        }

        let mut parser = tree_sitter::Parser::new();
        let language = tree_sitter_md::LANGUAGE.into();
        if parser.set_language(&language).is_err() {
            return self.fallback.chunk_file(file_path, bytes);
        }

        let Some(tree) = parser.parse(bytes, None) else {
            return self.fallback.chunk_file(file_path, bytes);
        };

        let mut headings = Vec::new();
        collect_headings(tree.root_node(), file_path, bytes, &mut headings);
        if headings.is_empty() {
            return self.fallback.chunk_file(file_path, bytes);
        }

        for idx in 0..headings.len() {
            headings[idx].byte_end = headings
                .get(idx + 1)
                .map_or(bytes.len(), |next| next.byte_start);
            headings[idx].line_end = line_for_byte(bytes, headings[idx].byte_end.saturating_sub(1));
        }

        let ranges = headings
            .iter()
            .map(|heading| (heading.byte_start, heading.byte_end))
            .collect::<Vec<_>>();
        let merged = greedy_merge_ranges(&ranges, bytes, self.budget);
        merged
            .into_iter()
            .filter_map(|(byte_start, byte_end)| {
                let first = headings.iter().find(|heading| {
                    heading.byte_start >= byte_start && heading.byte_start < byte_end
                })?;
                let doc_text = String::from_utf8_lossy(&bytes[byte_start..byte_end]).into_owned();
                if nws_size(&doc_text) < self.budget.minimum && headings.len() > 1 {
                    return None;
                }
                let embed_text = build_embed_text(file_path, &first.breadcrumb, &doc_text);
                let byte_start_i64 = byte_start as i64;
                let byte_end_i64 = byte_end as i64;
                Some(Chunk {
                    id: ids::chunk_id(file_path, byte_start_i64, byte_end_i64, ChunkView::Doc),
                    node_id: ids::node_id(file_path, byte_start_i64, byte_end_i64),
                    view: ChunkView::Doc,
                    parent_id: None,
                    file_path: file_path.to_string(),
                    language_id: "markdown".to_string(),
                    kind: "section".to_string(),
                    name: Some(first.name.clone()),
                    byte_start: byte_start_i64,
                    byte_end: byte_end_i64,
                    line_start: first.line_start as i32,
                    line_end: line_for_byte(bytes, byte_end.saturating_sub(1)) as i32,
                    imports: String::new(),
                    signature: first.name.clone(),
                    breadcrumb: first.breadcrumb.clone(),
                    code_text: doc_text.clone(),
                    doc_text,
                    embed_text_sha: ids::embed_text_sha(&embed_text),
                    embed_text,
                    referenced_symbols: Vec::new(),
                })
            })
            .collect()
    }
}

struct Heading {
    level: usize,
    name: String,
    breadcrumb: String,
    byte_start: usize,
    byte_end: usize,
    line_start: usize,
    line_end: usize,
}

fn collect_headings(
    node: tree_sitter::Node<'_>,
    file_path: &str,
    source: &[u8],
    headings: &mut Vec<Heading>,
) {
    if matches!(node.kind(), "atx_heading" | "setext_heading") {
        let level = heading_level(node, source);
        let name = heading_name(node, source);
        if !name.is_empty() {
            let breadcrumb = markdown_breadcrumb(file_path, headings, level, &name);
            headings.push(Heading {
                level,
                name,
                breadcrumb,
                byte_start: node.start_byte(),
                byte_end: node.end_byte(),
                line_start: node.start_position().row,
                line_end: node.end_position().row,
            });
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_headings(child, file_path, source, headings);
    }
}

fn markdown_breadcrumb(file_path: &str, previous: &[Heading], level: usize, name: &str) -> String {
    let mut ancestors = Vec::new();
    for heading in previous.iter().rev() {
        if heading.level < level {
            ancestors.push(heading.name.clone());
        }
    }
    ancestors.reverse();

    let mut parts = vec![file_path.to_string()];
    parts.extend(ancestors);
    parts.push(name.to_string());
    parts.join(" > ")
}

fn heading_level(node: tree_sitter::Node<'_>, source: &[u8]) -> usize {
    let text = String::from_utf8_lossy(&source[node.start_byte()..node.end_byte()]);
    if node.kind() == "setext_heading" {
        if text.lines().any(|line| line.trim_start().starts_with('=')) {
            return 1;
        }
        return 2;
    }

    text.chars().take_while(|ch| *ch == '#').count().max(1)
}

fn heading_name(node: tree_sitter::Node<'_>, source: &[u8]) -> String {
    let text = String::from_utf8_lossy(&source[node.start_byte()..node.end_byte()]);
    if node.kind() == "setext_heading" {
        return text.lines().next().unwrap_or_default().trim().to_string();
    }
    text.trim()
        .trim_start_matches('#')
        .trim()
        .trim_end_matches('#')
        .trim()
        .to_string()
}

fn build_embed_text(file_path: &str, breadcrumb: &str, doc_text: &str) -> String {
    format!("# {file_path}\n# language: markdown\n# {breadcrumb}\n{doc_text}")
        .trim()
        .to_string()
        + "\n"
}

fn line_for_byte(source: &[u8], byte: usize) -> usize {
    source[..byte.min(source.len())]
        .iter()
        .filter(|b| **b == b'\n')
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_splits_on_headings() {
        let chunker = MarkdownChunker::new(ChunkBudget::from_token_counts(1, 512, 1));
        let chunks = chunker.chunk_file(
            "README.md",
            b"# Setup\nIntro\n## Database\nUse sqlite.\n## Server\nRun it.\n",
        );
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[1].kind, "section");
        assert_eq!(chunks[1].name.as_deref(), Some("Database"));
        assert_eq!(chunks[1].breadcrumb, "README.md > Setup > Database");
    }

    #[test]
    fn markdown_without_headings_falls_back_to_text_chunker() {
        let chunker = MarkdownChunker::new(ChunkBudget::from_token_counts(128, 512, 1));
        let chunks = chunker.chunk_file("README.md", b"plain text\nwithout headings\n");
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].kind, "file");
        assert_eq!(chunks[0].view, ChunkView::Doc);
    }

    #[test]
    fn markdown_greedy_merges_small_adjacent_sections() {
        let chunker = MarkdownChunker::new(ChunkBudget::from_token_counts(32, 512, 1));
        let chunks = chunker.chunk_file("README.md", b"# A\na\n## B\nb\n## C\nc\n");
        assert_eq!(chunks.len(), 1);
    }
}
