use std::sync::Arc;
use std::sync::OnceLock;

use async_trait::async_trait;
use chrono::{DateTime, Local, NaiveDateTime, TimeZone, Utc};
use serde_json::json;

use crate::cron::{AddJobParams, CronSchedule, CronScheduleKind, CronService};
use crate::error::{NanobotError, Result};
use crate::tools::base::{
    Tool, ToolContext, ToolDefinition, parse_args, tool_definition_from_json,
};
use crate::types::tools::{CronAction, CronArgs};

pub struct CronTool {
    service: Arc<CronService>,
}

impl CronTool {
    pub fn new(service: Arc<CronService>) -> Self {
        Self { service }
    }

    pub fn definition() -> ToolDefinition {
        static DEF: OnceLock<ToolDefinition> = OnceLock::new();
        DEF.get_or_init(|| {
            tool_definition_from_json(json!({
                "type": "function",
                "function": {
                    "name": "cron",
                    "description": "Schedule reminders and recurring tasks. Actions: add, once, list, remove.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "action": {
                                "type": "string",
                                "description": "Action to perform",
                                "enum": ["add", "once", "list", "remove"]
                            },
                            "message": {
                                "type": "string",
                                "description": "Reminder message (for add)"
                            },
                            "every_seconds": {
                                "type": "integer",
                                "description": "Interval in seconds (for recurring tasks)"
                            },
                            "cron_expr": {
                                "type": "string",
                                "description": "Cron expression like '0 9 * * *' (for scheduled tasks)"
                            },
                            "tz": {
                                "type": "string",
                                "description": "IANA timezone for cron expressions (e.g. 'America/Vancouver')"
                            },
                            "at": {
                                "type": "string",
                                "description": "ISO datetime for one-time execution (e.g. '2026-02-12T10:30:00')"
                            },
                            "job_id": {
                                "type": "string",
                                "description": "Job ID (for remove)"
                            }
                        },
                        "required": ["action"]
                    }
                }
            }))
        })
        .clone()
    }

    pub(crate) async fn execute_typed(&self, args: CronArgs, ctx: &ToolContext) -> Result<String> {
        match args.action {
            CronAction::Add => {
                let message = args.message.unwrap_or_default();
                if message.trim().is_empty() {
                    return Err(NanobotError::invalid_tool_args(
                        "cron",
                        "message is required for add",
                    ));
                }

                if ctx.channel.trim().is_empty() || ctx.chat_id.trim().is_empty() {
                    return Err(NanobotError::tool_execution(
                        "cron",
                        anyhow::anyhow!("no session context (channel/chat_id)"),
                    ));
                }

                let every_seconds = args.every_seconds;
                let cron_expr = args.cron_expr;
                let tz = args.tz;
                let at = args.at;

                if tz.is_some() && cron_expr.is_none() {
                    return Err(NanobotError::invalid_tool_args(
                        "cron",
                        "tz can only be used with cron_expr",
                    ));
                }

                let mut delete_after = false;
                let schedule = if let Some(sec) = every_seconds {
                    if sec <= 0 {
                        return Err(NanobotError::invalid_tool_args(
                            "cron",
                            "every_seconds must be > 0",
                        ));
                    }
                    CronSchedule {
                        kind: CronScheduleKind::Every,
                        every_ms: Some(sec * 1000),
                        ..CronSchedule::default()
                    }
                } else if let Some(expr) = cron_expr {
                    CronSchedule {
                        kind: CronScheduleKind::Cron,
                        expr: Some(expr),
                        tz,
                        ..CronSchedule::default()
                    }
                } else if let Some(at_value) = at {
                    let at_ms = parse_at_to_ms(&at_value)?;
                    delete_after = true;
                    CronSchedule {
                        kind: CronScheduleKind::At,
                        at_ms: Some(at_ms),
                        ..CronSchedule::default()
                    }
                } else {
                    return Err(NanobotError::invalid_tool_args(
                        "cron",
                        "either every_seconds, cron_expr, or at is required",
                    ));
                };

                let name = if message.len() > 30 {
                    message[..30].to_string()
                } else {
                    message.clone()
                };

                match self
                    .service
                    .add_job(
                        AddJobParams::new(name, schedule, message)
                            .with_deliver(true)
                            .with_channel(ctx.channel.clone())
                            .with_to(ctx.chat_id.clone())
                            .with_delete_after_run(delete_after),
                    )
                    .await
                {
                    Ok(job) => Ok(format!("Created job '{}' (id: {})", job.name, job.id)),
                    Err(err) => Err(NanobotError::tool_execution("cron", err)),
                }
            }
            CronAction::Once => {
                let message = args.message.unwrap_or_default();
                if message.trim().is_empty() {
                    return Err(NanobotError::invalid_tool_args(
                        "cron",
                        "message is required for once",
                    ));
                }

                if ctx.channel.trim().is_empty() || ctx.chat_id.trim().is_empty() {
                    return Err(NanobotError::tool_execution(
                        "cron",
                        anyhow::anyhow!("no session context (channel/chat_id)"),
                    ));
                }

                if args.every_seconds.is_some() || args.cron_expr.is_some() || args.tz.is_some() {
                    return Err(NanobotError::invalid_tool_args(
                        "cron",
                        "once only supports optional at",
                    ));
                }

                let at_ms = match args.at {
                    Some(at_value) => parse_at_to_ms(&at_value)?,
                    None => Utc::now().timestamp_millis() + 1000,
                };
                let schedule = CronSchedule {
                    kind: CronScheduleKind::At,
                    at_ms: Some(at_ms),
                    ..CronSchedule::default()
                };

                let name = if message.len() > 30 {
                    message[..30].to_string()
                } else {
                    message.clone()
                };

                match self
                    .service
                    .add_job(
                        AddJobParams::new(name, schedule, message)
                            .with_deliver(true)
                            .with_channel(ctx.channel.clone())
                            .with_to(ctx.chat_id.clone())
                            .with_delete_after_run(true),
                    )
                    .await
                {
                    Ok(job) => Ok(format!("Created job '{}' (id: {})", job.name, job.id)),
                    Err(err) => Err(NanobotError::tool_execution("cron", err)),
                }
            }
            CronAction::List => match self.service.list_jobs(false).await {
                Ok(jobs) => {
                    if jobs.is_empty() {
                        Ok("No scheduled jobs.".to_string())
                    } else {
                        let lines = jobs
                            .iter()
                            .map(|j| {
                                let kind = match j.schedule.kind {
                                    CronScheduleKind::At => "at",
                                    CronScheduleKind::Every => "every",
                                    CronScheduleKind::Cron => "cron",
                                };
                                format!("- {} (id: {}, {})", j.name, j.id, kind)
                            })
                            .collect::<Vec<_>>();
                        Ok(format!("Scheduled jobs:\n{}", lines.join("\n")))
                    }
                }
                Err(err) => Err(NanobotError::tool_execution("cron", err)),
            },
            CronAction::Remove => {
                let Some(job_id) = args.job_id else {
                    return Err(NanobotError::invalid_tool_args(
                        "cron",
                        "job_id is required for remove",
                    ));
                };

                match self.service.remove_job(&job_id).await {
                    Ok(true) => Ok(format!("Removed job {}", job_id)),
                    Ok(false) => Ok(format!("Job {} not found", job_id)),
                    Err(err) => Err(NanobotError::tool_execution("cron", err)),
                }
            }
        }
    }
}

