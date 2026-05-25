use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ChunkView {
    Code,
    Doc,
}

impl ChunkView {
    pub fn as_str(&self) -> &'static str {
        match self {
            ChunkView::Code => "code",
            ChunkView::Doc => "doc",
        }
    }
}

impl fmt::Display for ChunkView {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A persisted chunk row before embedding vectors are attached.
#[derive(Clone, Debug)]
pub struct Chunk {
    /// Unique per row: `sha256(file_path|byte_start|byte_end|view)`.
    pub id: String,
    /// Shared between CODE and DOC views of the same definition:
    /// `sha256(file_path|byte_start|byte_end)`.
    pub node_id: String,
    pub view: ChunkView,

    pub parent_id: Option<String>,
    pub file_path: String,
    pub language_id: String,

    /// Source construct kind. Text chunking records whole-file windows.
    /// TODO: populate parser-specific kinds when structural chunkers are added.
    pub kind: String,
    pub name: Option<String>,

    pub byte_start: i64,
    pub byte_end: i64,
    pub line_start: i32,
    pub line_end: i32,

    pub imports: String,
    pub signature: String,
    pub breadcrumb: String,

    pub code_text: String,
    pub doc_text: String,
    pub embed_text: String,
    /// `sha256(embed_text)` hex string, used for cache-skip decisions.
    pub embed_text_sha: String,
    pub referenced_symbols: Vec<String>,
}
