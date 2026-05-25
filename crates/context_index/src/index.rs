use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use collections::{HashMap, HashSet};
use gpui::{Context, Entity, EventEmitter, Subscription, Task};
use http_client::HttpClient;
use project::worktree_store::{WorktreeStore, WorktreeStoreEvent};
use settings::{Settings as _, SettingsStore};
use sha2::{Digest, Sha256};
use worktree::{PathChange, Snapshot, WorktreeId};

use crate::{
    api_keys::{embed_api_key_state, embed_api_url},
    chunker::{self, ChunkBudget},
    embed::EmbedClient,
    hasher::{self, FileHash},
    settings::ContextIndexSettings,
    stats::ContextIndexStats,
    store::LanceStore,
};

const DEBOUNCE_DURATION: Duration = Duration::from_millis(200);

#[derive(Clone, Debug)]
pub struct FileEntry {
    pub sha: FileHash,
    pub size: u64,
    pub hashed_at: Instant,
}

pub enum ContextIndexEvent {
    StatsUpdated,
}

impl EventEmitter<ContextIndexEvent> for ContextIndex {}

/// A supported file whose hash changed and whose chunk rows need refreshing.
struct PendingChunk {
    /// Project-relative path stored in LanceDB's `file_path` column.
    rel_path: Arc<str>,
    /// Absolute path used only while reading the file contents.
    abs_path: PathBuf,
    /// Lowercase extension used to select the chunker.
    extension: String,
}

#[derive(Clone, Copy, Debug)]
enum ChunkInvalidationReason {
    FullRefresh,
    Removed,
    ReplacedByUnchunkableContent,
    ReplacedByFreshChunks,
}

impl ChunkInvalidationReason {
    fn as_str(&self) -> &'static str {
        match self {
            Self::FullRefresh => "full refresh",
            Self::Removed => "file removed",
            Self::ReplacedByUnchunkableContent => "file no longer chunkable",
            Self::ReplacedByFreshChunks => "file refreshed",
        }
    }
}

#[derive(Clone, Debug)]
enum ChunkInvalidation {
    /// Clear every chunk row before replaying a full project snapshot.
    All { reason: ChunkInvalidationReason },
    /// Remove chunk rows for one project-relative file path.
    File {
        rel_path: String,
        reason: ChunkInvalidationReason,
    },
}

pub struct ContextIndex {
    fs: Arc<dyn fs::Fs>,
    http_client: Arc<dyn HttpClient>,
    worktree_store: Entity<WorktreeStore>,
    index: HashMap<(WorktreeId, Arc<str>), FileEntry>,
    stats: ContextIndexStats,
    enabled: bool,
    _subscriptions: Vec<Subscription>,
    _pending_scan: Option<Task<()>>,
    _rehash_task: Option<Task<()>>,
    pending_rehash_paths: Vec<(WorktreeId, Arc<str>, PathBuf)>,
    rehash_running: bool,

    /// Lazily opened per-project LanceDB store.
    store: Option<Arc<LanceStore>>,
    /// Per-project LanceDB directory, derived from the first worktree root.
    store_dir: Option<PathBuf>,
    /// Background task that drains chunk writes and invalidations.
    _chunk_task: Option<Task<()>>,
    /// Files waiting for chunk generation and persistence.
    pending_chunks: Vec<PendingChunk>,
    /// Prevents duplicate chunk-drain tasks from running concurrently.
    chunk_running: bool,
    /// Store rows that must be removed before any queued writes are applied.
    pending_chunk_invalidations: Vec<ChunkInvalidation>,
    /// Project-relative paths with current chunk rows in the store.
    chunked_files: HashSet<String>,
}

