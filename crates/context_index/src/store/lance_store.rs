use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

use anyhow::{Context as _, Result};
use arrow_array::{
    Array, FixedSizeListArray, Float32Array, Float64Array, Int32Array, Int64Array, ListArray,
    RecordBatch, StringArray,
};
use arrow_schema::Schema;
use collections::HashMap;
use futures::TryStreamExt;
use lancedb::connection::Connection;
use lancedb::index::{
    Index,
    scalar::{FtsIndexBuilder, FullTextSearchQuery},
};
use lancedb::query::{ExecutableQuery, QueryBase, Select};
use lancedb::table::Table;
use tokio::runtime::Runtime;

use crate::chunker::types::Chunk;

use super::schema::{CHUNKS_TABLE, chunks_schema};

#[derive(Debug, Clone)]
pub struct LanceStoreStats {
    pub total_rows: u64,
    pub files: u64,
}

#[derive(Debug, Clone)]
pub struct ChunkRow {
    pub id: String,
    pub node_id: String,
    pub view: String,
    pub parent_id: Option<String>,
    pub file_path: String,
    pub language_id: String,
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
    pub referenced_symbols: Vec<String>,
    pub score: f32,
}

pub struct LanceStore {
    db: Connection,
    table: Table,
    embedding_dim: usize,
}

const ROW_COLUMNS: &[&str] = &[
    "id",
    "node_id",
    "view",
    "parent_id",
    "file_path",
    "language_id",
    "kind",
    "name",
    "byte_start",
    "byte_end",
    "line_start",
    "line_end",
    "imports",
    "signature",
    "breadcrumb",
    "code_text",
    "doc_text",
    "referenced_symbols",
];

static LANCE_RUNTIME: LazyLock<Runtime> = LazyLock::new(|| {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("context-index-lancedb")
        .build()
        .expect("failed to create context index LanceDB runtime")
});

impl LanceStore {
    /// Open or create the store at `data_dir`. On first use `embedding_dim`
    /// sets the vector column width (cannot change after table creation).
    pub async fn open(data_dir: &Path, embedding_dim: usize) -> Result<Self> {
        let data_dir = data_dir.to_path_buf();
        run_on_lance_runtime(async move { Self::open_inner(data_dir, embedding_dim).await }).await
    }

    pub fn embedding_dim(&self) -> usize {
        self.embedding_dim
    }

    /// Insert or replace chunk rows (vectors are null).
    pub async fn upsert_chunks(&self, chunks: &[Chunk]) -> Result<()> {
        if chunks.is_empty() {
            return Ok(());
        }
        let table = self.table.clone();
        let embedding_dim = self.embedding_dim;
        let chunks = chunks.to_vec();
        run_on_lance_runtime(async move {
            delete_by_ids(&table, &chunks).await?;
            let batch = chunks_to_batch(&chunks, embedding_dim, None)?;
            table
                .add(batch)
                .execute()
                .await
                .context("adding chunk rows")?;
            Ok(())
        })
        .await
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
        let table = self.table.clone();
        let embedding_dim = self.embedding_dim;
        let chunks = chunks.to_vec();
        let vectors = vectors.to_vec();
        run_on_lance_runtime(async move {
            delete_by_ids(&table, &chunks).await?;
            let batch = chunks_to_batch(&chunks, embedding_dim, Some(&vectors))?;
            table
                .add(batch)
                .execute()
                .await
                .context("adding chunk rows with vectors")?;
            Ok(())
        })
        .await
    }

    /// Delete every row whose `file_path` is in `paths`.
    pub async fn delete_by_file_paths(&self, paths: &[&str]) -> Result<u64> {
        if paths.is_empty() {
            return Ok(0);
        }
        let table = self.table.clone();
        let paths = paths
            .iter()
            .map(|path| path.to_string())
            .collect::<Vec<_>>();
        run_on_lance_runtime(async move {
            let before = count_rows_table(&table).await.unwrap_or(0);
            let in_clause = paths
                .iter()
                .map(|p| format!("'{}'", p.replace('\'', "''")))
                .collect::<Vec<_>>()
                .join(", ");
            table
                .delete(&format!("file_path IN ({in_clause})"))
                .await
                .context("deleting by file_paths")?;
            let after = count_rows_table(&table).await.unwrap_or(0);
            Ok(before.saturating_sub(after))
        })
        .await
    }

