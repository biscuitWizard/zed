use arrow_schema::{DataType, Field, Schema};

pub const CHUNKS_TABLE: &str = "chunks";

/// Arrow schema for the `chunks` table.
///
/// The vector column is nullable so file contents can be indexed before an
/// embedding worker has produced vectors for those chunks.
pub fn chunks_schema(embedding_dim: usize) -> Schema {
    Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("node_id", DataType::Utf8, false),
        Field::new("view", DataType::Utf8, false),
        Field::new("parent_id", DataType::Utf8, true),
        Field::new("file_path", DataType::Utf8, false),
        Field::new("language_id", DataType::Utf8, false),
        Field::new("kind", DataType::Utf8, false),
        Field::new("name", DataType::Utf8, true),
        Field::new("byte_start", DataType::Int64, false),
        Field::new("byte_end", DataType::Int64, false),
        Field::new("line_start", DataType::Int32, false),
        Field::new("line_end", DataType::Int32, false),
        Field::new("imports", DataType::Utf8, false),
        Field::new("signature", DataType::Utf8, false),
        Field::new("breadcrumb", DataType::Utf8, false),
        Field::new("code_text", DataType::Utf8, false),
        Field::new("doc_text", DataType::Utf8, false),
        Field::new(
            "referenced_symbols",
            DataType::List(Box::new(Field::new("item", DataType::Utf8, true)).into()),
            false,
        ),
        Field::new("fts_text", DataType::Utf8, false),
        Field::new("embed_text", DataType::Utf8, false),
        Field::new("embed_text_sha", DataType::Utf8, false),
        Field::new(
            "vector",
            DataType::FixedSizeList(
                Box::new(Field::new("item", DataType::Float32, true)).into(),
                embedding_dim as i32,
            ),
            true,
        ),
    ])
}
