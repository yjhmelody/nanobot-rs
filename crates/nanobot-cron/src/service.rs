//! Background cron scheduler with JSONL persistence and 1-second ticker loop.
//!
//! This module provides [`CronService`], the central scheduler, and [`CronJobHandler`],
//! the callback trait for job execution.
//!
//! ## How it works
//!
//! 1. On startup, the store file is loaded synchronously in [`CronService::new`].
//! 2. [`CronService::start`] spawns a tokio task that ticks every second.
//! 3. Each tick, [`CronService::on_timer`] gathers job IDs whose
//!    `next_run_at_ms <= now`, then executes each one via [`CronService::execute_job`].
//! 4. After execution, the job's state is updated and the store is persisted.
//!
//! ## Concurrency model
//!
//! - **Store access** is guarded by a `tokio::sync::RwLock` because writes may need
//!   to hold the lock across await points (file I/O).
//! - **Last-modified tracking** uses `parking_lot::Mutex` for short, synchronous
//!   comparisons.
//! - **Running flag** is an `AtomicBool` for lock-free status checks from the ticker.
//! - **Timer task handle** is in a `tokio::sync::Mutex` since it can be replaced
//!   across await points.
//!
//! ## External modification support
//!
//! The store file's mtime is checked on every tick. If an external process has
//! modified the file, the in-memory store is reloaded automatically. This enables
//! hot-reloading of jobs without restarting the service.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

use anyhow::Context;
use async_trait::async_trait;
use parking_lot::Mutex;
use tokio::fs as async_fs;
use tokio::sync::{RwLock, RwLockWriteGuard};
use tokio::task::JoinHandle;
use tracing::{error, info, warn};

use super::add_job_params::AddJobParams;
use super::error::{CronError, CronResult};

use nanobot_types::cron::{
    CronJob, CronJobState, CronPayload, CronScheduleKind, CronStatus, CronStore, now_ms,
};

/// Logging target used for all cron-related trace events.
const TARGET: &str = "nanobot::cron";

/// Callback invoked by `CronService` when a scheduled job fires.
#[async_trait]
pub trait CronJobHandler: Send + Sync {
    /// Called when a cron job is due to run.
    ///
    /// Return `Ok(Some(output))` to record output, `Ok(None)` to skip recording,
    /// or an error to mark the job as failed.
    async fn on_job(&self, job: CronJob) -> CronResult<Option<String>>;
}

/// Background scheduler that reads/writes a JSONL store and fires jobs on schedule.
///
/// `CronService` runs a background tokio task (started via [`start`](CronService::start))
/// that ticks every 1 second. On each tick, it:
///
/// 1. Reloads the store if the file's mtime changed (enabling hot-reload).
/// 2. Collects all enabled jobs whose `next_run_at_ms <= now`.
/// 3. Executes each due job in sequence via the registered [`CronJobHandler`].
/// 4. Updates the job state (last_run, next_run) and persists the store.
///
/// # Concurrency
///
/// - **`store`**: `tokio::sync::RwLock` — held across async file I/O.
/// - **`last_mtime`**: `parking_lot::Mutex` — short synchronous compare-and-swap.
/// - **`running`**: `AtomicBool` — lock-free flag for the ticker loop.
/// - **`timer_task`**: `tokio::sync::Mutex` — updated across await boundaries.
/// - **`on_job`**: `tokio::sync::RwLock` — read on every tick, write on registration.
///
/// # Examples
///
/// ```ignore
/// use std::sync::Arc;
/// use nanobot_cron::CronService;
///
/// let svc = Arc::new(CronService::new("/tmp/cron_store.json".into()));
/// svc.register_on_job_handler(my_handler).await;
/// svc.start().await.unwrap();
/// ```
pub struct CronService {
    /// Path to the JSONL store file on disk.
    store_path: PathBuf,
    /// In-memory store of all cron jobs, protected by a tokio RwLock.
    store: RwLock<CronStore>,
    /// Last-known modification time of the store file, used to detect external changes.
    last_mtime: Mutex<Option<SystemTime>>,
    /// Whether the background ticker loop is active.
    running: AtomicBool,
    /// Handle to the spawned ticker task, so it can be cancelled on stop.
    timer_task: tokio::sync::Mutex<Option<JoinHandle<()>>>,
    /// Optional callback invoked for each due job.
    on_job: RwLock<Option<Arc<dyn CronJobHandler>>>,
}

