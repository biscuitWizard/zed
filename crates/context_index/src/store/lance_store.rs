use std::path::Path;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use arrow_array::{
    Array, FixedSizeListArray, Int32Array, Int64Array, ListArray, RecordBatch,
    StringArray,
};
use arrow_schema::Schema;
use futures::TryStreamExt;
use lancedb::connection::Connection;
use lancedb::query::{ExecutableQuery, QueryBase, Select};
use lancedb::table::Table;

use crate::chunker::types::Chunk;

use super::schema::{chunks_schema, CHUNKS_TABLE};

#[derive(Debug, Clone)]
pub struct LanceStoreStats {
    pub total_rows: u64,
    pub files: u64,
}

pub struct LanceStore {
    db: Connection,
    table: Table,
    embedding_dim: usize,
}

impl LanceStore {
    /// Open or create the store at `data_dir`. On first use `embedding_dim`
    /// sets the vector column width (cannot change after table creation).
    pub async fn open(data_dir: &Path, embedding_dim: usize) -> Result<Self> {
        std::fs::create_dir_all(data_dir)
            .with_context(|| format!("creating lance dir {}", data_dir.display()))?;

        let db = lancedb::connect(data_dir.to_str().unwrap())
            .execute()
            .await
            .context("connecting to lancedb")?;

        let table_names = db.table_names().execute().await.unwrap_or_default();

        let table = if table_names.iter().any(|n| n == CHUNKS_TABLE) {
            db.open_table(CHUNKS_TABLE)
                .execute()
                .await
                .context("opening chunks table")?
        } else {
            let schema = chunks_schema(embedding_dim);
            let batch = empty_batch(&schema)?;
            db.create_table(CHUNKS_TABLE, batch)
                .execute()
                .await
                .context("creating chunks table")?
        };

        Ok(Self {
            db,
            table,
            embedding_dim,
        })
    }

    pub fn embedding_dim(&self) -> usize {
        self.embedding_dim
    }

    /// Insert or replace chunk rows (vectors are null).
    pub async fn upsert_chunks(&self, chunks: &[Chunk]) -> Result<()> {
        if chunks.is_empty() {
            return Ok(());
        }
        self.delete_by_ids(chunks).await?;
        let batch = chunks_to_batch(chunks, self.embedding_dim, None)?;
        self.table
            .add(batch)
            .execute()
            .await
            .context("adding chunk rows")?;
        Ok(())
    }

    /// Insert or replace chunk rows with precomputed embedding vectors.
    pub async fn upsert_chunks_with_vectors(
        &self,
        chunks: &[Chunk],
        vectors: &[Vec<f32>],
    ) -> Result<()> {
        if chunks.is_empty() {
            return Ok(());
        }
        anyhow::ensure!(
            chunks.len() == vectors.len(),
            "chunks ({}) and vectors ({}) length mismatch",
            chunks.len(),
            vectors.len()
        );
        self.delete_by_ids(chunks).await?;
        let batch = chunks_to_batch(chunks, self.embedding_dim, Some(vectors))?;
        self.table
            .add(batch)
            .execute()
            .await
            .context("adding chunk rows with vectors")?;
        Ok(())
    }

    /// Delete every row whose `file_path` is in `paths`.
    pub async fn delete_by_file_paths(&self, paths: &[&str]) -> Result<u64> {
        if paths.is_empty() {
            return Ok(0);
        }
        let before = self.count_rows().await.unwrap_or(0);
        let in_clause = paths
            .iter()
            .map(|p| format!("'{}'", p.replace('\'', "''")))
            .collect::<Vec<_>>()
            .join(", ");
        self.table
            .delete(&format!("file_path IN ({in_clause})"))
            .await
            .context("deleting by file_paths")?;
        let after = self.count_rows().await.unwrap_or(0);
        Ok(before.saturating_sub(after))
    }

    /// Delete every chunk row while preserving the existing table schema.
    pub async fn delete_all(&self) -> Result<u64> {
        let before = self.count_rows().await.unwrap_or(0);
        self.table
            .delete("id IS NOT NULL")
            .await
            .context("deleting all chunk rows")?;
        let after = self.count_rows().await.unwrap_or(0);
        Ok(before.saturating_sub(after))
    }

