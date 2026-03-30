use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use dashmap::DashMap;
use notify::event::EventKind;
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use parking_lot::{Mutex, RwLock};
use tokio::task::JoinHandle;
use tokio::time::{Instant, sleep_until};
use tracing::warn;

use crate::error::{AgentError, AgentResult};

const DEFAULT_DEBOUNCE: Duration = Duration::from_millis(200);
const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Input passed to hooks for each watched file.
#[derive(Debug, Clone)]
pub struct WatchedFileEntry {
    pub name: String,
    pub content: Option<String>,
}

/// Event triggered when watched files change
#[derive(Debug, Clone)]
pub enum WatchEvent {
    /// Initial load of all files
    Initial { files: Vec<WatchedFileEntry> },
    /// One or more files changed
    Changed {
        files: Vec<WatchedFileEntry>,
        changed_names: HashSet<String>,
    },
}

/// Hook called when watch events occur
pub trait WatchEventHook: Send + Sync {
    fn on_event(&self, event: &WatchEvent);
}

// Blanket implementation for closures
impl<F> WatchEventHook for F
where
    F: Fn(&WatchEvent) + Send + Sync,
{
    fn on_event(&self, event: &WatchEvent) {
        self(event)
    }
}
/// Default hook that renders bootstrap-style sections and stores in cache.
#[derive(Clone)]
pub struct BootstrapRenderHook {
    rendered: Arc<RwLock<String>>,
}

impl BootstrapRenderHook {
    pub fn new(_watcher: &FileWatcher) -> Self {
        Self {
            rendered: Arc::new(RwLock::new(String::new())),
        }
    }

    /// Get current rendered output
    pub fn get_rendered(&self) -> String {
        self.rendered.read().clone()
    }
}

impl WatchEventHook for BootstrapRenderHook {
    fn on_event(&self, event: &WatchEvent) {
        let files = match event {
            WatchEvent::Initial { files } => files,
            WatchEvent::Changed { files, .. } => files,
        };

        let mut parts = Vec::new();
        for entry in files {
            if let Some(content) = &entry.content {
                parts.push(format!("## {}\n\n{}", entry.name, content));
            }
        }
        let output = parts.join("\n\n");

        // Update rendered cache - use blocking write since this is sync context
        *self.rendered.write() = output;
    }
}

/// Watch strategy for a set of target files.
#[derive(Debug, Clone, Copy)]
pub enum WatchMode {
    /// Do not start any background watcher.
    Disabled,
    /// Use notify-based file watching with debounce.
    Notify { debounce: Duration },
    /// Use metadata polling on a fixed interval.
    Poll { interval: Duration },
    /// Try notify first; if it fails, fall back to polling.
    Auto {
        debounce: Duration,
        poll_interval: Duration,
    },
}