impl CronService {
    /// Creates a new `CronService` backed by the given store file path.
    ///
    /// The store file is loaded synchronously during construction. If the file
    /// does not exist or fails to parse, an empty store is used (with a warning
    /// logged).
    ///
    /// # Arguments
    ///
    /// * `store_path` — Absolute or relative path to the JSONL store file.
    ///
    /// # Returns
    ///
    /// A new `CronService` in the stopped state. Call [`start`](CronService::start)
    /// to begin the background ticker loop.
    pub fn new(store_path: PathBuf) -> Self {
        let (store, last_mtime) = load_store_sync(&store_path);
        Self {
            store_path,
            store: RwLock::new(store),
            last_mtime: Mutex::new(last_mtime),
            running: AtomicBool::new(false),
            timer_task: tokio::sync::Mutex::new(None),
            on_job: RwLock::new(None),
        }
    }

    /// Registers the handler that will be called each time a job fires.
    ///
    /// Call this before [`start`](CronService::start) or at any point during
    /// runtime. Only one handler can be registered at a time; calling this
    /// replaces any previously registered handler.
    ///
    /// # Arguments
    ///
    /// * `handler` — An `Arc`-wrapped implementation of [`CronJobHandler`].
    ///
    /// # Locking
    ///
    /// Acquires the `on_job` write lock. Safe to call while the service is running.
    pub async fn register_on_job_handler(&self, handler: Arc<dyn CronJobHandler>) {
        *self.on_job.write().await = Some(handler);
    }