    /// Drop the table and recreate it empty.
    pub async fn drop_and_recreate(&mut self, embedding_dim: usize) -> Result<()> {
        let _ = self.db.drop_table(CHUNKS_TABLE, &[]).await;
        let schema = chunks_schema(embedding_dim);
        let batch = empty_batch(&schema)?;
        self.table = self
            .db
            .create_table(CHUNKS_TABLE, batch)
            .execute()
            .await
            .context("recreating chunks table")?;
        self.embedding_dim = embedding_dim;
        Ok(())
    }

    pub async fn count_rows(&self) -> Result<u64> {
        let count = self
            .table
            .count_rows(None)
            .await
            .context("counting rows")?;
        Ok(count as u64)
    }

    pub async fn stats(&self) -> Result<LanceStoreStats> {
        let total_rows = self.count_rows().await?;
        let mut files = std::collections::HashSet::new();
        let mut stream = self
            .table
            .query()
            .select(Select::columns(&["file_path"]))
            .execute()
            .await
            .context("querying chunk file paths")?;

        while let Some(batch) = stream.try_next().await.context("reading chunk file paths")? {
            let file_paths = batch
                .column(0)
                .as_any()
                .downcast_ref::<StringArray>()
                .context("file_path column should be Utf8")?;

            for row in 0..file_paths.len() {
                if !file_paths.is_null(row) {
                    files.insert(file_paths.value(row).to_string());
                }
            }
        }

        Ok(LanceStoreStats {
            total_rows,
            files: files.len() as u64,
        })
    }

    // ------------------------------------------------------------------
    // Private helpers
    // ------------------------------------------------------------------

    async fn delete_by_ids(&self, chunks: &[Chunk]) -> Result<()> {
        if chunks.is_empty() {
            return Ok(());
        }
        let in_clause = chunks
            .iter()
            .map(|c| format!("'{}'", c.id.replace('\'', "''")))
            .collect::<Vec<_>>()
            .join(", ");
        // Missing ids mean the row set was already invalidated by another path.
        let _ = self
            .table
            .delete(&format!("id IN ({in_clause})"))
            .await;
        Ok(())
    }
}

/// Build the text body used by the future full-text search index.
fn build_fts_text(chunk: &Chunk) -> String {
    let parts: Vec<&str> = [
        chunk.file_path.as_str(),
        chunk.breadcrumb.as_str(),
        chunk.signature.as_str(),
        chunk.name.as_deref().unwrap_or(""),
        chunk.doc_text.as_str(),
        chunk.code_text.as_str(),
    ]
    .into_iter()
    .filter(|p| !p.is_empty())
    .collect();

    let mut text = parts.join("\n");
    if !chunk.referenced_symbols.is_empty() {
        text.push('\n');
        text.push_str(&chunk.referenced_symbols.join(" "));
    }
    text
}

