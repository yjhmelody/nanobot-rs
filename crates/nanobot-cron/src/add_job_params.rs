//! Parameters for creating a new cron job.
//!
//! This module defines [`AddJobParams`], a builder-style parameter object used to
//! construct a new `CronJob`. It is consumed by [`CronService::add_job`].

use nanobot_types::cron::CronSchedule;

/// Parameters for adding a cron job.
///
/// This struct uses a builder pattern — call [`AddJobParams::new`] with the required
/// fields, then chain `.with_*()` methods to set optional fields. Default values:
/// `deliver = false`, `delete_after_run = false`, `channel = None`, `to = None`.
///
/// # Examples
///
/// ```ignore
/// use nanobot_cron::AddJobParams;
/// use nanobot_types::cron::{CronSchedule, CronScheduleKind};
///
/// let schedule = CronSchedule {
///     kind: CronScheduleKind::Every,
///     every_ms: Some(3600000), // every hour
///     ..CronSchedule::default()
/// };
/// let params = AddJobParams::new("reminder".into(), schedule, "time to stand up".into())
///     .with_deliver(true)
///     .with_channel("slack".into())
///     .with_to("#general".into());
/// ```
#[derive(Debug, Clone)]
pub struct AddJobParams {
    /// Human-readable name for the job (used in logs and status).
    pub name: String,
    /// The scheduling rule (at, every, or cron).
    pub schedule: CronSchedule,
    /// The message payload to send when the job fires.
    pub message: String,
    /// Whether to actively deliver the message to a channel (vs. recording it).
    pub deliver: bool,
    /// Optional target channel name (e.g., `"slack"`, `"cli"`).
    pub channel: Option<String>,
    /// Optional recipient identifier (e.g., user ID or channel name).
    pub to: Option<String>,
    /// If `true`, the job is removed from the store after its first execution.
    /// Only meaningful for `CronScheduleKind::At` jobs.
    pub delete_after_run: bool,
}

impl AddJobParams {
    /// Creates a new `AddJobParams` with the required fields.
    ///
    /// Optional fields default to `false` / `None` and can be overridden with the
    /// builder methods below.
    ///
    /// # Arguments
    ///
    /// * `name` - A human-readable label for the job.
    /// * `schedule` - When/how-often the job should fire.
    /// * `message` - The content to deliver when the job fires.
    pub fn new(name: String, schedule: CronSchedule, message: String) -> Self {
        Self {
            name,
            schedule,
            message,
            deliver: false,
            channel: None,
            to: None,
            delete_after_run: false,
        }
    }

    /// Sets whether to actively deliver the message to a channel.
    ///
    /// When `true`, the cron service will attempt to send the message through the
    /// configured channel adapter. When `false`, the message is recorded but not sent.
    pub fn with_deliver(mut self, deliver: bool) -> Self {
        self.deliver = deliver;
        self
    }

    /// Sets the channel identifier to deliver the message to.
    ///
    /// This identifies which channel adapter to use (e.g., `"cli"`, `"slack"`,
    /// `"telegram"`).
    pub fn with_channel(mut self, channel: String) -> Self {
        self.channel = Some(channel);
        self
    }

    /// Sets the recipient for the message.
    ///
    /// The interpretation depends on the channel (e.g., a user ID for Telegram,
    /// a channel name for Slack).
    pub fn with_to(mut self, to: String) -> Self {
        self.to = Some(to);
        self
    }

    /// Sets whether the job should be removed from the store after its first run.
    ///
    /// This is most useful for one-shot (`At`) schedules that should not persist
    /// after execution.
    pub fn with_delete_after_run(mut self, delete: bool) -> Self {
        self.delete_after_run = delete;
        self
    }
}

// Unit tests for `AddJobParams` builder methods.
#[cfg(test)]
mod tests {
    use super::*;
    use nanobot_types::cron::CronScheduleKind;

    /// Verifies that the builder chain produces the expected field values.
    #[test]
    fn add_job_params_builder_works() {
        let schedule = CronSchedule {
            kind: CronScheduleKind::Every,
            at_ms: None,
            every_ms: Some(3600000), // 1 hour
            expr: None,
            tz: None,
        };

        let params = AddJobParams::new("test".to_string(), schedule, "message".to_string())
            .with_deliver(true)
            .with_channel("telegram".to_string())
            .with_to("123456".to_string())
            .with_delete_after_run(true);

        assert_eq!(params.name, "test");
        assert_eq!(params.message, "message");
        assert!(params.deliver);
        assert_eq!(params.channel, Some("telegram".to_string()));
        assert_eq!(params.to, Some("123456".to_string()));
        assert!(params.delete_after_run);
    }
}