impl Default for WatchMode {
    fn default() -> Self {
        Self::Auto {
            debounce: DEFAULT_DEBOUNCE,
            poll_interval: DEFAULT_POLL_INTERVAL,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
struct FileMeta {
    modified: Option<SystemTime>,
    len: u64,
}

impl FileMeta {
    fn missing() -> Self {
        Self {
            modified: None,
            len: 0,
        }
    }
}

struct FileWatcherInner {
    // Immutable after construction
    root: PathBuf,
    files: Vec<String>,
    file_set: HashSet<String>,
    event_hooks: Vec<Arc<dyn WatchEventHook>>,

    // Current file contents - use Arc for zero-copy reads
    entries: RwLock<Arc<Vec<WatchedFileEntry>>>,

    // Metadata for change detection - use DashMap for lock-free reads
    metas: DashMap<String, FileMeta>,

    // Keep watcher alive for the lifetime of the cache.
    watcher: Mutex<Option<RecommendedWatcher>>,
    // Background tasks for notify and polling.
    watch_task: Mutex<Option<JoinHandle<()>>>,
    poll_task: Mutex<Option<JoinHandle<()>>>,
}

#[derive(Clone)]
pub struct FileWatcher {
    inner: Arc<FileWatcherInner>,
}

impl FileWatcher {
    /// Create watcher with default bootstrap renderer
    /// Returns the watcher and the bootstrap hook for accessing rendered output
    pub async fn new(
        root: PathBuf,
        files: impl IntoIterator<Item = impl Into<String>>,
    ) -> AgentResult<(Self, BootstrapRenderHook)> {
        let (watcher, hook) = Self::builder(root, files).build().await?;
        Ok((watcher, hook.expect("new() always creates bootstrap hook")))
    }

    /// Builder pattern for advanced configuration
    pub fn builder(
        root: PathBuf,
        files: impl IntoIterator<Item = impl Into<String>>,
    ) -> FileWatcherBuilder {
        FileWatcherBuilder::new(root, files)
    }
    /// Get current file entries (zero-copy via Arc)
    pub fn get_entries(&self) -> Arc<Vec<WatchedFileEntry>> {
        self.inner.entries.read().clone()
    }

    pub async fn start(&self, mode: WatchMode) -> AgentResult<()> {
        match mode {
            WatchMode::Disabled => Ok(()),
            WatchMode::Notify { debounce } => self.start_notify(debounce).await,
            WatchMode::Poll { interval } => self.start_polling(interval).await,
            WatchMode::Auto {
                debounce,
                poll_interval,
            } => match self.start_notify(debounce).await {
                Ok(()) => Ok(()),
                Err(err) => {
                    warn!("file watcher failed, falling back to polling: {}", err);
                    self.start_polling(poll_interval).await
                }
            },
        }
    }

    async fn refresh(&self) -> AgentResult<()> {
        let mut new_entries = Vec::new();

        for file in &self.inner.files {
            let path = self.inner.root.join(file);
            let content = match tokio::fs::read_to_string(&path).await {
                Ok(content) => Some(content),
                Err(err) => {
                    if err.kind() != std::io::ErrorKind::NotFound {
                        warn!("failed to read watched file {}: {}", path.display(), err);
                    }
                    None
                }
            };

            new_entries.push(WatchedFileEntry {
                name: file.clone(),
                content,
            });

            // Capture metadata for cheap change detection and polling fallback.
            let meta = match tokio::fs::metadata(&path).await {
                Ok(info) => FileMeta {
                    modified: info.modified().ok(),
                    len: info.len(),
                },
                Err(_) => FileMeta::missing(),
            };
            self.inner.metas.insert(file.clone(), meta);
        }
        let old_entries = self.inner.entries.read().clone();
        let new_arc = Arc::new(new_entries.clone());
        *self.inner.entries.write() = new_arc;

        // Compute changed names by diffing old vs new
        let event = if old_entries.is_empty() {
            WatchEvent::Initial {
                files: new_entries,
            }
        } else {
            let changed_names: HashSet<String> = new_entries
                .iter()
                .filter(|new| {
                    old_entries
                        .iter()
                        .find(|old| old.name == new.name)
                        .map(|old| old.content != new.content)
                        .unwrap_or(true)
                })
                .map(|e| e.name.clone())
                .collect();

            if !changed_names.is_empty() {
                WatchEvent::Changed {
                    files: new_entries,
                    changed_names,
                }
            } else {
                return Ok(());
            }
        };

        for hook in &self.inner.event_hooks {
            hook.on_event(&event);
        }

        Ok(())
    }

    async fn refresh_if_changed(&self) -> AgentResult<bool> {
        let latest = self.collect_metas().await;

        // Check if any metadata changed using DashMap
        let mut changed = false;
        for entry in latest.iter() {
            let file = entry.key();
            let new_meta = entry.value();
            if let Some(old_meta) = self.inner.metas.get(file) {
                if *old_meta != *new_meta {
                    changed = true;
                    break;
                }
            } else {
                changed = true;
                break;
            }
        }

        if !changed {
            return Ok(false);
        }

        self.refresh().await?;
        Ok(true)
    }
    async fn collect_metas(&self) -> DashMap<String, FileMeta> {
        let metas = DashMap::new();
        for file in &self.inner.files {
            let path = self.inner.root.join(file);
            let meta = match tokio::fs::metadata(&path).await {
                Ok(info) => FileMeta {
                    modified: info.modified().ok(),
                    len: info.len(),
                },
                Err(_) => FileMeta::missing(),
            };
            metas.insert(file.clone(), meta);
        }
        metas
    }

    async fn start_polling(&self, interval: Duration) -> AgentResult<()> {
        if self.inner.poll_task.lock().is_some() {
            return Ok(());
        }
        let cache = self.clone();
        let handle = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.tick().await;
            loop {
                ticker.tick().await;
                let _ = cache.refresh_if_changed().await;
            }
        });
        *self.inner.poll_task.lock() = Some(handle);
        Ok(())
    }

    async fn start_notify(&self, debounce: Duration) -> AgentResult<()> {
        if self.inner.watch_task.lock().is_some() {
            return Ok(());
        }

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut watcher = notify::recommended_watcher(move |res| {
            let _ = tx.send(res);
        })
        .map_err(|err| {
            AgentError::context_builder(format!("notify watcher init failed: {}", err))
        })?;

        // Prefer watching exact files; if unsupported, watch the root directory.
        if self.try_watch_files(&mut watcher).is_err() {
            self.watch_root(&mut watcher)?;
        }

        let cache = self.clone();
        let handle = tokio::spawn(async move {
            let mut pending = false;
            let mut deadline: Option<Instant> = None;

            loop {
                tokio::select! {
                    maybe = rx.recv() => {
                        let Some(event) = maybe else { break; };
                        match event {
                            Ok(event) => {
                                if cache.should_refresh(&event) {
                                    // Debounce: coalesce multiple events into one refresh.
                                    pending = true;
                                    deadline = Some(Instant::now() + debounce);
                                }
                            }
                            Err(err) => {
                                warn!("file watcher error: {}", err);
                            }
                        }
                    }
                    _ = sleep_until(deadline.unwrap()), if deadline.is_some() => {
                        if pending {
                            let _ = cache.refresh().await;
                        }
                        pending = false;
                        deadline = None;
                    }
                }
            }
        });

        *self.inner.watcher.lock() = Some(watcher);
        *self.inner.watch_task.lock() = Some(handle);
        Ok(())
    }
    fn try_watch_files(&self, watcher: &mut RecommendedWatcher) -> notify::Result<()> {
        for file in &self.inner.files {
            let path = self.inner.root.join(file);
            watcher.watch(&path, RecursiveMode::NonRecursive)?;
        }
        Ok(())
    }

    fn watch_root(&self, watcher: &mut RecommendedWatcher) -> AgentResult<()> {
        watcher
            .watch(&self.inner.root, RecursiveMode::NonRecursive)
            .map_err(|err| {
                AgentError::context_builder(format!(
                    "notify watcher failed to watch root {}: {}",
                    self.inner.root.display(),
                    err
                ))
            })
    }

    fn should_refresh(&self, event: &Event) -> bool {
        if !matches!(
            event.kind,
            EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
        ) {
            return false;
        }

        // Only react to target files to avoid unnecessary refreshes.
        event.paths.iter().any(|path| self.is_target_path(path))
    }

    fn is_target_path(&self, path: &Path) -> bool {
        let name = match path.file_name().and_then(|s| s.to_str()) {
            Some(value) => value,
            None => return false,
        };
        if !self.inner.file_set.contains(name) {
            return false;
        }

        // Ensure the event is for the root entry, not a same-named file elsewhere.
        if let Some(parent) = path.parent() {
            if parent == self.inner.root {
                return true;
            }
        }

        let expected = self.inner.root.join(name);
        path == expected
    }
}

/// Builder for FileWatcher
pub struct FileWatcherBuilder {
    root: PathBuf,
    files: Vec<String>,
    event_hooks: Vec<Arc<dyn WatchEventHook>>,
}

impl FileWatcherBuilder {
    pub fn new(root: PathBuf, files: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            root,
            files: files.into_iter().map(|v| v.into()).collect(),
            event_hooks: Vec::new(),
        }
    }

