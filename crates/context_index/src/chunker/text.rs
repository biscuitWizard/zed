use super::{
    budget::{nws_size, ChunkBudget},
    ids,
    types::{Chunk, ChunkView},
    FileChunker,
};

/// Line-windowed text chunker used when no structural parser is selected.
///
/// TODO: replace this for languages where parser-aware chunking can preserve
/// definitions and hierarchy more accurately.
///
/// Algorithm:
///
/// 1. Split on `\n`, accumulate NWS chars per line.
/// 2. Greedily extend a window while sum ≤ `budget.maximum`.
/// 3. Greedy-merge adjacent windows whose union ≤ `budget.target`.
/// 4. Drop windows below `budget.minimum` unless it's the only one.
pub struct TextChunker {
    pub budget: ChunkBudget,
    pub language_id: &'static str,
    /// If `true`, set `view = Doc` and populate `doc_text`. Used by the
    /// markdown wrapper.
    pub doc_mode: bool,
}

struct LineWindow {
    start_line: usize,
    end_line: usize, // exclusive
    byte_start: usize,
    byte_end: usize,
    nws: usize,
}

impl TextChunker {
    fn split_windows(&self, text: &str) -> Vec<LineWindow> {
        let lines: Vec<&str> = text.split('\n').collect();
        let mut windows: Vec<LineWindow> = Vec::new();

        let mut byte_offset = 0usize;
        let mut cur_start_line = 0usize;
        let mut cur_byte_start = 0usize;
        let mut cur_nws = 0usize;

        for (i, line) in lines.iter().enumerate() {
            let line_nws = nws_size(line);
            let line_bytes = line.len() + if i + 1 < lines.len() { 1 } else { 0 };

            if cur_nws + line_nws > self.budget.maximum && cur_nws > 0 {
                windows.push(LineWindow {
                    start_line: cur_start_line,
                    end_line: i,
                    byte_start: cur_byte_start,
                    byte_end: byte_offset,
                    nws: cur_nws,
                });
                cur_start_line = i;
                cur_byte_start = byte_offset;
                cur_nws = 0;
            }

            cur_nws += line_nws;
            byte_offset += line_bytes;
        }

        if cur_nws > 0 || windows.is_empty() {
            windows.push(LineWindow {
                start_line: cur_start_line,
                end_line: lines.len(),
                byte_start: cur_byte_start,
                byte_end: byte_offset,
                nws: cur_nws,
            });
        }

        windows
    }

    fn greedy_merge(&self, windows: Vec<LineWindow>, source: &[u8]) -> Vec<LineWindow> {
        if windows.is_empty() {
            return windows;
        }

        let mut merged: Vec<LineWindow> = Vec::with_capacity(windows.len());
        let mut iter = windows.into_iter();
        merged.push(iter.next().unwrap());

        for w in iter {
            let prev = merged.last().unwrap();
            let combined_text =
                String::from_utf8_lossy(&source[prev.byte_start..w.byte_end]);
            let combined_nws = nws_size(&combined_text);

            if combined_nws <= self.budget.target {
                let last = merged.last_mut().unwrap();
                last.end_line = w.end_line;
                last.byte_end = w.byte_end;
                last.nws = combined_nws;
            } else {
                merged.push(w);
            }
        }

        merged
    }

    fn drop_tiny(&self, windows: Vec<LineWindow>) -> Vec<LineWindow> {
        if windows.len() <= 1 {
            return windows;
        }
        windows
            .into_iter()
            .filter(|w| w.nws >= self.budget.minimum)
            .collect()
    }
}