    /// Delete every chunk row while preserving the existing table schema.
    pub async fn delete_all(&self) -> Result<u64> {
        let table = self.table.clone();
        run_on_lance_runtime(async move {
            let before = count_rows_table(&table).await.unwrap_or(0);
            table
                .delete("id IS NOT NULL")
                .await
                .context("deleting all chunk rows")?;
            let after = count_rows_table(&table).await.unwrap_or(0);
            Ok(before.saturating_sub(after))
        })
        .await
    }

    /// Drop the table and recreate it empty.
    pub async fn drop_and_recreate(&mut self, embedding_dim: usize) -> Result<()> {
        let db = self.db.clone();
        let table = run_on_lance_runtime(async move {
            let _ = db.drop_table(CHUNKS_TABLE, &[]).await;
            let schema = chunks_schema(embedding_dim);
            let batch = empty_batch(&schema)?;
            db.create_table(CHUNKS_TABLE, batch)
                .execute()
                .await
                .context("recreating chunks table")
        })
        .await?;
        self.table = table;
        self.embedding_dim = embedding_dim;
        Ok(())
    }

    pub async fn count_rows(&self) -> Result<u64> {
        let table = self.table.clone();
        run_on_lance_runtime(async move { count_rows_table(&table).await }).await
    }

    pub async fn stats(&self) -> Result<LanceStoreStats> {
        let table = self.table.clone();
        run_on_lance_runtime(async move {
            let total_rows = count_rows_table(&table).await?;
            let mut files = std::collections::HashSet::new();
            let mut stream = table
                .query()
                .select(Select::columns(&["file_path"]))
                .execute()
                .await
                .context("querying chunk file paths")?;

            while let Some(batch) = stream
                .try_next()
                .await
                .context("reading chunk file paths")?
            {
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
        })
        .await
    }

    pub async fn ensure_fts_index(&self) -> Result<()> {
        let table = self.table.clone();
        run_on_lance_runtime(async move {
            let result = table
                .create_index(&["fts_text"], Index::FTS(FtsIndexBuilder::default()))
                .execute()
                .await;
            if let Err(err) = result {
                let message = err.to_string();
                if !message.to_lowercase().contains("already") {
                    return Err(err).context("creating fts_text index");
                }
            }
            Ok(())
        })
        .await
    }

    pub async fn vector_search(
        &self,
        vector: &[f32],
        k: usize,
        view_filter: Option<&str>,
    ) -> Result<Vec<ChunkRow>> {
        let table = self.table.clone();
        let vector = vector.to_vec();
        let view_filter = view_filter.map(ToOwned::to_owned);
        run_on_lance_runtime(async move {
            let mut query = table
                .query()
                .nearest_to(vector)
                .context("building vector search query")?
                .select(Select::columns(ROW_COLUMNS))
                .limit(k);
            if let Some(view) = view_filter {
                query = query.only_if(format!("view = {}", sql_quote(&view)));
            }
            let stream = query.execute().await.context("executing vector search")?;
            collect_chunk_rows(stream).await
        })
        .await
    }

    pub async fn fts_search(
        &self,
        query_text: &str,
        k: usize,
        view_filter: Option<&str>,
    ) -> Result<Vec<ChunkRow>> {
        let table = self.table.clone();
        let query_text = query_text.to_string();
        let view_filter = view_filter.map(ToOwned::to_owned);
        run_on_lance_runtime(async move {
            let mut query = table
                .query()
                .full_text_search(FullTextSearchQuery::new(query_text))
                .select(Select::columns(ROW_COLUMNS))
                .limit(k);
            if let Some(view) = view_filter {
                query = query.only_if(format!("view = {}", sql_quote(&view)));
            }
            let stream = query.execute().await.context("executing fts search")?;
            collect_chunk_rows(stream).await
        })
        .await
    }

    pub async fn get_canonical_by_node_ids(
        &self,
        node_ids: &[String],
    ) -> Result<HashMap<String, ChunkRow>> {
        if node_ids.is_empty() {
            return Ok(HashMap::default());
        }
        let table = self.table.clone();
        let node_ids = node_ids.to_vec();
        run_on_lance_runtime(async move {
            let filter = format!(
                "view = 'code' AND node_id IN ({})",
                node_ids
                    .iter()
                    .map(|id| sql_quote(id))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            let stream = table
                .query()
                .select(Select::columns(ROW_COLUMNS))
                .only_if(filter)
                .limit(node_ids.len())
                .execute()
                .await
                .context("querying canonical chunk rows")?;
            let rows = collect_chunk_rows(stream).await?;
            Ok(rows
                .into_iter()
                .map(|row| (row.node_id.clone(), row))
                .collect())
        })
        .await
    }

    pub async fn parent_chain(&self, chunk: &ChunkRow) -> Result<Vec<ChunkRow>> {
        let table = self.table.clone();
        let mut parent_id = chunk.parent_id.clone();
        run_on_lance_runtime(async move {
            let mut ancestors = Vec::new();
            while let Some(id) = parent_id {
                let stream = table
                    .query()
                    .select(Select::columns(ROW_COLUMNS))
                    .only_if(format!("id = {}", sql_quote(&id)))
                    .limit(1)
                    .execute()
                    .await
                    .context("querying parent chunk row")?;
                let mut rows = collect_chunk_rows(stream).await?;
                let Some(parent) = rows.pop() else {
                    break;
                };
                parent_id = parent.parent_id.clone();
                ancestors.push(parent);
            }
            ancestors.reverse();
            Ok(ancestors)
        })
        .await
    }

    pub async fn siblings_of(&self, chunk: &ChunkRow) -> Result<Vec<ChunkRow>> {
        let Some(parent_id) = chunk.parent_id.clone() else {
            return Ok(Vec::new());
        };
        let table = self.table.clone();
        let chunk_id = chunk.id.clone();
        run_on_lance_runtime(async move {
            let filter = format!(
                "view = 'code' AND parent_id = {} AND id != {}",
                sql_quote(&parent_id),
                sql_quote(&chunk_id)
            );
            let stream = table
                .query()
                .select(Select::columns(ROW_COLUMNS))
                .only_if(filter)
                .execute()
                .await
                .context("querying sibling chunk rows")?;
            let mut rows = collect_chunk_rows(stream).await?;
            rows.sort_by_key(|row| row.line_start);
            Ok(rows)
        })
        .await
    }

    // ------------------------------------------------------------------
    // Private helpers
    // ------------------------------------------------------------------

    async fn open_inner(data_dir: PathBuf, embedding_dim: usize) -> Result<Self> {
        std::fs::create_dir_all(&data_dir)
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
}

async fn run_on_lance_runtime<T, F>(future: F) -> Result<T>
where
    T: Send + 'static,
    F: Future<Output = Result<T>> + Send + 'static,
{
    LANCE_RUNTIME
        .spawn(future)
        .await
        .context("joining LanceDB runtime task")?
}

async fn count_rows_table(table: &Table) -> Result<u64> {
    let count = table.count_rows(None).await.context("counting rows")?;
    Ok(count as u64)
}

async fn delete_by_ids(table: &Table, chunks: &[Chunk]) -> Result<()> {
    if chunks.is_empty() {
        return Ok(());
    }
    let in_clause = chunks
        .iter()
        .map(|c| format!("'{}'", c.id.replace('\'', "''")))
        .collect::<Vec<_>>()
        .join(", ");
    // Missing ids mean the row set was already invalidated by another path.
    let _ = table.delete(&format!("id IN ({in_clause})")).await;
    Ok(())
}

async fn collect_chunk_rows<S>(mut stream: S) -> Result<Vec<ChunkRow>>
where
    S: futures::TryStream<Ok = RecordBatch> + Unpin,
    S::Error: std::error::Error + Send + Sync + 'static,
{
    let mut rows = Vec::new();
    while let Some(batch) = stream.try_next().await.context("reading chunk rows")? {
        rows.extend(chunk_rows_from_batch(&batch)?);
    }
    Ok(rows)
}

fn chunk_rows_from_batch(batch: &RecordBatch) -> Result<Vec<ChunkRow>> {
    let mut rows = Vec::with_capacity(batch.num_rows());
    for row in 0..batch.num_rows() {
        rows.push(ChunkRow {
            id: string_value(batch, "id", row)?,
            node_id: string_value(batch, "node_id", row)?,
            view: string_value(batch, "view", row)?,
            parent_id: optional_string_value(batch, "parent_id", row)?,
            file_path: string_value(batch, "file_path", row)?,
            language_id: string_value(batch, "language_id", row)?,
            kind: string_value(batch, "kind", row)?,
            name: optional_string_value(batch, "name", row)?,
            byte_start: i64_value(batch, "byte_start", row)?,
            byte_end: i64_value(batch, "byte_end", row)?,
            line_start: i32_value(batch, "line_start", row)?,
            line_end: i32_value(batch, "line_end", row)?,
            imports: string_value(batch, "imports", row)?,
            signature: string_value(batch, "signature", row)?,
            breadcrumb: string_value(batch, "breadcrumb", row)?,
            code_text: string_value(batch, "code_text", row)?,
            doc_text: string_value(batch, "doc_text", row)?,
            referenced_symbols: list_string_value(batch, "referenced_symbols", row)?,
            score: score_value(batch, row),
        });
    }
    Ok(rows)
}

fn string_array<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a StringArray> {
    batch
        .column_by_name(name)
        .with_context(|| format!("missing {name} column"))?
        .as_any()
        .downcast_ref::<StringArray>()
        .with_context(|| format!("{name} column should be Utf8"))
}

fn string_value(batch: &RecordBatch, name: &str, row: usize) -> Result<String> {
    let array = string_array(batch, name)?;
    if array.is_null(row) {
        Ok(String::new())
    } else {
        Ok(array.value(row).to_string())
    }
}

fn optional_string_value(batch: &RecordBatch, name: &str, row: usize) -> Result<Option<String>> {
    let array = string_array(batch, name)?;
    if array.is_null(row) {
        Ok(None)
    } else {
        Ok(Some(array.value(row).to_string()))
    }
}

fn i64_value(batch: &RecordBatch, name: &str, row: usize) -> Result<i64> {
    batch
        .column_by_name(name)
        .with_context(|| format!("missing {name} column"))?
        .as_any()
        .downcast_ref::<Int64Array>()
        .with_context(|| format!("{name} column should be Int64"))
        .map(|array| array.value(row))
}

fn i32_value(batch: &RecordBatch, name: &str, row: usize) -> Result<i32> {
    batch
        .column_by_name(name)
        .with_context(|| format!("missing {name} column"))?
        .as_any()
        .downcast_ref::<Int32Array>()
        .with_context(|| format!("{name} column should be Int32"))
        .map(|array| array.value(row))
}

fn list_string_value(batch: &RecordBatch, name: &str, row: usize) -> Result<Vec<String>> {
    let array = batch
        .column_by_name(name)
        .with_context(|| format!("missing {name} column"))?
        .as_any()
        .downcast_ref::<ListArray>()
        .with_context(|| format!("{name} column should be List<Utf8>"))?;

    if array.is_null(row) {
        return Ok(Vec::new());
    }

    let values = array.value(row);
    let values = values
        .as_any()
        .downcast_ref::<StringArray>()
        .context("referenced_symbols values should be Utf8")?;
    let mut out = Vec::new();
    for i in 0..values.len() {
        if !values.is_null(i) {
            out.push(values.value(i).to_string());
        }
    }
    Ok(out)
}

fn score_value(batch: &RecordBatch, row: usize) -> f32 {
    for name in ["_distance", "_score"] {
        let Some(column) = batch.column_by_name(name) else {
            continue;
        };
        if let Some(array) = column.as_any().downcast_ref::<Float32Array>() {
            if !array.is_null(row) {
                return array.value(row);
            }
        }
        if let Some(array) = column.as_any().downcast_ref::<Float64Array>() {
            if !array.is_null(row) {
                return array.value(row) as f32;
            }
        }
    }
    0.0
}

fn sql_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
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

    let fts_texts: StringArray = chunks.iter().map(|c| Some(build_fts_text(c))).collect();
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

    let mut builder = FixedSizeListBuilder::new(Float32Builder::new(), embedding_dim as i32);

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
