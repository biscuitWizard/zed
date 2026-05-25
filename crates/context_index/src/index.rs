use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use collections::HashMap;
use gpui::{Context, Entity, EventEmitter, Subscription, Task};
use project::worktree_store::{WorktreeStore, WorktreeStoreEvent};
use settings::{Settings as _, SettingsStore};
use worktree::{PathChange, Snapshot, WorktreeId};

use crate::{
    hasher::{self, FileHash},
    settings::ContextIndexSettings,
    stats::ContextIndexStats,
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

pub struct ContextIndex {
    fs: Arc<dyn fs::Fs>,
    worktree_store: Entity<WorktreeStore>,
    index: HashMap<(WorktreeId, Arc<str>), FileEntry>,
    stats: ContextIndexStats,
    enabled: bool,
    _subscriptions: Vec<Subscription>,
    _pending_scan: Option<Task<()>>,
    _rehash_task: Option<Task<()>>,
    pending_rehash_paths: Vec<(WorktreeId, Arc<str>, PathBuf)>,
    rehash_running: bool,
}

impl ContextIndex {
    pub fn new(
        fs: Arc<dyn fs::Fs>,
        worktree_store: Entity<WorktreeStore>,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut subscriptions: Vec<Subscription> = vec![
            cx.observe_global::<SettingsStore>(Self::on_settings_changed),
        ];

        if enabled {
            subscriptions.push(cx.subscribe(&worktree_store, Self::on_worktree_store_event));
        }

        Self {
            fs,
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
        }
    }

    pub fn stats(&self) -> &ContextIndexStats {
        &self.stats
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn set_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.enabled == enabled {
            return;
        }
        self.enabled = enabled;
        self.stats.enabled = enabled;

        if enabled {
            self._subscriptions.push(
                cx.subscribe(&self.worktree_store, Self::on_worktree_store_event),
            );
            self.start_full_scan(cx);
            log::info!("[context_index] enabled, starting full scan");
        } else {
            self._subscriptions.retain(|_| false);
            self._subscriptions.push(
                cx.observe_global::<SettingsStore>(Self::on_settings_changed),
            );
            self.index.clear();
            self._pending_scan = None;
            self._rehash_task = None;
            self.pending_rehash_paths.clear();
            self.rehash_running = false;
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
        self._pending_scan = None;
        self._rehash_task = None;
        self.pending_rehash_paths.clear();
        self.rehash_running = false;

        if self.enabled {
            self.start_full_scan(cx);
        }
        cx.emit(ContextIndexEvent::StatsUpdated);
    }

    pub fn file_count(&self) -> usize {
        self.index.len()
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
                let worktree = worktree_store.worktrees().find(|wt| wt.read(cx).id() == wt_id);
                let Some(worktree) = worktree else { return };
                let snapshot = worktree.read(cx).snapshot();
                let abs_root = snapshot.abs_path().to_path_buf();

                let mut paths_to_rehash: Vec<(WorktreeId, Arc<str>, PathBuf)> = Vec::new();
                let mut paths_to_remove: Vec<(WorktreeId, Arc<str>)> = Vec::new();

                for (path, _entry_id, change) in changes.iter() {
                    let rel_str: Arc<str> = path.as_unix_str().into();
                    match change {
                        PathChange::Removed => {
                            log::info!(
                                "[context_index] file deleted: {}",
                                path.as_unix_str()
                            );
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

                if !paths_to_rehash.is_empty() {
                    self.schedule_rehash(paths_to_rehash, cx);
                }

                cx.emit(ContextIndexEvent::StatsUpdated);
            }
            WorktreeStoreEvent::WorktreeRemoved(_entity_id, wt_id) => {
                self.index.retain(|(id, _), _| *id != *wt_id);
                self.stats.files_indexed = self.index.len() as u64;
                log::info!("[context_index] worktree removed: {:?}", wt_id);
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
            cx.background_executor()
                .timer(DEBOUNCE_DURATION)
                .await;

            loop {
                let paths_to_hash = this
                    .update(cx, |this, _cx| std::mem::take(&mut this.pending_rehash_paths))
                    .unwrap_or_default();

                if paths_to_hash.is_empty() {
                    let _ = this.update(cx, |this, _cx| {
                        this.rehash_running = false;
                        this._rehash_task = None;
                    });
                    return;
                }

                let mut results: Vec<(WorktreeId, Arc<str>, Option<FileEntry>)> = Vec::new();
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
                        }
                        Ok(None) => {
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
                    log::info!(
                        "[context_index] indexed {} files",
                        this.stats.files_indexed
                    );
                    cx.emit(ContextIndexEvent::StatsUpdated);
                });
            }
        }));
    }
}
