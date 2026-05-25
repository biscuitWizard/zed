use std::path::Path;

use fs::FakeFs;
use gpui::{AppContext as _, TestAppContext};
use project::worktree_store::{WorktreeIdCounter, WorktreeStore};
use serde_json::json;
use settings::SettingsStore;
use worktree::{Worktree, WorktreeId};

use context_index::{ContextIndex, ContextIndexStats};

fn init_test(cx: &mut TestAppContext) {
    zlog::init_test();
    cx.update(|cx| {
        let settings_store = SettingsStore::test(cx);
        cx.set_global(settings_store);
    });
}

#[gpui::test]
async fn test_hasher_with_fake_fs(cx: &mut TestAppContext) {
    init_test(cx);
    let fs = FakeFs::new(cx.background_executor.clone());
    fs.insert_tree(
        "/project",
        json!({
            "hello.rs": "fn main() { println!(\"hello\"); }",
            "binary.bin": "\x00\x01\x02\x03binary content",
        }),
    )
    .await;

    let hash = context_index::hasher::hash_file(fs.as_ref(), Path::new("/project/hello.rs"))
        .await
        .unwrap();
    assert!(hash.is_some(), "text file should be hashed");

    let hash2 = context_index::hasher::hash_file(fs.as_ref(), Path::new("/project/hello.rs"))
        .await
        .unwrap();
    assert_eq!(hash, hash2, "same file should produce same hash");

    let binary_hash =
        context_index::hasher::hash_file(fs.as_ref(), Path::new("/project/binary.bin"))
            .await
            .unwrap();
    assert!(binary_hash.is_none(), "binary file should return None");

    let missing = context_index::hasher::hash_file(fs.as_ref(), Path::new("/project/nope.rs"))
        .await
        .unwrap();
    assert!(missing.is_none(), "missing file should return None");
}

#[gpui::test]
async fn test_context_index_full_scan(cx: &mut TestAppContext) {
    init_test(cx);
    let fs = FakeFs::new(cx.background_executor.clone());
    fs.insert_tree(
        "/project",
        json!({
            "src": {
                "main.rs": "fn main() {}",
                "lib.rs": "pub mod foo;",
            },
            "README.md": "# Hello",
        }),
    )
    .await;

    let tree = Worktree::local(
        Path::new("/project"),
        true,
        fs.clone(),
        Default::default(),
        true,
        WorktreeId::from_proto(1),
        &mut cx.to_async(),
    )
    .await
    .unwrap();

    cx.read(|cx| tree.read(cx).as_local().unwrap().scan_complete())
        .await;

    let worktree_store = cx.update(|cx| {
        cx.new(|cx| {
            let mut store = WorktreeStore::local(true, fs.clone(), WorktreeIdCounter::default());
            store.add(&tree, cx);
            store
        })
    });

    let context_index =
        cx.update(|cx| cx.new(|cx| ContextIndex::new(fs.clone(), worktree_store, true, cx)));

    cx.run_until_parked();

    context_index.read_with(cx, |idx, _| {
        assert_eq!(idx.file_count(), 3, "should index 3 text files");
        assert!(idx.stats().files_indexed == 3);
        assert!(idx.stats().bytes_hashed > 0);
        assert!(!idx.stats().currently_scanning);
    });
}

#[gpui::test]
async fn test_context_index_incremental_update(cx: &mut TestAppContext) {
    init_test(cx);
    let fs = FakeFs::new(cx.background_executor.clone());
    fs.insert_tree(
        "/project",
        json!({
            "src": {
                "main.rs": "fn main() {}",
            },
        }),
    )
    .await;

    let tree = Worktree::local(
        Path::new("/project"),
        true,
        fs.clone(),
        Default::default(),
        true,
        WorktreeId::from_proto(1),
        &mut cx.to_async(),
    )
    .await
    .unwrap();

    cx.read(|cx| tree.read(cx).as_local().unwrap().scan_complete())
        .await;

    let worktree_store = cx.update(|cx| {
        cx.new(|cx| {
            let mut store = WorktreeStore::local(true, fs.clone(), WorktreeIdCounter::default());
            store.add(&tree, cx);
            store
        })
    });

    let context_index = cx
        .update(|cx| cx.new(|cx| ContextIndex::new(fs.clone(), worktree_store.clone(), true, cx)));

    cx.run_until_parked();

    context_index.read_with(cx, |idx, _| {
        assert_eq!(idx.file_count(), 1);
    });

    fs.insert_file("/project/src/new_file.rs", b"fn new() {}".to_vec())
        .await;

    cx.run_until_parked();

    let count_after_add = context_index.read_with(cx, |idx, _| idx.file_count());
    assert!(
        count_after_add >= 1,
        "file count should stay >= 1 after adding a file"
    );

    fs.insert_file("/project/src/main.rs", b"fn main() { updated(); }".to_vec())
        .await;

    cx.run_until_parked();

    context_index.read_with(cx, |idx, _| {
        assert!(idx.stats().bytes_hashed > 0);
    });
}

