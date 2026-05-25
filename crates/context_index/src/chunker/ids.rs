use sha2::{Digest, Sha256};

use super::types::ChunkView;

fn hex_sha256(input: &str) -> String {
    let hash = Sha256::digest(input.as_bytes());
    let mut s = String::with_capacity(64);
    for b in hash.as_slice() {
        use std::fmt::Write;
        let _ = write!(s, "{:02x}", b);
    }
    s
}

/// Deterministic chunk id: `sha256("{path}|{byte_start}|{byte_end}|{view}")`.
pub fn chunk_id(path: &str, byte_start: i64, byte_end: i64, view: ChunkView) -> String {
    hex_sha256(&format!(
        "{}|{}|{}|{}",
        path,
        byte_start,
        byte_end,
        view.as_str()
    ))
}

/// Deterministic node id shared between CODE and DOC views:
/// `sha256("{path}|{byte_start}|{byte_end}")`.
pub fn node_id(path: &str, byte_start: i64, byte_end: i64) -> String {
    hex_sha256(&format!("{}|{}|{}", path, byte_start, byte_end))
}

/// `sha256(embed_text)` for cache-skip decisions during embedding.
pub fn embed_text_sha(text: &str) -> String {
    hex_sha256(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_chunk_id() {
        let a = chunk_id("src/main.rs", 0, 100, ChunkView::Code);
        let b = chunk_id("src/main.rs", 0, 100, ChunkView::Code);
        assert_eq!(a, b);
        assert_eq!(a.len(), 64);
    }

    #[test]
    fn chunk_id_differs_by_view() {
        let code = chunk_id("a.rs", 0, 10, ChunkView::Code);
        let doc = chunk_id("a.rs", 0, 10, ChunkView::Doc);
        assert_ne!(code, doc);
    }

    #[test]
    fn node_id_stable() {
        let a = node_id("a.rs", 0, 10);
        let b = node_id("a.rs", 0, 10);
        assert_eq!(a, b);
        assert_eq!(a.len(), 64);
    }

    #[test]
    fn embed_text_sha_stable() {
        let a = embed_text_sha("hello world");
        let b = embed_text_sha("hello world");
        assert_eq!(a, b);
        assert_eq!(a.len(), 64);
        assert_ne!(a, embed_text_sha("different"));
    }
}