#[async_trait]
impl Tool for CronTool {
    fn name(&self) -> &str {
        "cron"
    }

    fn definition(&self) -> ToolDefinition {
        Self::definition()
    }

    async fn execute(&self, args_json: &str, ctx: &ToolContext) -> Result<String> {
        let parsed = parse_args::<CronArgs>(args_json)?;
        self.execute_typed(parsed, ctx).await
    }
}

fn parse_at_to_ms(input: &str) -> Result<i64> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(input) {
        return Ok(dt.timestamp_millis());
    }

    for fmt in ["%Y-%m-%dT%H:%M:%S", "%Y-%m-%d %H:%M:%S"] {
        if let Ok(naive) = NaiveDateTime::parse_from_str(input, fmt) {
            if let Some(local_dt) = Local.from_local_datetime(&naive).single() {
                return Ok(local_dt.timestamp_millis());
            }
        }
    }

    Err(NanobotError::invalid_tool_args(
        "cron",
        "invalid at datetime, expected ISO format like 2026-02-12T10:30:00",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::SessionKey;

    fn temp_store_path(case: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "nanobot-rs-tool-cron-{}-{}.json",
            case,
            uuid::Uuid::new_v4()
        ))
    }

    #[test]
    fn parse_at_accepts_rfc3339() {
        let ms =
            parse_at_to_ms("2026-02-12T10:30:00+00:00").expect("rfc3339 datetime should be parsed");
        assert!(ms > 0);
    }

    #[test]
    fn parse_at_rejects_invalid_input() {
        let err = parse_at_to_ms("not-a-time").expect_err("invalid datetime should fail");
        assert!(err.to_string().contains("invalid at datetime"));
    }

    #[tokio::test]
    async fn add_list_remove_flow_works() {
        let path = temp_store_path("flow");
        let service = Arc::new(CronService::new(path.clone()));
        let tool = CronTool::new(service.clone());

        let ctx = ToolContext {
            channel: "cli".to_string(),
            chat_id: "direct".to_string(),
            session_key: SessionKey::from("cli:direct"),
            message_id: None,
        };

        let add_args: CronArgs =
            parse_args(r#"{"action":"add","message":"take a break","every_seconds":60}"#)
                .expect("parse add args");
        let added = tool.execute_typed(add_args, &ctx).await.expect("add cron");
        assert!(added.starts_with("Created job"));

        let list_args: CronArgs = parse_args(r#"{"action":"list"}"#).expect("parse list args");
        let listed = tool
            .execute_typed(list_args, &ctx)
            .await
            .expect("list cron");
        assert!(listed.contains("Scheduled jobs:"));
        assert!(listed.contains("take a break"));

        let jobs = service.list_jobs(false).await.expect("list jobs");
        let id = jobs[0].id.clone();

        let remove_json = format!(r#"{{"action":"remove","job_id":"{}"}}"#, id);
        let remove_args: CronArgs = parse_args(&remove_json).expect("parse remove args");
        let removed = tool
            .execute_typed(remove_args, &ctx)
            .await
            .expect("remove cron");
        assert!(removed.starts_with("Removed job"));

        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn once_action_creates_one_time_job() {
        let path = temp_store_path("once");
        let service = Arc::new(CronService::new(path.clone()));
        let tool = CronTool::new(service.clone());

        let ctx = ToolContext {
            channel: "cli".to_string(),
            chat_id: "direct".to_string(),
            session_key: SessionKey::from("cli:direct"),
            message_id: None,
        };

        let once_args: CronArgs =
            parse_args(r#"{"action":"once","message":"do it once"}"#).expect("parse once args");
        let created = tool
            .execute_typed(once_args, &ctx)
            .await
            .expect("create once cron");
        assert!(created.starts_with("Created job"));

        let jobs = service.list_jobs(false).await.expect("list jobs");
        assert_eq!(jobs.len(), 1);
        assert!(matches!(jobs[0].schedule.kind, CronScheduleKind::At));
        assert!(jobs[0].delete_after_run);
        assert!(jobs[0].state.next_run_at_ms.is_some());

        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn once_action_rejects_recurring_fields() {
        let path = temp_store_path("once-invalid");
        let service = Arc::new(CronService::new(path.clone()));
        let tool = CronTool::new(service);

        let ctx = ToolContext {
            channel: "cli".to_string(),
            chat_id: "direct".to_string(),
            session_key: SessionKey::from("cli:direct"),
            message_id: None,
        };

        let once_args: CronArgs =
            parse_args(r#"{"action":"once","message":"x","every_seconds":30}"#)
                .expect("parse once args");
        let err = tool
            .execute_typed(once_args, &ctx)
            .await
            .expect_err("once with recurring field should fail");
        assert!(err.to_string().contains("once only supports optional at"));

        let _ = std::fs::remove_file(path);
    }
}
