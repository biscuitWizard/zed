use super::{budget::nws_size, types::Chunk};

// TODO: disable once embeddings are integrated (chunker::dump).
// This module writes directly to stderr so chunk inspection is not hidden by
// dependency log filters or noisy storage internals.
pub fn log_chunks(file_path: &str, chunks: &[Chunk]) {
    if chunks.is_empty() {
        return;
    }

    eprintln!();
    eprintln!("================ CONTEXT INDEX CHUNK DUMP ================");
    eprintln!("file: {file_path}");
    eprintln!("chunks: {}", chunks.len());
    eprintln!("==========================================================");
    for (idx, chunk) in chunks.iter().enumerate() {
        eprintln!("{}", format_chunk_block(idx, chunks.len(), chunk));
    }
    eprintln!("================ END CONTEXT INDEX CHUNKS ================");
    eprintln!();
}

fn format_chunk_block(idx: usize, total: usize, chunk: &Chunk) -> String {
    format!(
        "\
---------------- chunk {}/{} ----------------
id: {}
node_id: {}
view: {}
language: {}
kind: {}
name: {}
parent_id: {}
lines: {}..{}
bytes: {}..{}
embed_text_sha: {}
embed_text_nws: {}
breadcrumb: {}
signature:
{}
imports:
{}
referenced_symbols: {}
embed_text:
{}
---------------- end chunk {}/{} ----------------
",
        idx + 1,
        total,
        chunk.id,
        chunk.node_id,
        chunk.view,
        chunk.language_id,
        chunk.kind,
        chunk.name.as_deref().unwrap_or("-"),
        chunk.parent_id.as_deref().unwrap_or("-"),
        chunk.line_start,
        chunk.line_end,
        chunk.byte_start,
        chunk.byte_end,
        chunk.embed_text_sha,
        nws_size(&chunk.embed_text),
        chunk.breadcrumb,
        chunk.signature,
        chunk.imports,
        if chunk.referenced_symbols.is_empty() {
            "-".to_string()
        } else {
            chunk.referenced_symbols.join(", ")
        },
        chunk.embed_text.trim_end(),
        idx + 1,
        total
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunker::ChunkView;

    #[test]
    fn formats_chunk_summary() {
        let chunk = Chunk {
            id: "id".to_string(),
            node_id: "node".to_string(),
            view: ChunkView::Code,
            parent_id: None,
            file_path: "src/main.rs".to_string(),
            language_id: "rust".to_string(),
            kind: "function_item".to_string(),
            name: Some("main".to_string()),
            byte_start: 0,
            byte_end: 12,
            line_start: 0,
            line_end: 0,
            imports: String::new(),
            signature: "fn main()".to_string(),
            breadcrumb: "src/main.rs > main".to_string(),
            code_text: "fn main() {}".to_string(),
            doc_text: String::new(),
            embed_text: "fn main() {}".to_string(),
            embed_text_sha: "sha".to_string(),
            referenced_symbols: Vec::new(),
        };

        let summary = format_chunk_block(0, 1, &chunk);
        assert!(summary.contains("chunk 1/1"));
        assert!(summary.contains("kind: function_item"));
        assert!(summary.contains("name: main"));
        assert!(summary.contains("embed_text:"));
    }
}
