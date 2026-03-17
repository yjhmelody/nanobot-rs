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
use crate::observability::TARGET_CRON;

use crate::types::cron::{
    CronJob, CronJobState, CronPayload, CronScheduleKind, CronStatus, CronStore, now_ms,
};

#[async_trait]
pub trait CronJobHandler: Send + Sync {
    async fn on_job(&self, job: CronJob) -> CronResult<Option<String>>;
}

pub struct CronService {
    store_path: PathBuf,
    store: RwLock<CronStore>,
    last_mtime: Mutex<Option<SystemTime>>,
    running: AtomicBool,
    timer_task: tokio::sync::Mutex<Option<JoinHandle<()>>>,
    on_job: RwLock<Option<Arc<dyn CronJobHandler>>>,
}

impl CronService {
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

    pub async fn register_on_job_handler(&self, handler: Arc<dyn CronJobHandler>) {
        *self.on_job.write().await = Some(handler);
    }

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
                    warn!(target: TARGET_CRON, "cron tick failed: {}", err);
                }
            }
        });
        *self.timer_task.lock().await = Some(handle);
        info!(target: TARGET_CRON, "cron service started");

        Ok(())
    }

    pub async fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(handle) = self.timer_task.lock().await.take() {
            handle.abort();
        }
    }

    pub async fn status(&self) -> CronResult<CronStatus> {
        let mut store = self.write_store().await;
        self.reload_if_modified_locked(&mut store).await?;
        Ok(CronStatus {
            enabled: self.running.load(Ordering::SeqCst),
            jobs: store.jobs.len(),
            next_wake_at_ms: next_wake(&store.jobs),
        })
    }

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
                error!(target: TARGET_CRON, "cron job {} failed: {}", id, err);
            }
        }

        Ok(())
    }

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
            target: TARGET_CRON,
            "cron executing job '{}' ({})",
            job_snapshot.name, job_snapshot.id
        );
        let started_at = now_ms();

        let handler = self.on_job.read().await.clone();
        let mut last_status = "ok".to_string();
        let mut last_error: Option<String> = None;

        if let Some(handler) = handler {
            if let Err(err) = handler.on_job(job_snapshot.clone()).await {
                last_status = "error".to_string();
                last_error = Some(err.to_string());
            }
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

    async fn write_store(&self) -> RwLockWriteGuard<'_, CronStore> {
        self.store.write().await
    }

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

        let modified = async_fs::metadata(&self.store_path)
            .await
            .ok()
            .and_then(|m| m.modified().ok());

        *self.last_mtime.lock() = modified;

        Ok(())
    }
}

fn recompute_next_runs(jobs: &mut [CronJob]) {
    let now = now_ms();
    for job in jobs.iter_mut() {
        if job.enabled {
            job.state.next_run_at_ms = job.schedule.compute_next_run(now);
        }
    }
}

fn next_wake(jobs: &[CronJob]) -> Option<i64> {
    jobs.iter()
        .filter(|j| j.enabled)
        .filter_map(|j| j.state.next_run_at_ms)
        .min()
}

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
                target: TARGET_CRON,
                "failed to load cron store '{}': {}",
                path.display(),
                err
            );
            (CronStore::default(), None)
        }
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::cron::CronSchedule;
    use std::sync::atomic::AtomicUsize;

    fn temp_store_path(case: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "nanobot-rs-cron-{}-{}.json",
            case,
            uuid::Uuid::new_v4()
        ))
    }

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