    pub fn on_event(mut self, hook: Arc<dyn WatchEventHook>) -> Self {
        self.event_hooks.push(hook);
        self
    }
    pub async fn build(self) -> AgentResult<(FileWatcher, Option<BootstrapRenderHook>)> {
        let root = tokio::fs::canonicalize(&self.root)
            .await
            .unwrap_or(self.root);
        let file_set = self.files.iter().cloned().collect();

        // Determine hooks before construction
        let (event_hooks, bootstrap_hook) = if self.event_hooks.is_empty() {
            let hook = BootstrapRenderHook {
                rendered: Arc::new(RwLock::new(String::new())),
            };
            let hooks = vec![Arc::new(hook.clone()) as Arc<dyn WatchEventHook>];
            (hooks, Some(hook))
        } else {
            (self.event_hooks, None)
        };

        let inner = Arc::new(FileWatcherInner {
            root,
            files: self.files,
            file_set,
            entries: RwLock::new(Arc::new(Vec::new())),
            metas: DashMap::new(),
            event_hooks,
            watcher: Mutex::new(None),
            watch_task: Mutex::new(None),
            poll_task: Mutex::new(None),
        });

        let cache = FileWatcher { inner };
        cache.refresh().await?;
        Ok((cache, bootstrap_hook))
    }
}