fn chunks_to_batch(
    chunks: &[Chunk],
    embedding_dim: usize,
    vectors: Option<&[Vec<f32>]>,
) -> Result<RecordBatch> {
    let len = chunks.len();
    let schema = Arc::new(chunks_schema(embedding_dim));

    let ids: StringArray = chunks.iter().map(|c| Some(c.id.as_str())).collect();
    let node_ids: StringArray = chunks.iter().map(|c| Some(c.node_id.as_str())).collect();
    let views: StringArray = chunks.iter().map(|c| Some(c.view.as_str())).collect();
    let parent_ids: StringArray = chunks.iter().map(|c| c.parent_id.as_deref()).collect();
    let file_paths: StringArray = chunks.iter().map(|c| Some(c.file_path.as_str())).collect();
    let language_ids: StringArray = chunks
        .iter()
        .map(|c| Some(c.language_id.as_str()))
        .collect();
    let kinds: StringArray = chunks.iter().map(|c| Some(c.kind.as_str())).collect();
    let names: StringArray = chunks.iter().map(|c| c.name.as_deref()).collect();
    let byte_starts: Int64Array = chunks.iter().map(|c| Some(c.byte_start)).collect();
    let byte_ends: Int64Array = chunks.iter().map(|c| Some(c.byte_end)).collect();
    let line_starts: Int32Array = chunks.iter().map(|c| Some(c.line_start)).collect();
    let line_ends: Int32Array = chunks.iter().map(|c| Some(c.line_end)).collect();
    let imports: StringArray = chunks.iter().map(|c| Some(c.imports.as_str())).collect();
    let signatures: StringArray = chunks.iter().map(|c| Some(c.signature.as_str())).collect();
    let breadcrumbs: StringArray = chunks.iter().map(|c| Some(c.breadcrumb.as_str())).collect();
    let code_texts: StringArray = chunks.iter().map(|c| Some(c.code_text.as_str())).collect();
    let doc_texts: StringArray = chunks.iter().map(|c| Some(c.doc_text.as_str())).collect();

    let ref_syms = build_ref_symbols_array(chunks)?;

    let fts_texts: StringArray = chunks
        .iter()
        .map(|c| Some(build_fts_text(c)))
        .collect();
    let embed_texts: StringArray = chunks.iter().map(|c| Some(c.embed_text.as_str())).collect();
    let embed_text_shas: StringArray = chunks
        .iter()
        .map(|c| Some(c.embed_text_sha.as_str()))
        .collect();

    let vector_col = build_vector_array(len, embedding_dim, vectors)?;

    let batch = RecordBatch::try_new(
        schema,
        vec![
            Arc::new(ids),
            Arc::new(node_ids),
            Arc::new(views),
            Arc::new(parent_ids),
            Arc::new(file_paths),
            Arc::new(language_ids),
            Arc::new(kinds),
            Arc::new(names),
            Arc::new(byte_starts),
            Arc::new(byte_ends),
            Arc::new(line_starts),
            Arc::new(line_ends),
            Arc::new(imports),
            Arc::new(signatures),
            Arc::new(breadcrumbs),
            Arc::new(code_texts),
            Arc::new(doc_texts),
            Arc::new(ref_syms),
            Arc::new(fts_texts),
            Arc::new(embed_texts),
            Arc::new(embed_text_shas),
            Arc::new(vector_col),
        ],
    )
    .context("building RecordBatch")?;

    Ok(batch)
}

fn build_ref_symbols_array(chunks: &[Chunk]) -> Result<ListArray> {
    use arrow_array::builder::{ListBuilder, StringBuilder};

    let mut builder = ListBuilder::new(StringBuilder::new());
    for c in chunks {
        let values = builder.values();
        for sym in &c.referenced_symbols {
            values.append_value(sym);
        }
        builder.append(true);
    }
    Ok(builder.finish())
}

fn build_vector_array(
    len: usize,
    embedding_dim: usize,
    vectors: Option<&[Vec<f32>]>,
) -> Result<FixedSizeListArray> {
    use arrow_array::builder::{FixedSizeListBuilder, Float32Builder};

    let mut builder =
        FixedSizeListBuilder::new(Float32Builder::new(), embedding_dim as i32);

    for i in 0..len {
        match vectors {
            Some(vecs) => {
                let v = &vecs[i];
                anyhow::ensure!(
                    v.len() == embedding_dim,
                    "vector[{i}] has dim {} but expected {embedding_dim}",
                    v.len()
                );
                let values = builder.values();
                for &f in v {
                    values.append_value(f);
                }
                builder.append(true);
            }
            None => {
                let values = builder.values();
                for _ in 0..embedding_dim {
                    values.append_null();
                }
                builder.append(false); // null entry
            }
        }
    }

    Ok(builder.finish())
}

fn empty_batch(schema: &Schema) -> Result<RecordBatch> {
    let schema_ref = Arc::new(schema.clone());
    let columns: Vec<Arc<dyn Array>> = schema
        .fields()
        .iter()
        .map(|f| arrow_array::new_empty_array(f.data_type()))
        .collect();
    RecordBatch::try_new(schema_ref, columns).context("building empty batch")
}