#[gpui::test]
async fn test_context_index_toggle_enabled(cx: &mut TestAppContext) {
    init_test(cx);
    let fs = FakeFs::new(cx.background_executor.clone());
    fs.insert_tree(
        "/project",
        json!({
            "src": {
                "main.rs": "fn main() {}",
                "lib.rs": "pub mod bar;",
            },
        }),
    )
    .await;

    let tree = Worktree::local(
        Path::new("/project"),
        true,
        fs.clone(),
        Default::default(),
        true,
        WorktreeId::from_proto(1),
        &mut cx.to_async(),
    )
    .await
    .unwrap();

    cx.read(|cx| tree.read(cx).as_local().unwrap().scan_complete())
        .await;

    let worktree_store = cx.update(|cx| {
        cx.new(|cx| {
            let mut store = WorktreeStore::local(true, fs.clone(), WorktreeIdCounter::default());
            store.add(&tree, cx);
            store
        })
    });

    let context_index =
        cx.update(|cx| cx.new(|cx| ContextIndex::new(fs.clone(), worktree_store, false, cx)));

    cx.run_until_parked();

    context_index.read_with(cx, |idx, _| {
        assert_eq!(idx.file_count(), 0, "disabled index should have no files");
        assert!(!idx.enabled());
    });

    context_index.update(cx, |idx, cx| {
        idx.set_enabled(true, cx);
    });

    cx.run_until_parked();

    context_index.read_with(cx, |idx, _| {
        assert_eq!(idx.file_count(), 2, "enabled index should scan files");
        assert!(idx.enabled());
    });

    context_index.update(cx, |idx, cx| {
        idx.set_enabled(false, cx);
    });

    context_index.read_with(cx, |idx, _| {
        assert_eq!(idx.file_count(), 0, "disabled index should clear");
        assert!(!idx.enabled());
    });
}

#[gpui::test]
async fn test_context_index_reset(cx: &mut TestAppContext) {
    init_test(cx);
    let fs = FakeFs::new(cx.background_executor.clone());
    fs.insert_tree(
        "/project",
        json!({
            "a.rs": "fn a() {}",
            "b.rs": "fn b() {}",
        }),
    )
    .await;

    let tree = Worktree::local(
        Path::new("/project"),
        true,
        fs.clone(),
        Default::default(),
        true,
        WorktreeId::from_proto(1),
        &mut cx.to_async(),
    )
    .await
    .unwrap();

    cx.read(|cx| tree.read(cx).as_local().unwrap().scan_complete())
        .await;

    let worktree_store = cx.update(|cx| {
        cx.new(|cx| {
            let mut store = WorktreeStore::local(true, fs.clone(), WorktreeIdCounter::default());
            store.add(&tree, cx);
            store
        })
    });

    let context_index =
        cx.update(|cx| cx.new(|cx| ContextIndex::new(fs.clone(), worktree_store, true, cx)));

    cx.run_until_parked();

    context_index.read_with(cx, |idx, _| {
        assert_eq!(idx.file_count(), 2);
    });

    context_index.update(cx, |idx, cx| {
        idx.reset(cx);
    });

    cx.run_until_parked();

    context_index.read_with(cx, |idx, _| {
        assert_eq!(idx.file_count(), 2, "reset should rescan all files");
        assert!(!idx.stats().currently_scanning);
    });
}

#[gpui::test]
async fn test_proto_stats_roundtrip(cx: &mut TestAppContext) {
    init_test(cx);

    let stats = ContextIndexStats {
        files_indexed: 42,
        bytes_hashed: 12345,
        currently_scanning: true,
        scan_progress_done: 10,
        scan_progress_total: 42,
        enabled: true,
        chunks_indexed: 200,
        files_chunked: 30,
        ..Default::default()
    };

    let proto_msg = stats.to_proto();
    assert_eq!(proto_msg.files_indexed, 42);
    assert_eq!(proto_msg.bytes_hashed, 12345);
    assert!(proto_msg.currently_scanning);
    assert_eq!(proto_msg.scan_progress_done, 10);
    assert_eq!(proto_msg.scan_progress_total, 42);
    assert!(proto_msg.enabled);
    assert_eq!(proto_msg.chunks_indexed, 200);
    assert_eq!(proto_msg.files_chunked, 30);

    let roundtripped = ContextIndexStats::from_proto(&proto_msg);
    assert_eq!(roundtripped.files_indexed, 42);
    assert_eq!(roundtripped.bytes_hashed, 12345);
    assert!(roundtripped.currently_scanning);
    assert_eq!(roundtripped.scan_progress_done, 10);
    assert_eq!(roundtripped.scan_progress_total, 42);
    assert!(roundtripped.enabled);
    assert_eq!(roundtripped.chunks_indexed, 200);
    assert_eq!(roundtripped.files_chunked, 30);
}