impl ContextIndex {
    pub fn new(
        fs: Arc<dyn fs::Fs>,
        http_client: Arc<dyn HttpClient>,
        worktree_store: Entity<WorktreeStore>,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut subscriptions: Vec<Subscription> =
            vec![cx.observe_global::<SettingsStore>(Self::on_settings_changed)];

        if enabled {
            subscriptions.push(cx.subscribe(&worktree_store, Self::on_worktree_store_event));
        }

        let mut this = Self {
            fs,
            http_client,
            worktree_store,
            index: HashMap::default(),
            stats: ContextIndexStats {
                enabled,
                ..Default::default()
            },
            enabled,
            _subscriptions: subscriptions,
            _pending_scan: None,
            _rehash_task: None,
            pending_rehash_paths: Vec::new(),
            rehash_running: false,
            store: None,
            store_dir: None,
            _chunk_task: None,
            pending_chunks: Vec::new(),
            chunk_running: false,
            pending_chunk_invalidations: Vec::new(),
            chunked_files: HashSet::default(),
        };

        if enabled {
            this.start_full_scan(cx);
        }

        this
    }

    pub fn stats(&self) -> &ContextIndexStats {
        &self.stats
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn store(&self) -> Option<&Arc<LanceStore>> {
        self.store.as_ref()
    }

    pub fn refresh_store_stats(&mut self, cx: &mut Context<Self>) {
        let Some(store) = self.store.clone() else {
            return;
        };

        cx.spawn(async move |this, cx| match store.stats().await {
            Ok(stats) => {
                let _ = this.update(cx, |this, cx| {
                    this.stats.chunks_indexed = stats.total_rows;
                    this.stats.files_chunked = stats.files;
                    cx.emit(ContextIndexEvent::StatsUpdated);
                });
            }
            Err(err) => {
                log::warn!("[context_index] failed to refresh store stats: {err}");
            }
        })
        .detach();
    }

    pub fn set_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.enabled == enabled {
            return;
        }
        self.enabled = enabled;
        self.stats.enabled = enabled;

        if enabled {
            self._subscriptions
                .push(cx.subscribe(&self.worktree_store, Self::on_worktree_store_event));
            self.start_full_scan(cx);
            log::info!("[context_index] enabled, starting full scan");
        } else {
            self._subscriptions.retain(|_| false);
            self._subscriptions
                .push(cx.observe_global::<SettingsStore>(Self::on_settings_changed));
            self.index.clear();
            self._pending_scan = None;
            self._rehash_task = None;
            self._chunk_task = None;
            self.pending_rehash_paths.clear();
            self.pending_chunks.clear();
            self.pending_chunk_invalidations.clear();
            self.chunked_files.clear();
            self.rehash_running = false;
            self.chunk_running = false;
            self.store = None;
            self.stats = ContextIndexStats {
                enabled: false,
                ..Default::default()
            };
            log::info!("[context_index] disabled, index cleared");
        }
        cx.emit(ContextIndexEvent::StatsUpdated);
    }

    fn on_settings_changed(&mut self, cx: &mut Context<Self>) {
        let new_enabled = ContextIndexSettings::get_global(cx).enabled;
        self.set_enabled(new_enabled, cx);
    }

    pub fn reset(&mut self, cx: &mut Context<Self>) {
        log::info!("[context_index] reset requested, clearing index and rescanning");
        self.index.clear();
        self.stats.files_indexed = 0;
        self.stats.bytes_hashed = 0;
        self.stats.chunks_indexed = 0;
        self.stats.files_chunked = 0;
        self._pending_scan = None;
        self._rehash_task = None;
        self._chunk_task = None;
        self.pending_rehash_paths.clear();
        self.pending_chunks.clear();
        self.pending_chunk_invalidations.clear();
        self.chunked_files.clear();
        self.rehash_running = false;
        self.chunk_running = false;

        // Replace the table because embedding width is fixed at creation time.
        if self.store.is_some() {
            let store_dir = self.store_dir.clone();
            let settings = ContextIndexSettings::get_global(cx);
            let dim = settings.embedding_dim as usize;
            self.store = None;
            cx.spawn(async move |this, cx| {
                if let Some(dir) = store_dir {
                    match LanceStore::open(&dir, dim).await {
                        Ok(mut new_store) => {
                            if let Err(e) = new_store.drop_and_recreate(dim).await {
                                log::warn!("[context_index] reset: drop_and_recreate failed: {e}");
                            }
                            let _ = this.update(cx, |this, _cx| {
                                this.store = Some(Arc::new(new_store));
                            });
                        }
                        Err(e) => {
                            log::warn!("[context_index] reset: failed to reopen store: {e}");
                        }
                    }
                }
            })
            .detach();
        }

        if self.enabled {
            self.start_full_scan(cx);
        }
        cx.emit(ContextIndexEvent::StatsUpdated);
    }

