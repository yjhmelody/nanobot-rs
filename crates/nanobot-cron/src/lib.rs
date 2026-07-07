//! Crate-level re-exports for the cron scheduling subsystem.
//!
//! This crate provides a background cron scheduler that reads/writes a JSONL-based
//! store and fires registered callbacks when jobs become due. It supports three
//! schedule kinds:
//!
//! - **At**: fire once at a specific timestamp.
//! - **Every**: fire repeatedly at a fixed interval (in milliseconds).
//! - **Cron**: fire according to a cron expression (6- or 7-field format).
//!
//! ## Architecture
//!
//! The core types (`CronJob`, `CronSchedule`, etc.) live in `nanobot-types` and are
//! re-exported here for convenience. The scheduling logic lives in [`CronService`],
//! which runs a background 1-second ticker loop. Users register a callback via
//! [`CronJobHandler`] to react when a job fires.
//!
//! ## Persistence
//!
//! Jobs are persisted as a JSONL file at a configurable path. The store is reloaded
//! on each tick if the file's mtime has changed, allowing external tools to add or
//! remove jobs without restarting.

pub mod add_job_params;
pub mod error;
pub mod service;

/// Parameters for adding a new cron job. See [`AddJobParams`](add_job_params::AddJobParams).
pub use add_job_params::AddJobParams;

/// Error and result types for the cron subsystem. See [`CronError`](error::CronError).
pub use error::{CronError, CronResult};

/// Core job types re-exported from `nanobot-types::cron`.
pub use nanobot_types::cron::{
    CronJob, CronJobState, CronPayload, CronSchedule, CronScheduleKind, CronStatus,
};

/// The scheduler service and its callback trait. See [`CronService`](service::CronService)
/// and [`CronJobHandler`](service::CronJobHandler).
pub use service::{CronJobHandler, CronService};
