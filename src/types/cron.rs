use anyhow::{Context, Result};
use chrono::{Local, TimeZone, Utc};
use chrono_tz::Tz;
use cron::Schedule;
use serde::{Deserialize, Serialize};
use std::str::FromStr;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
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

impl CronSchedule {
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
pub(crate) struct CronStore {
    pub(crate) version: i64,
    pub(crate) jobs: Vec<CronJob>,
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

pub(crate) fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}
