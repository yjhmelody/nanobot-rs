use anyhow::{Context, Result};
use chrono::{Local, TimeZone, Utc};
use chrono_tz::Tz;
use cron::Schedule;
use serde::{Deserialize, Serialize};
use std::str::FromStr;

/// Supported schedule kinds for cron jobs.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CronScheduleKind {
    At,
    Every,
    Cron,
}

/// Schedule definition for cron jobs (at/every/cron).
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

impl CronSchedule {
    /// Validates the schedule fields before adding a new cron job.
    ///
    /// Returns an error if required fields are missing or inconsistent
    /// (e.g. `tz` supplied for a non-cron schedule, missing `at_ms` for an `At` schedule).
    pub fn validate_for_add(&self) -> Result<()> {
        if self.tz.is_some() && !matches!(self.kind, CronScheduleKind::Cron) {
            anyhow::bail!("tz can only be used with cron schedules");
        }

        match self.kind {
            CronScheduleKind::At => {
                if self.at_ms.is_none() {
                    anyhow::bail!("at schedule requires at_ms");
                }
            }
            CronScheduleKind::Every => {
                if self.every_ms.unwrap_or_default() <= 0 {
                    anyhow::bail!("every schedule requires every_ms > 0");
                }
            }
            CronScheduleKind::Cron => {
                let expr = self.expr.as_deref().unwrap_or_default().trim();
                if expr.is_empty() {
                    anyhow::bail!("cron schedule requires expr");
                }
                let _ = Schedule::from_str(expr)
                    .with_context(|| format!("invalid cron expr: {}", expr))?;
                if let Some(tz) = &self.tz {
                    let _: Tz = tz
                        .parse()
                        .with_context(|| format!("unknown timezone '{}'", tz))?;
                }
            }
        }

        Ok(())
    }

    pub fn compute_next_run(&self, now_ms: i64) -> Option<i64> {
        match self.kind {
            CronScheduleKind::At => self
                .at_ms
                .and_then(|ts| if ts > now_ms { Some(ts) } else { None }),
            CronScheduleKind::Every => self
                .every_ms
                .and_then(|ms| if ms > 0 { Some(now_ms + ms) } else { None }),
            CronScheduleKind::Cron => {
                let expr = self.expr.as_deref()?.trim();
                if expr.is_empty() {
                    return None;
                }
                let parsed = Schedule::from_str(expr).ok()?;

                if let Some(tz_name) = &self.tz {
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
}

/// Payload for cron job execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CronPayload {
    /// Payload kind (e.g., agent_turn).
    pub kind: String,
    /// Message content to deliver or execute.
    pub message: String,
    /// Whether to deliver message through channels.
    pub deliver: bool,
    /// Optional target channel override.
    pub channel: Option<String>,
    /// Optional target recipient identifier.
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

/// Runtime state for a cron job's execution history.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CronJobState {
    /// Next scheduled run time in unix ms.
    pub next_run_at_ms: Option<i64>,
    /// Last run time in unix ms.
    pub last_run_at_ms: Option<i64>,
    /// Last execution status string.
    pub last_status: Option<String>,
    /// Last execution error message, if any.
    pub last_error: Option<String>,
}

/// Stored cron job configuration and state.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CronJob {
    /// Unique job identifier.
    pub id: String,
    /// Human-readable job name.
    pub name: String,
    /// Whether the job is enabled.
    pub enabled: bool,
    /// Schedule configuration for the job.
    pub schedule: CronSchedule,
    /// Payload to execute on each run.
    pub payload: CronPayload,
    /// Runtime state for last/next runs.
    pub state: CronJobState,
    /// Creation time in unix ms.
    pub created_at_ms: i64,
    /// Update time in unix ms.
    pub updated_at_ms: i64,
    /// Delete after a successful run (one-shot).
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

/// On-disk cron store format.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CronStore {
    pub version: i64,
    pub jobs: Vec<CronJob>,
}

impl Default for CronStore {
    fn default() -> Self {
        Self {
            version: 1,
            jobs: Vec::new(),
        }
    }
}

/// Snapshot of cron service status.
#[derive(Debug, Clone, Serialize)]
pub struct CronStatus {
    /// Whether the cron service is enabled.
    pub enabled: bool,
    /// Number of registered jobs.
    pub jobs: usize,
    /// Next wake time in unix ms, if scheduled.
    pub next_wake_at_ms: Option<i64>,
}

/// Returns the current time as Unix milliseconds (UTC).
pub fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}
