//! Cron job scheduling types.
//!
//! This module defines the data model for scheduled (cron) jobs in nanobot:
//! schedule definitions, payloads, runtime state, and persistent store format.
//!
//! # Schedule kinds
//!
//! Three schedule modes are supported:
//!
//! - **At** — Run once at a specific Unix timestamp (ms).
//! - **Every** — Run repeatedly at a fixed interval (ms).
//! - **Cron** — Run according to a standard cron expression, with optional
//!   IANA timezone support.
//!
//! # Design
//!
//! - Schedule computation is done via the `cron` crate (for cron expressions)
//!   and `chrono`/`chrono-tz` (for timezone-aware scheduling).
//! - The [`CronSchedule`] struct uses `Option` fields for each schedule kind,
//!   with validation in [`CronSchedule::validate_for_add`] to catch missing
//!   or inconsistent fields.
//! - [`CronStore`] is the on-disk format, versioned for forwards compatibility.

use anyhow::{Context, Result};
use chrono::{Local, TimeZone, Utc};
use chrono_tz::Tz;
use cron::Schedule;
use serde::{Deserialize, Serialize};
use std::str::FromStr;

/// Supported schedule kinds for cron jobs.
///
/// Each variant corresponds to a different scheduling strategy:
///
/// | Variant | Strategy | Example |
/// |---------|----------|---------|
/// | `At` | One-shot at a specific timestamp | "run at 2026-12-01T00:00:00Z" |
/// | `Every` | Fixed-rate recurring | "run every 30 minutes" |
/// | `Cron` | Cron-expression-based | "run at 9 AM weekdays" |
///
/// # Derive rationale
///
/// - `Clone + Copy`: schedule kind is tiny and passed by value in scheduling logic.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CronScheduleKind {
    At,
    Every,
    Cron,
}

/// Schedule definition for a cron job.
///
/// Exactly one of `kind` determines which schedule mode is active. The
/// remaining fields must be populated according to the kind:
///
/// | `kind` | Required fields |
/// |--------|----------------|
/// | `At` | `at_ms` |
/// | `Every` | `every_ms > 0` |
/// | `Cron` | `expr` (valid cron), optionally `tz` |
///
/// # Validation
///
/// Call [`validate_for_add`](CronSchedule::validate_for_add) before storing
/// a new schedule to ensure consistency.
///
/// # Default
///
/// Defaults to [`Every`](CronScheduleKind::Every) with no interval, which
/// will fail validation until `every_ms` is set.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CronSchedule {
    /// One of `at`, `every`, `cron`.
    pub kind: CronScheduleKind,
    /// Unix ms timestamp for one-shot (At) schedules.
    pub at_ms: Option<i64>,
    /// Interval in milliseconds for fixed-rate (Every) schedules.
    pub every_ms: Option<i64>,
    /// Cron expression string (e.g. `"0 9 * * 1-5"`) when `kind == Cron`.
    pub expr: Option<String>,
    /// IANA timezone string (e.g. `"Asia/Shanghai"`) for cron expressions.
    /// Only valid when `kind == Cron`.
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
    /// Validates the schedule fields before adding or updating a cron job.
    ///
    /// Returns an error if:
    /// - `tz` is supplied for a non-cron schedule.
    /// - `At` schedule is missing `at_ms`.
    /// - `Every` schedule has `every_ms <= 0`.
    /// - `Cron` schedule has an empty `expr` or an invalid cron expression.
    /// - `Cron` schedule has an unrecognised IANA timezone in `tz`.
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

    /// Computes the next run time (in Unix ms) for this schedule, given the
    /// current time `now_ms`.
    ///
    /// Returns `None` if:
    /// - The schedule is [`At`](CronScheduleKind::At) and its timestamp has
    ///   already passed.
    /// - The schedule is [`Every`](CronScheduleKind::Every) with an invalid
    ///   or zero interval.
    /// - The schedule is [`Cron`](CronScheduleKind::Cron) and the expression
    ///   or timezone cannot be parsed.
    ///
    /// # Timezone handling
    ///
    /// For cron schedules with a `tz` field, the computation uses the
    /// specified IANA timezone. Otherwise, the local system timezone is used.
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

/// Payload executed when a cron job fires.
///
/// Defines what action to take and where to deliver the result.
///
/// # Fields
///
/// * `kind` — Payload type (e.g., `"agent_turn"` triggers the agent loop
///   with the given message).
/// * `message` — Message content to deliver or execute.
/// * `deliver` — If `true`, the message is also sent through the channel
///   (broadcast). If `false`, it is processed internally without delivery.
/// * `channel` — Optional override for the target channel.
/// * `to` — Optional override for the target recipient/chat ID.
///
/// # Default
///
/// Defaults to `agent_turn` kind with an empty message and no delivery.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CronPayload {
    /// Payload kind (e.g., `agent_turn`).
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

/// Runtime execution state for a cron job.
///
/// Tracks the last and next run times along with execution status,
/// updated by the cron service after each job execution.
///
/// # Serde notes
///
/// All fields use `#[serde(default)]` so that a newly-created job has
/// an empty state.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CronJobState {
    /// Next scheduled run time in Unix ms.
    pub next_run_at_ms: Option<i64>,
    /// Last run time in Unix ms.
    pub last_run_at_ms: Option<i64>,
    /// Last execution status string (e.g., `"success"`, `"running"`).
    pub last_status: Option<String>,
    /// Last execution error message, if any.
    pub last_error: Option<String>,
}

/// A single cron job with its configuration and runtime state.
///
/// Each job has a unique `id`, a human-readable `name`, a schedule
/// ([`CronSchedule`]), and a [`CronPayload`] that defines what runs.
/// The `state` field is updated by the cron service as the job executes.
///
/// # One-shot jobs
///
/// When `delete_after_run` is `true`, the job is removed from the store
/// after its first successful execution. This is used for `at`-type
/// one-shot schedules.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CronJob {
    /// Unique job identifier.
    pub id: String,
    /// Human-readable job name.
    pub name: String,
    /// Whether the job is enabled (disabled jobs are skipped during scheduling).
    pub enabled: bool,
    /// Schedule configuration for the job.
    pub schedule: CronSchedule,
    /// Payload to execute on each run.
    pub payload: CronPayload,
    /// Runtime state tracking last/next runs.
    pub state: CronJobState,
    /// Creation time in Unix ms.
    pub created_at_ms: i64,
    /// Last update time in Unix ms.
    pub updated_at_ms: i64,
    /// If `true`, the job is deleted after a successful run (one-shot).
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

/// On-disk serialization format for the cron job store.
///
/// The `version` field allows schema evolution. Currently at version `1`.
///
/// # Serde notes
///
/// `#[serde(default)]` on the struct ensures that missing fields in an
/// existing store file deserialise to sensible defaults.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CronStore {
    /// Schema version for forwards compatibility.
    pub version: i64,
    /// List of registered cron jobs.
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

/// A snapshot of the cron service's current status, used for monitoring
/// and reporting.
///
/// Serialised but never deserialised (read-only status view).
#[derive(Debug, Clone, Serialize)]
pub struct CronStatus {
    /// Whether the cron service is enabled and running.
    pub enabled: bool,
    /// Number of registered cron jobs.
    pub jobs: usize,
    /// Next wake time in Unix ms, if any job is scheduled.
    pub next_wake_at_ms: Option<i64>,
}

/// Returns the current UTC time as Unix milliseconds.
///
/// Utility function used throughout the cron module for consistent
/// time handling.
pub fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}
