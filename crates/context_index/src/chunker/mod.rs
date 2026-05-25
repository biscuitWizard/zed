pub mod budget;
pub mod ids;
pub mod markdown;
pub mod text;
pub mod types;

pub use budget::ChunkBudget;
pub use types::{Chunk, ChunkView};

pub trait FileChunker: Send + Sync {
    fn chunk_file(&self, file_path: &str, bytes: &[u8]) -> Vec<Chunk>;
}

/// Returns the language id for a file extension, or `None` if the extension
/// is not supported (the file should be skipped).
pub fn language_id_for_extension(ext: &str) -> Option<&'static str> {
    match ext {
        "py" => Some("python"),
        "rs" => Some("rust"),
        "cs" => Some("csharp"),
        "go" => Some("go"),
        "js" | "mjs" | "cjs" => Some("javascript"),
        "ts" | "tsx" => Some("typescript"),
        "jsx" => Some("javascriptreact"),
        "md" | "markdown" => Some("markdown"),
        "txt" => Some("plaintext"),
        _ => None,
    }
}

/// Returns a chunker for the given file extension, or `None` if the extension
/// is unsupported (the file should be skipped entirely).
pub fn chunker_for_extension(ext: &str, budget: ChunkBudget) -> Option<Box<dyn FileChunker>> {
    let language_id = language_id_for_extension(ext)?;

    match ext {
        "md" | "markdown" => Some(Box::new(markdown::MarkdownChunker::new(budget))),
        _ => Some(Box::new(text::TextChunker {
            budget,
            language_id,
            doc_mode: false,
        })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_extensions() {
        let budget = ChunkBudget::default();
        for ext in &[
            "py", "rs", "cs", "go", "js", "mjs", "cjs", "ts", "tsx", "jsx", "md", "markdown",
            "txt",
        ] {
            assert!(
                chunker_for_extension(ext, budget).is_some(),
                "expected chunker for .{ext}"
            );
            assert!(
                language_id_for_extension(ext).is_some(),
                "expected language_id for .{ext}"
            );
        }
    }

    #[test]
    fn unsupported_extensions_return_none() {
        let budget = ChunkBudget::default();
        for ext in &["png", "jpg", "exe", "zip", "pdf", "wasm", "o", "dll"] {
            assert!(
                chunker_for_extension(ext, budget).is_none(),
                "expected None for .{ext}"
            );
            assert!(
                language_id_for_extension(ext).is_none(),
                "expected None language_id for .{ext}"
            );
        }
    }

    #[test]
    fn language_ids_correct() {
        assert_eq!(language_id_for_extension("py"), Some("python"));
        assert_eq!(language_id_for_extension("rs"), Some("rust"));
        assert_eq!(language_id_for_extension("cs"), Some("csharp"));
        assert_eq!(language_id_for_extension("go"), Some("go"));
        assert_eq!(language_id_for_extension("js"), Some("javascript"));
        assert_eq!(language_id_for_extension("mjs"), Some("javascript"));
        assert_eq!(language_id_for_extension("cjs"), Some("javascript"));
        assert_eq!(language_id_for_extension("ts"), Some("typescript"));
        assert_eq!(language_id_for_extension("tsx"), Some("typescript"));
        assert_eq!(language_id_for_extension("jsx"), Some("javascriptreact"));
        assert_eq!(language_id_for_extension("md"), Some("markdown"));
        assert_eq!(language_id_for_extension("markdown"), Some("markdown"));
        assert_eq!(language_id_for_extension("txt"), Some("plaintext"));
    }

    #[test]
    fn markdown_chunker_uses_doc_view() {
        let budget = ChunkBudget::default();
        let chunker = chunker_for_extension("md", budget).unwrap();
        let chunks = chunker.chunk_file("readme.md", b"# Hello\nSome text content here.");
        assert!(!chunks.is_empty());
        assert_eq!(chunks[0].view, ChunkView::Doc);
        assert_eq!(chunks[0].language_id, "markdown");
        assert!(!chunks[0].doc_text.is_empty());
    }

    #[test]
    fn code_chunker_uses_code_view() {
        let budget = ChunkBudget::default();
        let chunker = chunker_for_extension("rs", budget).unwrap();
        let chunks = chunker.chunk_file("main.rs", b"fn main() { println!(\"hi\"); }");
        assert!(!chunks.is_empty());
        assert_eq!(chunks[0].view, ChunkView::Code);
        assert_eq!(chunks[0].language_id, "rust");
        assert!(chunks[0].doc_text.is_empty());
    }
}
