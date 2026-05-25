use context_index::chunker::types::{Chunk, ChunkView};
use context_index::store::LanceStore;

fn make_chunk(id_suffix: &str, file_path: &str) -> Chunk {
    let id = format!("chunk-{id_suffix}");
    let node_id = format!("node-{id_suffix}");
    Chunk {
        id,
        node_id,
        view: ChunkView::Code,
        parent_id: None,
        file_path: file_path.to_string(),
        language_id: "rust".to_string(),
        kind: "file".to_string(),
        name: None,
        byte_start: 0,
        byte_end: 100,
        line_start: 0,
        line_end: 5,
        imports: String::new(),
        signature: String::new(),
        breadcrumb: file_path.to_string(),
        code_text: "fn main() { println!(\"hello\"); }".to_string(),
        doc_text: String::new(),
        embed_text: format!("# {file_path}\nfn main() {{}}"),
        embed_text_sha: format!("sha-{id_suffix}"),
        referenced_symbols: Vec::new(),
    }
}

#[tokio::test]
async fn test_lance_store_open_and_upsert() {
    let dir = tempfile::TempDir::new().unwrap();
    let store = LanceStore::open(dir.path(), 2560).await.unwrap();

    assert_eq!(store.count_rows().await.unwrap(), 0);

    let chunks = vec![
        make_chunk("1", "src/a.rs"),
        make_chunk("2", "src/a.rs"),
        make_chunk("3", "src/b.rs"),
    ];

    store.upsert_chunks(&chunks).await.unwrap();
    assert_eq!(store.count_rows().await.unwrap(), 3);
}

#[tokio::test]
async fn test_lance_store_delete_by_file_paths() {
    let dir = tempfile::TempDir::new().unwrap();
    let store = LanceStore::open(dir.path(), 2560).await.unwrap();

    let chunks = vec![
        make_chunk("1", "src/a.rs"),
        make_chunk("2", "src/a.rs"),
        make_chunk("3", "src/b.rs"),
    ];

    store.upsert_chunks(&chunks).await.unwrap();
    assert_eq!(store.count_rows().await.unwrap(), 3);

    let deleted = store.delete_by_file_paths(&["src/a.rs"]).await.unwrap();
    assert_eq!(deleted, 2);
    assert_eq!(store.count_rows().await.unwrap(), 1);
}

#[tokio::test]
async fn test_lance_store_stats_counts_rows_and_files() {
    let dir = tempfile::TempDir::new().unwrap();
    let store = LanceStore::open(dir.path(), 2560).await.unwrap();

    let chunks = vec![
        make_chunk("1", "src/a.rs"),
        make_chunk("2", "src/a.rs"),
        make_chunk("3", "src/b.rs"),
    ];

    store.upsert_chunks(&chunks).await.unwrap();

    let stats = store.stats().await.unwrap();
    assert_eq!(stats.total_rows, 3);
    assert_eq!(stats.files, 2);
}

#[tokio::test]
async fn test_lance_store_drop_and_recreate() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut store = LanceStore::open(dir.path(), 2560).await.unwrap();

    let chunks = vec![make_chunk("1", "a.rs"), make_chunk("2", "b.rs")];
    store.upsert_chunks(&chunks).await.unwrap();
    assert_eq!(store.count_rows().await.unwrap(), 2);

    store.drop_and_recreate(2560).await.unwrap();
    assert_eq!(store.count_rows().await.unwrap(), 0);
}

#[tokio::test]
async fn test_lance_store_delete_all() {
    let dir = tempfile::TempDir::new().unwrap();
    let store = LanceStore::open(dir.path(), 2560).await.unwrap();

    let chunks = vec![make_chunk("1", "a.rs"), make_chunk("2", "b.rs")];
    store.upsert_chunks(&chunks).await.unwrap();
    assert_eq!(store.count_rows().await.unwrap(), 2);

    let deleted = store.delete_all().await.unwrap();
    assert_eq!(deleted, 2);
    assert_eq!(store.count_rows().await.unwrap(), 0);
}

#[tokio::test]
async fn test_lance_store_upsert_replaces_existing() {
    let dir = tempfile::TempDir::new().unwrap();
    let store = LanceStore::open(dir.path(), 2560).await.unwrap();

    let c1 = make_chunk("1", "a.rs");
    store.upsert_chunks(&[c1.clone()]).await.unwrap();
    assert_eq!(store.count_rows().await.unwrap(), 1);

    // Upsert the same id again
    store.upsert_chunks(&[c1]).await.unwrap();
    assert_eq!(store.count_rows().await.unwrap(), 1, "should not duplicate");
}

#[tokio::test]
async fn test_lance_store_reopen_existing() {
    let dir = tempfile::TempDir::new().unwrap();

    {
        let store = LanceStore::open(dir.path(), 2560).await.unwrap();
        store
            .upsert_chunks(&[make_chunk("1", "a.rs")])
            .await
            .unwrap();
        assert_eq!(store.count_rows().await.unwrap(), 1);
    }

    // Reopen — should see existing data
    let store2 = LanceStore::open(dir.path(), 2560).await.unwrap();
    assert_eq!(store2.count_rows().await.unwrap(), 1);
}
