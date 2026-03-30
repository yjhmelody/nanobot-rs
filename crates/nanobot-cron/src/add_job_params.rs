use nanobot_types::cron::CronSchedule;

/// Parameters for adding a cron job.
#[derive(Debug, Clone)]
pub struct AddJobParams {
    pub name: String,
    pub schedule: CronSchedule,
    pub message: String,
    pub deliver: bool,
    pub channel: Option<String>,
    pub to: Option<String>,
    pub delete_after_run: bool,
}

impl AddJobParams {
    /// Creates a new AddJobParams with required fields.
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

    /// Sets whether to deliver the message.
    pub fn with_deliver(mut self, deliver: bool) -> Self {
        self.deliver = deliver;
        self
    }

    /// Sets the channel to deliver to.
    pub fn with_channel(mut self, channel: String) -> Self {
        self.channel = Some(channel);
        self
    }

    /// Sets the recipient.
    pub fn with_to(mut self, to: String) -> Self {
        self.to = Some(to);
        self
    }

    /// Sets whether to delete the job after running.
    pub fn with_delete_after_run(mut self, delete: bool) -> Self {
        self.delete_after_run = delete;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nanobot_types::cron::CronScheduleKind;

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
