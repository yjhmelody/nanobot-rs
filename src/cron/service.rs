use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{Local, TimeZone, Utc};
use chrono_tz::Tz;
use cron::Schedule;
use serde::{Deserialize, Serialize};
use tokio::sync::{RwLock, RwLockWriteGuard};
use tokio::task::JoinHandle;
use tracing::{error, info, warn};

fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CronScheduleKind {
    At,
    Every,
    Cron,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CronSchedule {
    /// One of `at`, `every`, `cron`.
    pub kind: CronScheduleKind,
    /// Unix ms for one-shot schedule.
    pub at_ms: Option<i64>,
    /// Interval ms for fixed-rate schedule.
    pub every_ms: Option<i64>,
    /// Cron expression when `kind == Cron`.
    pub expr: Option<String>,
    /// IANA timezone for cron expressions.
    pub tz: Option<String>,
}

impl Default for CronSchedule {
    fn default() -> Self {
        Self {
            kind: CronScheduleKind::Every,
            at_ms: None,
            every_ms: None,
            expr: None,
            tz: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CronPayload {
    pub kind: String,
    pub message: String,
    pub deliver: bool,
    pub channel: Option<String>,
    pub to: Option<String>,
}

impl Default for CronPayload {
    fn default() -> Self {
        Self {
            kind: "agent_turn".to_string(),
            message: String::new(),
            deliver: false,
            channel: None,
            to: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CronJobState {
    pub next_run_at_ms: Option<i64>,
    pub last_run_at_ms: Option<i64>,
    pub last_status: Option<String>,
    pub last_error: Option<String>,
}

impl Default for CronJobState {
    fn default() -> Self {
        Self {
            next_run_at_ms: None,
            last_run_at_ms: None,
            last_status: None,
            last_error: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CronJob {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub schedule: CronSchedule,
    pub payload: CronPayload,
    pub state: CronJobState,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub delete_after_run: bool,
}

impl Default for CronJob {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            enabled: true,
            schedule: CronSchedule::default(),
            payload: CronPayload::default(),
            state: CronJobState::default(),
            created_at_ms: 0,
            updated_at_ms: 0,
            delete_after_run: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct CronStore {
    version: i64,
    jobs: Vec<CronJob>,
}

impl Default for CronStore {
    fn default() -> Self {
        Self {
            version: 1,
            jobs: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CronStatus {
    pub enabled: bool,
    pub jobs: usize,
    pub next_wake_at_ms: Option<i64>,
}

#[async_trait]
pub trait CronJobHandler: Send + Sync {
    async fn on_job(&self, job: CronJob) -> Result<Option<String>>;
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

    pub async fn start(self: &Arc<Self>) -> Result<()> {
        if self.running.swap(true, Ordering::SeqCst) {
            return Ok(());
        }

        {
            let mut store = self.write_store().await;
            self.reload_if_modified_locked(&mut store)?;
            recompute_next_runs(&mut store.jobs);
            self.save_store_locked(&store)?;
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
                    warn!("cron tick failed: {}", err);
                }
            }
        });
        *self.timer_task.lock().await = Some(handle);
        info!("cron service started");

        Ok(())
    }

    pub async fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(handle) = self.timer_task.lock().await.take() {
            handle.abort();
        }
    }

    pub async fn status(&self) -> Result<CronStatus> {
        let mut store = self.write_store().await;
        self.reload_if_modified_locked(&mut store)?;
        Ok(CronStatus {
            enabled: self.running.load(Ordering::SeqCst),
            jobs: store.jobs.len(),
            next_wake_at_ms: next_wake(&store.jobs),
        })
    }

    pub async fn list_jobs(&self, include_disabled: bool) -> Result<Vec<CronJob>> {
        let mut store = self.write_store().await;
        self.reload_if_modified_locked(&mut store)?;

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

    pub async fn add_job(
        &self,
        name: String,
        schedule: CronSchedule,
        message: String,
        deliver: bool,
        channel: Option<String>,
        to: Option<String>,
        delete_after_run: bool,
    ) -> Result<CronJob> {
        validate_schedule_for_add(&schedule)?;
        let now = now_ms();

        let mut store = self.write_store().await;
        self.reload_if_modified_locked(&mut store)?;

        let job = CronJob {
            id: uuid::Uuid::new_v4().to_string(),
            name,
            enabled: true,
            schedule: schedule.clone(),
            payload: CronPayload {
                kind: "agent_turn".to_string(),
                message,
                deliver,
                channel,
                to,
            },
            state: CronJobState {
                next_run_at_ms: compute_next_run(&schedule, now),
                ..CronJobState::default()
            },
            created_at_ms: now,
            updated_at_ms: now,
            delete_after_run,
        };

        store.jobs.push(job.clone());
        self.save_store_locked(&store)?;
        Ok(job)
    }

    pub async fn remove_job(&self, job_id: &str) -> Result<bool> {
        let mut store = self.write_store().await;
        self.reload_if_modified_locked(&mut store)?;

        let before = store.jobs.len();
        store.jobs.retain(|j| j.id != job_id);
        let removed = store.jobs.len() < before;

        if removed {
            self.save_store_locked(&store)?;
        }

        Ok(removed)
    }

    async fn on_timer(&self) -> Result<()> {
        // Collect due ids first to avoid holding the store lock while executing callbacks.
        let due_ids = {
            let mut store = self.write_store().await;
            self.reload_if_modified_locked(&mut store)?;
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
                error!("cron job {} failed: {}", id, err);
            }
        }

        Ok(())
    }

    async fn execute_job(&self, job_id: &str) -> Result<()> {
        let job_snapshot = {
            let mut store = self.write_store().await;
            self.reload_if_modified_locked(&mut store)?;
            store.jobs.iter().find(|j| j.id == job_id).cloned()
        };

        let Some(job_snapshot) = job_snapshot else {
            return Ok(());
        };

        info!(
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
        self.reload_if_modified_locked(&mut store)?;
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
                    job.state.next_run_at_ms = compute_next_run(&job.schedule, now_ms());
                }
            }
        }

        if should_delete {
            store.jobs.remove(idx);
        }

        self.save_store_locked(&store)?;
        Ok(())
    }

    async fn write_store(&self) -> RwLockWriteGuard<'_, CronStore> {
        self.store.write().await
    }

    fn reload_if_modified_locked(&self, store: &mut CronStore) -> Result<()> {
        if !self.store_path.exists() {
            return Ok(());
        }

        let metadata = std::fs::metadata(&self.store_path)
            .with_context(|| format!("failed to stat {}", self.store_path.display()))?;
        let modified = metadata.modified().ok();

        let mut last_mtime = self
            .last_mtime
            .lock()
            .map_err(|_| anyhow::anyhow!("cron mtime lock poisoned"))?;

        if modified.is_some() && *last_mtime == modified {
            return Ok(());
        }

        let loaded = read_store_file(&self.store_path)?;
        *store = loaded;
        *last_mtime = modified;
        Ok(())
    }

    fn save_store_locked(&self, store: &CronStore) -> Result<()> {
        if let Some(parent) = self.store_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }

        let text = serde_json::to_string_pretty(store)?;
        std::fs::write(&self.store_path, text)
            .with_context(|| format!("failed to write {}", self.store_path.display()))?;

        let modified = std::fs::metadata(&self.store_path)
            .ok()
            .and_then(|m| m.modified().ok());

        if let Ok(mut guard) = self.last_mtime.lock() {
            *guard = modified;
        }

        Ok(())
    }
}

fn validate_schedule_for_add(schedule: &CronSchedule) -> Result<()> {
    if schedule.tz.is_some() && !matches!(schedule.kind, CronScheduleKind::Cron) {
        anyhow::bail!("tz can only be used with cron schedules");
    }

    match schedule.kind {
        CronScheduleKind::At => {
            if schedule.at_ms.is_none() {
                anyhow::bail!("at schedule requires at_ms");
            }
        }
        CronScheduleKind::Every => {
            if schedule.every_ms.unwrap_or_default() <= 0 {
                anyhow::bail!("every schedule requires every_ms > 0");
            }
        }
        CronScheduleKind::Cron => {
            let expr = schedule.expr.as_deref().unwrap_or_default().trim();
            if expr.is_empty() {
                anyhow::bail!("cron schedule requires expr");
            }
            let _ =
                Schedule::from_str(expr).with_context(|| format!("invalid cron expr: {}", expr))?;
            if let Some(tz) = &schedule.tz {
                let _: Tz = tz
                    .parse()
                    .with_context(|| format!("unknown timezone '{}'", tz))?;
            }
        }
    }

    Ok(())
}

fn compute_next_run(schedule: &CronSchedule, now_ms: i64) -> Option<i64> {
    match schedule.kind {
        CronScheduleKind::At => schedule
            .at_ms
            .and_then(|ts| if ts > now_ms { Some(ts) } else { None }),
        CronScheduleKind::Every => schedule
            .every_ms
            .and_then(|ms| if ms > 0 { Some(now_ms + ms) } else { None }),
        CronScheduleKind::Cron => {
            let expr = schedule.expr.as_deref()?.trim();
            if expr.is_empty() {
                return None;
            }
            let parsed = Schedule::from_str(expr).ok()?;

            if let Some(tz_name) = &schedule.tz {
                let tz: Tz = tz_name.parse().ok()?;
                let base = tz.timestamp_millis_opt(now_ms).single()?;
                parsed.after(&base).next().map(|dt| dt.timestamp_millis())
            } else {
                let base = Local.timestamp_millis_opt(now_ms).single()?;
                parsed.after(&base).next().map(|dt| dt.timestamp_millis())
            }
        }
    }
}

fn recompute_next_runs(jobs: &mut [CronJob]) {
    let now = now_ms();
    for job in jobs.iter_mut() {
        if job.enabled {
            job.state.next_run_at_ms = compute_next_run(&job.schedule, now);
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
            warn!("failed to load cron store '{}': {}", path.display(), err);
            (CronStore::default(), None)
        }
    }
}

fn read_store_file(path: &Path) -> Result<CronStore> {
    let raw = std::fs::read_to_string(path)
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
        async fn on_job(&self, _job: CronJob) -> Result<Option<String>> {
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
        let err = validate_schedule_for_add(&schedule)
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
        assert_eq!(compute_next_run(&at, now), Some(now + 5_000));

        let every = CronSchedule {
            kind: CronScheduleKind::Every,
            every_ms: Some(30_000),
            ..CronSchedule::default()
        };
        assert_eq!(compute_next_run(&every, now), Some(now + 30_000));

        let cron = CronSchedule {
            kind: CronScheduleKind::Cron,
            expr: Some("*/5 * * * * * *".to_string()),
            ..CronSchedule::default()
        };
        let next = compute_next_run(&cron, now).expect("cron schedule should compute next run");
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
                "test-job".to_string(),
                schedule,
                "hello".to_string(),
                true,
                Some("cli".to_string()),
                Some("direct".to_string()),
                false,
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
                "cb-job".to_string(),
                schedule,
                "hello".to_string(),
                true,
                Some("cli".to_string()),
                Some("direct".to_string()),
                false,
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