    pub fn file_count(&self) -> usize {
        self.index.len()
    }

    /// Compute the data directory for this project's LanceDB store.
    fn ensure_store_dir(&mut self, cx: &Context<Self>) -> Option<PathBuf> {
        if let Some(dir) = &self.store_dir {
            return Some(dir.clone());
        }

        let worktree_store = self.worktree_store.read(cx);
        let first_wt = worktree_store.worktrees().next()?;
        let abs_path = first_wt.read(cx).abs_path();
        let hash = {
            let mut hasher = Sha256::new();
            hasher.update(abs_path.as_os_str().as_encoded_bytes());
            let result = hasher.finalize();
            format!("{:x}", result)
        };
        let dir = paths::data_dir()
            .join("context_index")
            .join(&hash)
            .join("lancedb");
        self.store_dir = Some(dir.clone());
        Some(dir)
    }

    fn start_full_scan(&mut self, cx: &mut Context<Self>) {
        let worktree_store = self.worktree_store.read(cx);
        let snapshots: Vec<Snapshot> = worktree_store
            .worktrees()
            .map(|wt| wt.read(cx).snapshot())
            .collect();

        let total_files: u64 = snapshots
            .iter()
            .map(|s| s.files(false, 0).count() as u64)
            .sum();

        self.stats.currently_scanning = true;
        self.stats.scan_progress_done = 0;
        self.stats.scan_progress_total = total_files;
        cx.emit(ContextIndexEvent::StatsUpdated);

        let mut file_paths: Vec<(WorktreeId, Arc<str>, PathBuf)> = Vec::new();
        for snapshot in &snapshots {
            let wt_id = snapshot.id();
            let abs_root = snapshot.abs_path().to_path_buf();
            for entry in snapshot.files(false, 0) {
                if entry.is_ignored {
                    continue;
                }
                let rel_str: Arc<str> = entry.path.as_unix_str().into();
                let abs = abs_root.join(entry.path.as_std_path());
                file_paths.push((wt_id, rel_str, abs));
            }
        }

        let fs = self.fs.clone();
        let file_count = file_paths.len();

        log::info!(
            "[context_index] full scan started: {} files across {} worktrees",
            file_count,
            snapshots.len()
        );

        self._pending_scan = Some(cx.spawn(async move |this, cx| {
            let scan_start = Instant::now();
            let mut hashed_count: u64 = 0;
            let mut total_bytes: u64 = 0;
            let mut results: Vec<(WorktreeId, Arc<str>, FileEntry)> = Vec::new();
            let mut chunk_queue: Vec<PendingChunk> = Vec::new();

            for (wt_id, rel_str, abs_path) in file_paths {
                match hasher::hash_file(fs.as_ref(), &abs_path).await {
                    Ok(Some(sha)) => {
                        let metadata = fs.metadata(&abs_path).await.ok().flatten();
                        let size = metadata.map(|m| m.len).unwrap_or(0);
                        total_bytes += size;
                        hashed_count += 1;

                        results.push((
                            wt_id,
                            rel_str.clone(),
                            FileEntry {
                                sha,
                                size,
                                hashed_at: Instant::now(),
                            },
                        ));

                        // Only supported extensions participate in chunk storage.
                        if let Some(ext) = extension_from_path(&abs_path) {
                            if chunker::language_id_for_extension(&ext).is_some() {
                                chunk_queue.push(PendingChunk {
                                    rel_path: rel_str.clone(),
                                    abs_path: abs_path.clone(),
                                    extension: ext,
                                });
                            }
                        }

                        if hashed_count <= 100 || hashed_count % 500 == 0 {
                            log::info!(
                                "[context_index] hashed {} ({} bytes) sha={}",
                                rel_str,
                                size,
                                &hasher::hash_to_hex(&sha)[..8]
                            );
                        }
                    }
                    Ok(None) => {}
                    Err(e) => {
                        log::warn!(
                            "[context_index] failed to hash {}: {}",
                            abs_path.display(),
                            e
                        );
                    }
                }

                if hashed_count % 50 == 0 {
                    let _ = this.update(cx, |this, cx| {
                        this.stats.scan_progress_done = hashed_count;
                        cx.emit(ContextIndexEvent::StatsUpdated);
                    });
                }
            }

            let elapsed = scan_start.elapsed();
            log::info!(
                "[context_index] full scan complete: {} files, {} bytes in {:.2}s",
                hashed_count,
                total_bytes,
                elapsed.as_secs_f64()
            );

            let _ = this.update(cx, |this, cx| {
                for (wt_id, rel_str, entry) in results {
                    this.index.insert((wt_id, rel_str), entry);
                }
                this.stats.files_indexed = this.index.len() as u64;
                this.stats.bytes_hashed = total_bytes;
                this.stats.last_scan_at = Some(Instant::now());
                this.stats.currently_scanning = false;
                this.stats.scan_progress_done = hashed_count;
                this._pending_scan = None;

                this.pending_chunk_invalidations
                    .push(ChunkInvalidation::All {
                        reason: ChunkInvalidationReason::FullRefresh,
                    });
                this.pending_chunks.extend(chunk_queue);
                this.maybe_schedule_chunk_processing(cx);

                cx.emit(ContextIndexEvent::StatsUpdated);
            });
        }));
    }