impl Drop for FileWatcherInner {
    fn drop(&mut self) {
        if let Some(handle) = self.watch_task.lock().take() {
            handle.abort();
        }
        if let Some(handle) = self.poll_task.lock().take() {
            handle.abort();
        }
        let _ = self.watcher.lock().take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn watched_files_cache_refresh_reads_files() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().to_path_buf();
        tokio::fs::write(root.join("AGENTS.md"), "Be helpful")
            .await
            .expect("write agents");

        let (_cache, hook) = FileWatcher::new(root, ["AGENTS.md", "USER.md"])
            .await
            .expect("cache init");
        let rendered = hook.get_rendered();

        assert!(rendered.contains("AGENTS.md"));
        assert!(rendered.contains("Be helpful"));
        assert!(!rendered.contains("USER.md"));
    }
    #[tokio::test]
    async fn watched_files_cache_poll_updates_on_change() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().to_path_buf();
        let file = root.join("AGENTS.md");

        tokio::fs::write(&file, "first").await.expect("write");

        let (cache, hook) = FileWatcher::new(root.clone(), ["AGENTS.md"])
            .await
            .expect("cache init");
        cache
            .start(WatchMode::Poll {
                interval: Duration::from_millis(50),
            })
            .await
            .expect("start polling");

        tokio::fs::write(&file, "second").await.expect("write");
        tokio::time::sleep(Duration::from_millis(150)).await;

        let rendered = hook.get_rendered();
        assert!(rendered.contains("second"));
    }

    #[tokio::test]
    async fn watched_files_cache_uses_custom_render_hook() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().to_path_buf();

        tokio::fs::write(root.join("AGENTS.md"), "Be helpful")
            .await
            .expect("write agents");

        // Create custom render hook
        struct CustomRenderHook {
            rendered: Arc<RwLock<String>>,
        }

        impl WatchEventHook for CustomRenderHook {
            fn on_event(&self, event: &WatchEvent) {
                let files = match event {
                    WatchEvent::Initial { files } => files,
                    WatchEvent::Changed { files, .. } => files,
                };
                let output = files
                    .iter()
                    .map(|entry| {
                        let content = entry.content.as_deref().unwrap_or("MISSING");
                        format!("{}={}", entry.name, content)
                    })
                    .collect::<Vec<_>>()
                    .join("|");
                *self.rendered.write() = output;
            }
        }

        let rendered = Arc::new(RwLock::new(String::new()));
        let hook = CustomRenderHook {
            rendered: rendered.clone(),
        };

        let _cache = FileWatcher::builder(root, ["AGENTS.md", "USER.md"])
            .on_event(Arc::new(hook))
            .build()
            .await
            .expect("cache init");

        // Wait for initial event to be processed
        tokio::time::sleep(Duration::from_millis(50)).await;

        let output = rendered.read();
        assert_eq!(*output, "AGENTS.md=Be helpful|USER.md=MISSING");
    }
    #[tokio::test]
    async fn watched_files_cache_builder_fires_initial_event() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().to_path_buf();

        tokio::fs::write(root.join("AGENTS.md"), "Be helpful")
            .await
            .expect("write agents");

        let event_log = Arc::new(RwLock::new(Vec::<String>::new()));
        let event_log_clone = event_log.clone();

        let _cache = FileWatcher::builder(root, ["AGENTS.md"])
            .on_event(Arc::new(move |event: &WatchEvent| {
                let log = event_log_clone.clone();
                let event = event.clone();
                tokio::spawn(async move {
                    let mut log = log.write();
                    match event {
                        WatchEvent::Initial { .. } => log.push("initial".to_string()),
                        WatchEvent::Changed { .. } => log.push("changed".to_string()),
                    }
                });
            }))
            .build()
            .await
            .expect("cache init");

        // Wait for initial event to be processed
        tokio::time::sleep(Duration::from_millis(50)).await;

        let log = event_log.read();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0], "initial");
    }

    #[tokio::test]
    async fn watched_files_cache_builder_multiple_event_hooks() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().to_path_buf();

        tokio::fs::write(root.join("AGENTS.md"), "Be helpful")
            .await
            .expect("write agents");

        let counter1 = Arc::new(RwLock::new(0));
        let counter2 = Arc::new(RwLock::new(0));

        let counter1_clone = counter1.clone();
        let counter2_clone = counter2.clone();

        let _cache = FileWatcher::builder(root, ["AGENTS.md"])
            .on_event(Arc::new(move |_event: &WatchEvent| {
                let counter = counter1_clone.clone();
                tokio::spawn(async move {
                    *counter.write() += 1;
                });
            }))
            .on_event(Arc::new(move |_event: &WatchEvent| {
                let counter = counter2_clone.clone();
                tokio::spawn(async move {
                    *counter.write() += 1;
                });
            }))
            .build()
            .await
            .expect("cache init");

        // Wait for events to be processed
        tokio::time::sleep(Duration::from_millis(50)).await;

        assert_eq!(*counter1.read(), 1);
        assert_eq!(*counter2.read(), 1);
    }
}
