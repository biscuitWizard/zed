/// Markdown chunker that currently delegates to TextChunker with document view
/// fields populated.
///
/// TODO: split on heading boundaries while respecting frontmatter and fenced
/// code blocks.
use super::{
    budget::ChunkBudget,
    text::TextChunker,
    types::Chunk,
    FileChunker,
};

pub struct MarkdownChunker {
    inner: TextChunker,
}

impl MarkdownChunker {
    pub fn new(budget: ChunkBudget) -> Self {
        Self {
            inner: TextChunker {
                budget,
                language_id: "markdown",
                doc_mode: true,
            },
        }
    }
}

impl FileChunker for MarkdownChunker {
    fn chunk_file(&self, file_path: &str, bytes: &[u8]) -> Vec<Chunk> {
        self.inner.chunk_file(file_path, bytes)
    }
}