impl FileChunker for TextChunker {
    fn chunk_file(&self, file_path: &str, bytes: &[u8]) -> Vec<Chunk> {
        let text = String::from_utf8_lossy(bytes);
        if text.trim().is_empty() {
            return Vec::new();
        }

        let windows = self.split_windows(&text);
        let windows = self.greedy_merge(windows, bytes);
        let windows = self.drop_tiny(windows);

        let view = if self.doc_mode {
            ChunkView::Doc
        } else {
            ChunkView::Code
        };

        let breadcrumb = file_path.to_string();

        windows
            .into_iter()
            .map(|w| {
                let byte_start = w.byte_start as i64;
                let byte_end = w.byte_end as i64;
                let code_text =
                    String::from_utf8_lossy(&bytes[w.byte_start..w.byte_end]).into_owned();

                let doc_text = if self.doc_mode {
                    code_text.clone()
                } else {
                    String::new()
                };

                let embed_text = build_embed_text(
                    file_path,
                    self.language_id,
                    &breadcrumb,
                    &code_text,
                );

                let embed_sha = ids::embed_text_sha(&embed_text);
                let id = ids::chunk_id(file_path, byte_start, byte_end, view);
                let nid = ids::node_id(file_path, byte_start, byte_end);

                Chunk {
                    id,
                    node_id: nid,
                    view,
                    parent_id: None,
                    file_path: file_path.to_string(),
                    language_id: self.language_id.to_string(),
                    kind: "file".to_string(),
                    name: None,
                    byte_start,
                    byte_end,
                    line_start: w.start_line as i32,
                    line_end: w.end_line.saturating_sub(1) as i32,
                    imports: String::new(),
                    signature: String::new(),
                    breadcrumb: breadcrumb.clone(),
                    code_text,
                    doc_text,
                    embed_text,
                    embed_text_sha: embed_sha,
                    referenced_symbols: Vec::new(),
                }
            })
            .collect()
    }
}

fn build_embed_text(
    file_path: &str,
    language_id: &str,
    breadcrumb: &str,
    code_text: &str,
) -> String {
    format!(
        "# {}\n# language: {}\n# {}\n{}",
        file_path, language_id, breadcrumb, code_text
    )
    .trim()
    .to_string()
        + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_chunker() -> TextChunker {
        TextChunker {
            budget: ChunkBudget::from_token_counts(128, 256, 16),
            language_id: "rust",
            doc_mode: false,
        }
    }

    fn make_source(line_count: usize) -> String {
        (0..line_count)
            .map(|i| format!("fn func_{}() {{ /* body */ }}", i))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn produces_multiple_chunks_for_large_file() {
        let chunker = make_chunker();
        let src = make_source(200);
        let chunks = chunker.chunk_file("src/main.rs", src.as_bytes());
        assert!(chunks.len() >= 2, "expected >=2 chunks, got {}", chunks.len());
    }

    #[test]
    fn chunks_within_budget() {
        let chunker = make_chunker();
        let src = make_source(200);
        let chunks = chunker.chunk_file("src/main.rs", src.as_bytes());
        for c in &chunks {
            let nws = super::super::budget::nws_size(&c.code_text);
            assert!(
                nws <= chunker.budget.maximum,
                "chunk nws {} exceeds max {}",
                nws,
                chunker.budget.maximum
            );
        }
    }

    #[test]
    fn deterministic_ids() {
        let chunker = make_chunker();
        let src = make_source(50);
        let a = chunker.chunk_file("a.rs", src.as_bytes());
        let b = chunker.chunk_file("a.rs", src.as_bytes());
        assert_eq!(a.len(), b.len());
        for (ca, cb) in a.iter().zip(b.iter()) {
            assert_eq!(ca.id, cb.id);
            assert_eq!(ca.node_id, cb.node_id);
        }
    }

    #[test]
    fn empty_file_returns_no_chunks() {
        let chunker = make_chunker();
        let chunks = chunker.chunk_file("empty.rs", b"   \n  \n");
        assert!(chunks.is_empty());
    }

    #[test]
    fn single_line_file() {
        let chunker = make_chunker();
        let chunks = chunker.chunk_file("tiny.rs", b"fn main() {}");
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].view, ChunkView::Code);
    }
}