    fn on_worktree_store_event(
        &mut self,
        _worktree_store: Entity<WorktreeStore>,
        event: &WorktreeStoreEvent,
        cx: &mut Context<Self>,
    ) {
        if !self.enabled {
            return;
        }

        match event {
            WorktreeStoreEvent::WorktreeUpdatedEntries(worktree_id, changes) => {
                let wt_id = *worktree_id;
                if changes.is_empty() {
                    return;
                }

                self.stats.last_change_at = Some(Instant::now());

                let worktree_store = self.worktree_store.read(cx);
                let worktree = worktree_store
                    .worktrees()
                    .find(|wt| wt.read(cx).id() == wt_id);
                let Some(worktree) = worktree else { return };
                let snapshot = worktree.read(cx).snapshot();
                let abs_root = snapshot.abs_path().to_path_buf();

                let mut paths_to_rehash: Vec<(WorktreeId, Arc<str>, PathBuf)> = Vec::new();
                let mut paths_to_remove: Vec<(WorktreeId, Arc<str>)> = Vec::new();
                let mut chunk_invalidations: Vec<ChunkInvalidation> = Vec::new();

                for (path, _entry_id, change) in changes.iter() {
                    let rel_str: Arc<str> = path.as_unix_str().into();
                    match change {
                        PathChange::Removed => {
                            log::info!("[context_index] file deleted: {}", path.as_unix_str());
                            chunk_invalidations.push(ChunkInvalidation::File {
                                rel_path: rel_str.to_string(),
                                reason: ChunkInvalidationReason::Removed,
                            });
                            paths_to_remove.push((wt_id, rel_str));
                        }
                        PathChange::Added
                        | PathChange::Updated
                        | PathChange::AddedOrUpdated
                        | PathChange::Loaded => {
                            let entry = snapshot.entry_for_path(path);
                            if let Some(e) = entry {
                                if !e.is_dir() && !e.is_ignored {
                                    let abs = abs_root.join(path.as_std_path());
                                    paths_to_rehash.push((wt_id, rel_str, abs));
                                }
                            }
                        }
                    }
                }

                for key in paths_to_remove {
                    self.index.remove(&key);
                }
                self.stats.files_indexed = self.index.len() as u64;

                if !chunk_invalidations.is_empty() {
                    self.pending_chunk_invalidations.extend(chunk_invalidations);
                    self.maybe_schedule_chunk_processing(cx);
                }

                if !paths_to_rehash.is_empty() {
                    self.schedule_rehash(paths_to_rehash, cx);
                }

                cx.emit(ContextIndexEvent::StatsUpdated);
            }
            WorktreeStoreEvent::WorktreeRemoved(_entity_id, wt_id) => {
                self.index.retain(|(id, _), _| *id != *wt_id);
                self.stats.files_indexed = self.index.len() as u64;
                log::info!("[context_index] worktree removed: {:?}", wt_id);
                self.start_full_scan(cx);
                cx.emit(ContextIndexEvent::StatsUpdated);
            }
            WorktreeStoreEvent::WorktreeAdded(_worktree) => {
                log::info!("[context_index] worktree added");
            }
            _ => {}
        }
    }