    /// Starts the background ticker loop. Returns immediately if already running.
    ///
    /// On first start, this method:
    /// 1. Reloads the store from disk (in case an external process modified it).
    /// 2. Recomputes `next_run_at_ms` for all enabled jobs.
    /// 3. Persists the updated store.
    /// 4. Spawns a tokio task with a 1-second interval ticker.
    ///
    /// The ticker uses [`MissedTickBehavior::Delay`] to avoid burst-firing when
    /// the system is under load.
    ///
    /// # Arguments
    ///
    /// * `self` — `Arc<Self>` is required because the spawned task must own a
    ///   reference to the service.
    ///
    /// # Errors
    ///
    /// Returns an error if the store file cannot be read or written during the
    /// initial reload/save cycle.
    ///
    /// # Locking
    ///
    /// Acquires the `store` write lock during initialisation, then only the
    /// `running` atomic flag during the ticker loop.
    pub async fn start(self: &Arc<Self>) -> CronResult<()> {
        if self.running.swap(true, Ordering::SeqCst) {
            return Ok(());
        }

        {
            let mut store = self.write_store().await;
            self.reload_if_modified_locked(&mut store).await?;
            recompute_next_runs(&mut store.jobs);
            self.save_store_locked(&store).await?;
        }

        let this = self.clone();
        let handle = tokio::spawn(async move {
            // 1s ticker keeps scheduling simple and deterministic for MVP.
            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(1));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                if !this.running.load(Ordering::SeqCst) {
                    break;
                }
                if let Err(err) = this.on_timer().await {
                    warn!(target: TARGET, "cron tick failed: {}", err);
                }
            }
        });
        *self.timer_task.lock().await = Some(handle);
        info!(target: TARGET, "cron service started");

        Ok(())
    }

    /// Stops the background ticker and aborts the timer task.
    ///
    /// Sets the `running` flag to `false` (so the ticker loop exits on its next
    /// iteration) and aborts the spawned tokio task. This is safe to call even if
    /// the service was never started or already stopped.
    ///
    /// # Locking
    ///
    /// Acquires the `timer_task` mutex to take and abort the join handle.
    pub async fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(handle) = self.timer_task.lock().await.take() {
            handle.abort();
        }
    }

    /// Returns a snapshot of the current scheduler status.
    ///
    /// The result includes whether the service is enabled, the total number of jobs
    /// in the store, and the earliest upcoming wake time across all enabled jobs.
    ///
    /// # Errors
    ///
    /// Returns an error if the store file cannot be read during reload.
    ///
    /// # Locking
    ///
    /// Acquires the `store` write lock (to reload-if-modified) and reads the
    /// `running` atomic flag.
    pub async fn status(&self) -> CronResult<CronStatus> {
        let mut store = self.write_store().await;
        self.reload_if_modified_locked(&mut store).await?;
        Ok(CronStatus {
            enabled: self.running.load(Ordering::SeqCst),
            jobs: store.jobs.len(),
            next_wake_at_ms: next_wake(&store.jobs),
        })
    }

    /// Lists all jobs, optionally including disabled ones, sorted by next run time.
    ///
    /// Jobs are sorted by their `next_run_at_ms` in ascending order. Disabled jobs
    /// and jobs with no next run are placed at the end (using `i64::MAX` as the
    /// sort key for `None` values).
    ///
    /// # Arguments
    ///
    /// * `include_disabled` — If `true`, returns both enabled and disabled jobs.
    ///   If `false`, only enabled jobs are returned.
    ///
    /// # Errors
    ///
    /// Returns an error if the store file cannot be read during reload.
    pub async fn list_jobs(&self, include_disabled: bool) -> CronResult<Vec<CronJob>> {
        let mut store = self.write_store().await;
        self.reload_if_modified_locked(&mut store).await?;

        let mut jobs = if include_disabled {
            store.jobs.clone()
        } else {
            store
                .jobs
                .iter()
                .filter(|j| j.enabled)
                .cloned()
                .collect::<Vec<_>>()
        };

        jobs.sort_by_key(|j| j.state.next_run_at_ms.unwrap_or(i64::MAX));
        Ok(jobs)
    }

    /// Validates and adds a new cron job, persisting it to the store.
    ///
    /// The job is assigned a UUID v4 identifier and added with `enabled: true`.
    /// Its `next_run_at_ms` is computed immediately based on the schedule.
    ///
    /// # Arguments
    ///
    /// * `params` — Builder-style parameters (name, schedule, message, etc.).
    ///
    /// # Errors
    ///
    /// - Returns an error if the schedule fails validation (e.g., `tz` set without
    ///   a cron expression).
    /// - Returns an error if the store file cannot be written.
    ///
    /// # Locking
    ///
    /// Acquires the `store` write lock for the duration of reload, insert, and save.
    pub async fn add_job(&self, params: AddJobParams) -> CronResult<CronJob> {
        params.schedule.validate_for_add()?;
        let now = now_ms();

        let mut store = self.write_store().await;
        self.reload_if_modified_locked(&mut store).await?;
        let next_run_at_ms = params.schedule.compute_next_run(now);

        let job = CronJob {
            id: uuid::Uuid::new_v4().to_string(),
            name: params.name,
            enabled: true,
            schedule: params.schedule,
            payload: CronPayload {
                // The payload kind is hardcoded to "agent_turn" because cron jobs are
                // currently only used to schedule agent interactions. This could be
                // made configurable if other payload types are needed in the future.
                kind: "agent_turn".to_string(),
                message: params.message,
                deliver: params.deliver,
                channel: params.channel,
                to: params.to,
            },
            state: CronJobState {
                next_run_at_ms,
                ..CronJobState::default()
            },
            created_at_ms: now,
            updated_at_ms: now,
            delete_after_run: params.delete_after_run,
        };

        store.jobs.push(job.clone());
        self.save_store_locked(&store).await?;
        Ok(job)
    }

    /// Removes the job with the given ID. Returns `true` if the job was found and removed.
    ///
    /// The store is only saved to disk if a job was actually removed. If no job
    /// matched the provided ID, `false` is returned and no write occurs.
    ///
    /// # Arguments
    ///
    /// * `job_id` — The UUID of the job to remove (as a string).
    ///
    /// # Errors
    ///
    /// Returns an error if the store file cannot be read or written.
    pub async fn remove_job(&self, job_id: &str) -> CronResult<bool> {
        let mut store = self.write_store().await;
        self.reload_if_modified_locked(&mut store).await?;

        let before = store.jobs.len();
        store.jobs.retain(|j| j.id != job_id);
        let removed = store.jobs.len() < before;

        if removed {
            self.save_store_locked(&store).await?;
        }

        Ok(removed)
    }

    // Called on every 1-second tick. Collects job IDs whose scheduled time has
    // passed, then executes each one sequentially.
    //
    // Due IDs are gathered upfront so the store lock is released before any
    // callbacks run — this avoids holding the lock across the
    // `CronJobHandler::on_job` await point.
    async fn on_timer(&self) -> CronResult<()> {
        // Collect due ids first to avoid holding the store lock while executing callbacks.
        let due_ids = {
            let mut store = self.write_store().await;
            self.reload_if_modified_locked(&mut store).await?;
            let now = now_ms();
            store
                .jobs
                .iter()
                .filter(|j| j.enabled && j.state.next_run_at_ms.map(|t| now >= t).unwrap_or(false))
                .map(|j| j.id.clone())
                .collect::<Vec<_>>()
        };

        for id in due_ids {
            if let Err(err) = self.execute_job(&id).await {
                error!(target: TARGET, "cron job {} failed: {}", id, err);
            }
        }

        Ok(())
    }

    // Executes a single cron job by ID.
    //
    // 1. Takes a snapshot of the job (under the store lock).
    // 2. Calls the registered `CronJobHandler::on_job` (outside the store lock).
    // 3. Re-acquires the store lock to update the job's state (last run, next run).
    // 4. For `At` schedules: either disables or deletes the job depending on
    //    `delete_after_run`.
    // 5. Persists the store.
    //
    // If the handler returns an error, the job status is set to "error" and the
    // error message is recorded. The job is not removed on failure.
    //
    // If the job was deleted externally between snapshot and state update, the
    // update is silently skipped.
    async fn execute_job(&self, job_id: &str) -> CronResult<()> {
        let job_snapshot = {
            let mut store = self.write_store().await;
            self.reload_if_modified_locked(&mut store).await?;
            store.jobs.iter().find(|j| j.id == job_id).cloned()
        };

        let Some(job_snapshot) = job_snapshot else {
            return Ok(());
        };

        info!(
            target: TARGET,
            "cron executing job '{}' ({})",
            job_snapshot.name, job_snapshot.id
        );
        let started_at = now_ms();

        let handler = self.on_job.read().await.clone();
        let mut last_status = "ok".to_string();
        let mut last_error: Option<String> = None;

        if let Some(handler) = handler
            && let Err(err) = handler.on_job(job_snapshot.clone()).await
        {
            last_status = "error".to_string();
            last_error = Some(err.to_string());
        }

        let mut store = self.write_store().await;
        self.reload_if_modified_locked(&mut store).await?;
        let Some(idx) = store.jobs.iter().position(|j| j.id == job_id) else {
            return Ok(());
        };

        let mut should_delete = false;
        {
            let job = &mut store.jobs[idx];
            job.state.last_run_at_ms = Some(started_at);
            job.state.last_status = Some(last_status);
            job.state.last_error = last_error;
            job.updated_at_ms = now_ms();

            match job.schedule.kind {
                CronScheduleKind::At => {
                    if job.delete_after_run {
                        should_delete = true;
                    } else {
                        job.enabled = false;
                        job.state.next_run_at_ms = None;
                    }
                }
                _ => {
                    job.state.next_run_at_ms = job.schedule.compute_next_run(now_ms());
                }
            }
        }

        if should_delete {
            store.jobs.remove(idx);
        }

        self.save_store_locked(&store).await?;
        Ok(())
    }

    // Acquires the write lock on the in-memory cron store.
    //
    // This is a thin wrapper to avoid repeating `.store.write().await` and to make
    // lock-acquisition sites more readable.
    async fn write_store(&self) -> RwLockWriteGuard<'_, CronStore> {
        self.store.write().await
    }

    // Checks whether the store file's mtime has changed since the last load.
    // If so, the file is re-read and parsed, replacing the in-memory store.
    //
    // This enables hot-reload: external processes can modify the store file and
    // the scheduler picks up changes on the next tick without restarting.
    //
    // # Locking
    //
    // Caller must already hold the `store` write lock. This method also acquires
    // the `last_mtime` mutex (short, synchronous).
    async fn reload_if_modified_locked(&self, store: &mut CronStore) -> CronResult<()> {
        let metadata = match async_fs::metadata(&self.store_path).await {
            Ok(metadata) => metadata,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(err) => {
                return Err(CronError::message(format!(
                    "failed to stat {}: {}",
                    self.store_path.display(),
                    err
                )));
            }
        };
        let modified = metadata.modified().ok();

        {
            let last_mtime = self.last_mtime.lock();
            if modified.is_some() && *last_mtime == modified {
                return Ok(());
            }
        }

        let loaded = read_store_file_async(&self.store_path).await?;
        *store = loaded;
        *self.last_mtime.lock() = modified;
        Ok(())
    }

    // Persists the in-memory cron store to disk as a pretty-printed JSON file.
    //
    // The parent directory is created if it does not exist. After writing, the
    // file's mtime is refreshed in `self.last_mtime` so that the next
    // `reload_if_modified_locked` call does not immediately re-read what we just
    // wrote.
    //
    // # Locking
    //
    // Caller must already hold the `store` write lock. This method also acquires
    // the `last_mtime` mutex.
    async fn save_store_locked(&self, store: &CronStore) -> CronResult<()> {
        if let Some(parent) = self.store_path.parent() {
            async_fs::create_dir_all(parent)
                .await
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }

        let text = serde_json::to_string_pretty(store)?;
        async_fs::write(&self.store_path, text)
            .await
            .with_context(|| format!("failed to write {}", self.store_path.display()))?;

        // Refresh the mtime so our own write doesn't trigger a false reload.
        let modified = async_fs::metadata(&self.store_path)
            .await
            .ok()
            .and_then(|m| m.modified().ok());

        *self.last_mtime.lock() = modified;

        Ok(())
    }
}

// Recomputes `next_run_at_ms` for all enabled jobs based on the current time.
// Used during service start to re-sync schedules after a restart.
fn recompute_next_runs(jobs: &mut [CronJob]) {
    let now = now_ms();
    for job in jobs.iter_mut() {
        if job.enabled {
            job.state.next_run_at_ms = job.schedule.compute_next_run(now);
        }
    }
}

// Returns the earliest `next_run_at_ms` across all enabled jobs, or `None` if
// there are no enabled jobs scheduled.
fn next_wake(jobs: &[CronJob]) -> Option<i64> {
    jobs.iter()
        .filter(|j| j.enabled)
        .filter_map(|j| j.state.next_run_at_ms)
        .min()
}

// Synchronously loads the cron store from disk at construction time.
//
// Returns a default (empty) store if the file does not exist or fails to parse.
// Parse failures are logged as warnings rather than errors, since the file may
// have been written by a newer version of the schema.
fn load_store_sync(path: &Path) -> (CronStore, Option<SystemTime>) {
    if !path.exists() {
        return (CronStore::default(), None);
    }

    match read_store_file(path) {
        Ok(store) => {
            let modified = std::fs::metadata(path).ok().and_then(|m| m.modified().ok());
            (store, modified)
        }
        Err(err) => {
            warn!(
                target: TARGET,
                "failed to load cron store '{}': {}",
                path.display(),
                err
            );
            (CronStore::default(), None)
        }
    }
}