    fn schedule_rehash(
        &mut self,
        paths: Vec<(WorktreeId, Arc<str>, PathBuf)>,
        cx: &mut Context<Self>,
    ) {
        self.pending_rehash_paths.extend(paths);

        if self.rehash_running {
            return;
        }

        self.rehash_running = true;
        let fs = self.fs.clone();

        self._rehash_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(DEBOUNCE_DURATION).await;

            loop {
                let paths_to_hash = this
                    .update(cx, |this, _cx| {
                        std::mem::take(&mut this.pending_rehash_paths)
                    })
                    .unwrap_or_default();

                if paths_to_hash.is_empty() {
                    let _ = this.update(cx, |this, _cx| {
                        this.rehash_running = false;
                        this._rehash_task = None;
                    });
                    return;
                }

                let mut results: Vec<(WorktreeId, Arc<str>, Option<FileEntry>)> = Vec::new();
                let mut chunk_queue: Vec<PendingChunk> = Vec::new();
                let mut chunk_invalidations: Vec<ChunkInvalidation> = Vec::new();

                for (wt_id, rel_str, abs_path) in &paths_to_hash {
                    match hasher::hash_file(fs.as_ref(), abs_path).await {
                        Ok(Some(sha)) => {
                            let metadata = fs.metadata(abs_path).await.ok().flatten();
                            let size = metadata.map(|m| m.len).unwrap_or(0);
                            results.push((
                                *wt_id,
                                rel_str.clone(),
                                Some(FileEntry {
                                    sha,
                                    size,
                                    hashed_at: Instant::now(),
                                }),
                            ));

                            if let Some(ext) = extension_from_path(abs_path) {
                                if chunker::language_id_for_extension(&ext).is_some() {
                                    chunk_queue.push(PendingChunk {
                                        rel_path: rel_str.clone(),
                                        abs_path: abs_path.clone(),
                                        extension: ext,
                                    });
                                }
                            }
                        }
                        Ok(None) => {
                            chunk_invalidations.push(ChunkInvalidation::File {
                                rel_path: rel_str.to_string(),
                                reason: ChunkInvalidationReason::ReplacedByUnchunkableContent,
                            });
                            results.push((*wt_id, rel_str.clone(), None));
                        }
                        Err(e) => {
                            log::warn!(
                                "[context_index] rehash failed for {}: {}",
                                abs_path.display(),
                                e
                            );
                        }
                    }
                }

                let _ = this.update(cx, |this, cx| {
                    for (wt_id, rel_str, entry) in results {
                        let key = (wt_id, rel_str.clone());
                        match entry {
                            Some(new_entry) => {
                                this.stats.bytes_hashed += new_entry.size;
                                this.index.insert(key, new_entry);
                            }
                            None => {
                                this.index.remove(&key);
                            }
                        }
                    }
                    this.stats.files_indexed = this.index.len() as u64;
                    this.stats.last_change_at = Some(Instant::now());

                    this.pending_chunk_invalidations.extend(chunk_invalidations);
                    this.pending_chunks.extend(chunk_queue);
                    this.maybe_schedule_chunk_processing(cx);

                    log::info!("[context_index] indexed {} files", this.stats.files_indexed);
                    cx.emit(ContextIndexEvent::StatsUpdated);
                });
            }
        }));
    }

    /// Start the chunk drain only when there is queued persistence work.
    fn maybe_schedule_chunk_processing(&mut self, cx: &mut Context<Self>) {
        if self.pending_chunks.is_empty() && self.pending_chunk_invalidations.is_empty() {
            return;
        }
        self.schedule_chunk_processing(cx);
    }

    /// Drains invalidations before writes so stale rows are removed even when
    /// the refreshed file no longer produces chunks.
    fn schedule_chunk_processing(&mut self, cx: &mut Context<Self>) {
        if self.chunk_running {
            return;
        }

        let store_dir = match self.ensure_store_dir(cx) {
            Some(d) => d,
            None => {
                log::warn!("[context_index] cannot determine store dir, skipping chunk processing");
                return;
            }
        };

        self.chunk_running = true;
        let fs = self.fs.clone();
        let http_client = self.http_client.clone();
        let store = self.store.clone();
        let settings = ContextIndexSettings::get_global(cx).clone();
        let embed_api_url = embed_api_url(cx).to_string();
        let embed_api_key = embed_api_key_state(cx)
            .read(cx)
            .key(&embed_api_url)
            .map(|key| key.to_string());
        let embedding_dim = settings.embedding_dim as usize;
        let embed_client = EmbedClient::new(
            http_client,
            embed_api_url,
            settings.embed_model.clone(),
            settings.query_instruction.clone(),
            embed_api_key,
        );
        let budget = ChunkBudget::from_token_counts_with_doc_view_min(
            settings.chunk_target_tokens,
            settings.chunk_max_tokens,
            settings.chunk_min_tokens,
            settings.chunk_doc_view_min_tokens,
        );

        self._chunk_task = Some(cx.spawn(async move |this, cx| {
            let store = match store {
                Some(s) => s,
                None => match open_store_safe(&store_dir, embedding_dim).await {
                    Some(s) => {
                        let arc = Arc::new(s);
                        let _ = this.update(cx, |this, _cx| {
                            this.store = Some(arc.clone());
                        });
                        arc
                    }
                    None => {
                        let _ = this.update(cx, |this, _cx| {
                            this.chunk_running = false;
                            this._chunk_task = None;
                        });
                        return;
                    }
                },
            };

            loop {
                let (chunks_to_process, invalidations) = this
                    .update(cx, |this, _cx| {
                        (
                            std::mem::take(&mut this.pending_chunks),
                            std::mem::take(&mut this.pending_chunk_invalidations),
                        )
                    })
                    .unwrap_or_default();

                if chunks_to_process.is_empty() && invalidations.is_empty() {
                    let _ = this.update(cx, |this, _cx| {
                        this.chunk_running = false;
                        this._chunk_task = None;
                    });
                    return;
                }

                let mut invalidated_all = false;
                let mut invalidated_files = HashSet::default();
                let mut invalidated_rows = 0u64;

                for invalidation in &invalidations {
                    match invalidation {
                        ChunkInvalidation::All { reason } => {
                            invalidated_all = true;
                            match store.delete_all().await {
                                Ok(deleted) => {
                                    invalidated_rows += deleted;
                                    if deleted > 0 {
                                        log::info!(
                                            "[context_index] invalidated all chunks ({}) — {deleted} rows",
                                            reason.as_str()
                                        );
                                    }
                                }
                                Err(e) => {
                                    log::warn!(
                                        "[context_index] failed to invalidate all chunks ({}): {e}",
                                        reason.as_str()
                                    );
                                }
                            }
                        }
                        ChunkInvalidation::File { rel_path, reason } if !invalidated_all => {
                            match store.delete_by_file_paths(&[rel_path.as_str()]).await {
                                Ok(deleted) => {
                                    invalidated_rows += deleted;
                                    invalidated_files.insert(rel_path.clone());
                                    if deleted > 0 {
                                        log::info!(
                                            "[context_index] invalidated {} chunk rows for {} ({})",
                                            deleted,
                                            rel_path,
                                            reason.as_str()
                                        );
                                    }
                                }
                                Err(e) => {
                                    log::warn!(
                                        "[context_index] failed to invalidate chunks for {} ({}): {e}",
                                        rel_path,
                                        reason.as_str()
                                    );
                                }
                            }
                        }
                        ChunkInvalidation::File { rel_path, .. } => {
                            invalidated_files.insert(rel_path.clone());
                        }
                    }
                }

                let mut total_new_chunks = 0u64;
                let mut ensured_fts = false;
                let mut refreshed_files = HashSet::default();

                for pending in &chunks_to_process {
                    let chunker = match chunker::chunker_for_extension(&pending.extension, budget) {
                        Some(c) => c,
                        None => continue,
                    };

                    let rel = pending.rel_path.as_ref();

                    let bytes = match fs.load_bytes(&pending.abs_path).await {
                        Ok(b) => b,
                        Err(e) => {
                            log::warn!(
                                "[context_index] failed to read {} for chunking: {e}",
                                pending.rel_path
                            );
                            continue;
                        }
                    };

                    let chunks = chunker.chunk_file(rel, &bytes);

                    match store.delete_by_file_paths(&[rel]).await {
                        Ok(deleted) => {
                            invalidated_rows += deleted;
                            invalidated_files.insert(rel.to_string());
                            if deleted > 0 {
                                log::info!(
                                    "[context_index] invalidated {} existing chunk rows for {} ({})",
                                    deleted,
                                    rel,
                                    ChunkInvalidationReason::ReplacedByFreshChunks.as_str()
                                );
                            }
                        }
                        Err(e) => {
                            log::warn!("[context_index] failed to invalidate chunks for {rel}: {e}");
                            continue;
                        }
                    }

                    if chunks.is_empty() {
                        continue;
                    }

                    let embed_texts = chunks
                        .iter()
                        .map(|chunk| chunk.embed_text.clone())
                        .collect::<Vec<_>>();
                    let upsert_result = match embed_client.embed_documents(&embed_texts).await {
                        Ok(vectors) => store.upsert_chunks_with_vectors(&chunks, &vectors).await,
                        Err(err) => {
                            log::warn!(
                                "[context_index] failed to embed chunks for {rel}, storing without vectors: {err}"
                            );
                            store.upsert_chunks(&chunks).await
                        }
                    };

                    match upsert_result {
                        Ok(()) => {
                            if !ensured_fts {
                                if let Err(err) = store.ensure_fts_index().await {
                                    log::warn!("[context_index] failed to ensure FTS index: {err}");
                                }
                                ensured_fts = true;
                            }
                            total_new_chunks += chunks.len() as u64;
                            refreshed_files.insert(rel.to_string());
                            if refreshed_files.len() <= 50 || refreshed_files.len() % 200 == 0 {
                                log::info!(
                                    "[context_index] chunked {} → {} chunks",
                                    rel,
                                    chunks.len()
                                );
                            }
                        }
                        Err(e) => {
                            log::warn!(
                                "[context_index] failed to upsert chunks for {}: {e}",
                                rel
                            );
                        }
                    }
                }

                if total_new_chunks > 0 || invalidated_rows > 0 || invalidated_all {
                    let store_stats = store.stats().await.ok();
                    let _ = this.update(cx, |this, cx| {
                        if invalidated_all {
                            this.chunked_files.clear();
                        }
                        for rel_path in invalidated_files {
                            this.chunked_files.remove(&rel_path);
                        }
                        for rel_path in refreshed_files {
                            this.chunked_files.insert(rel_path);
                        }
                        let (row_count, file_count) = if let Some(store_stats) = store_stats {
                            (store_stats.total_rows, store_stats.files)
                        } else {
                            (this.stats.chunks_indexed, this.chunked_files.len() as u64)
                        };
                        this.stats.chunks_indexed = row_count;
                        this.stats.files_chunked = file_count;
                        log::info!(
                            "[context_index] chunk pass done: +{} chunks, -{} stale rows, {} total rows",
                            total_new_chunks,
                            invalidated_rows,
                            row_count
                        );
                        cx.emit(ContextIndexEvent::StatsUpdated);
                    });
                }
            }
        }));
    }
}

fn extension_from_path(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_lowercase())
}

/// Open the LanceDB store on its dedicated Tokio runtime.
async fn open_store_safe(store_dir: &Path, embedding_dim: usize) -> Option<LanceStore> {
    match LanceStore::open(store_dir, embedding_dim).await {
        Ok(store) => Some(store),
        Err(e) => {
            log::error!("[context_index] failed to open LanceDB store: {e}");
            None
        }
    }
}