// Synchronously reads and parses a cron store file.
//
// If the stored `version` field is <= 0, it is bumped to version 1 as a
// migration step for older store files that predate the version field.
fn read_store_file(path: &Path) -> CronResult<CronStore> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read cron store {}", path.display()))?;
    let mut store: CronStore = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse cron store {}", path.display()))?;
    if store.version <= 0 {
        store.version = 1;
    }
    Ok(store)
}

// Async version of `read_store_file`, used by the runtime reload path.
// Performs the same parsing and version migration as the sync counterpart.
async fn read_store_file_async(path: &Path) -> CronResult<CronStore> {
    let raw = async_fs::read_to_string(path)
        .await
        .with_context(|| format!("failed to read cron store {}", path.display()))?;
    let mut store: CronStore = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse cron store {}", path.display()))?;
    if store.version <= 0 {
        store.version = 1;
    }
    Ok(store)
}

// Unit and integration tests for `CronService`.
//
// Tests use temporary files in the system temp directory with UUID names to
// avoid collisions. Each test cleans up its temp file on completion.
#[cfg(test)]
mod tests {
    use super::*;
    use nanobot_types::cron::CronSchedule;
    use std::sync::atomic::AtomicUsize;

    // Creates a unique temporary file path for a test case.
    fn temp_store_path(case: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "nanobot-cron-{}-{}.json",
            case,
            uuid::Uuid::new_v4()
        ))
    }

    // A test-only `CronJobHandler` that counts how many times it was invoked.
    struct TestCronJobHandler {
        called: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl CronJobHandler for TestCronJobHandler {
        async fn on_job(&self, _job: CronJob) -> CronResult<Option<String>> {
            self.called.fetch_add(1, Ordering::SeqCst);
            Ok(Some("ok".to_string()))
        }
    }

    /// Verifies that a schedule with `tz` but without a cron expression is rejected.
    #[test]
    fn validate_schedule_rejects_tz_without_cron() {
        let schedule = CronSchedule {
            kind: CronScheduleKind::Every,
            every_ms: Some(1000),
            tz: Some("UTC".to_string()),
            ..CronSchedule::default()
        };
        let err = schedule
            .validate_for_add()
            .expect_err("schedule should reject tz outside cron");
        assert!(err.to_string().contains("tz can only be used"));
    }

    /// Verifies that `compute_next_run` produces correct results for all three
    /// schedule kinds (At, Every, Cron).
    #[test]
    fn compute_next_run_handles_at_every_cron() {
        let now = 1_700_000_000_000i64;

        let at = CronSchedule {
            kind: CronScheduleKind::At,
            at_ms: Some(now + 5_000),
            ..CronSchedule::default()
        };
        assert_eq!(at.compute_next_run(now), Some(now + 5_000));

        let every = CronSchedule {
            kind: CronScheduleKind::Every,
            every_ms: Some(30_000),
            ..CronSchedule::default()
        };
        assert_eq!(every.compute_next_run(now), Some(now + 30_000));

        let cron = CronSchedule {
            kind: CronScheduleKind::Cron,
            expr: Some("*/5 * * * * * *".to_string()),
            ..CronSchedule::default()
        };
        let next = cron
            .compute_next_run(now)
            .expect("cron schedule should compute next run");
        assert!(next > now);
    }

    /// Verifies the full add-list-remove lifecycle of a cron job, including
    /// persistence to disk (raw file content is checked for the job ID).
    #[tokio::test]
    async fn add_list_remove_job_roundtrip() {
        let path = temp_store_path("roundtrip");
        let service = CronService::new(path.clone());

        let schedule = CronSchedule {
            kind: CronScheduleKind::Every,
            every_ms: Some(60_000),
            ..CronSchedule::default()
        };
        let job = service
            .add_job(
                AddJobParams::new("test-job".to_string(), schedule, "hello".to_string())
                    .with_deliver(true)
                    .with_channel("cli".to_string())
                    .with_to("direct".to_string()),
            )
            .await
            .expect("add_job should succeed");

        let jobs = service
            .list_jobs(false)
            .await
            .expect("list_jobs should succeed");
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].id, job.id);

        let raw = std::fs::read_to_string(&path).expect("jobs.json should exist");
        assert!(raw.contains(&job.id));

        let removed = service
            .remove_job(&job.id)
            .await
            .expect("remove_job should succeed");
        assert!(removed);

        let jobs = service
            .list_jobs(false)
            .await
            .expect("list_jobs should succeed after remove");
        assert!(jobs.is_empty());

        let _ = std::fs::remove_file(path);
    }

    /// Verifies that executing a job invokes the registered callback and updates
    /// the job's state (`last_run_at_ms`, `last_status`, `next_run_at_ms`).
    #[tokio::test]
    async fn execute_job_invokes_callback_and_updates_state() {
        let path = temp_store_path("execute");
        let service = CronService::new(path.clone());

        let called = Arc::new(AtomicUsize::new(0));
        service
            .register_on_job_handler(Arc::new(TestCronJobHandler {
                called: called.clone(),
            }))
            .await;

        let schedule = CronSchedule {
            kind: CronScheduleKind::Every,
            every_ms: Some(60_000),
            ..CronSchedule::default()
        };
        let job = service
            .add_job(
                AddJobParams::new("cb-job".to_string(), schedule, "hello".to_string())
                    .with_deliver(true)
                    .with_channel("cli".to_string())
                    .with_to("direct".to_string()),
            )
            .await
            .expect("add_job should succeed");

        service
            .execute_job(&job.id)
            .await
            .expect("execute_job should succeed");

        assert_eq!(called.load(Ordering::SeqCst), 1);

        let jobs = service
            .list_jobs(true)
            .await
            .expect("list_jobs include disabled");
        let executed = jobs
            .iter()
            .find(|j| j.id == job.id)
            .expect("job should exist");
        assert!(executed.state.last_run_at_ms.is_some());
        assert_eq!(executed.state.last_status.as_deref(), Some("ok"));
        assert!(executed.state.next_run_at_ms.is_some());

        let _ = std::fs::remove_file(path);
    }
}
