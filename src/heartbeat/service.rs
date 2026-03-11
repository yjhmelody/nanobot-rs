use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;
use tracing::{error, info};

use crate::observability::TARGET_HEARTBEAT;
use crate::provider::{ChatMessage, ChatRequest, LLMProvider};
use crate::tools::base::{JsonSchema, ToolDefinition, parse_args};
use crate::types::SessionKey;
use crate::types::heartbeat::HeartbeatDecisionArgs;

#[async_trait]
pub trait HeartbeatExecuteHandler: Send + Sync {
    async fn on_execute(&self, tasks: String) -> Result<String>;
}

#[async_trait]
pub trait HeartbeatNotifyHandler: Send + Sync {
    async fn on_notify(&self, response: String);
}

/// Periodic heartbeat evaluator.
///
/// It reads `HEARTBEAT.md`, asks the model whether work should run, and
/// delegates execution/notification through registered handlers.
pub struct HeartbeatService {
    workspace: PathBuf,
    provider: Arc<dyn LLMProvider>,
    model: String,
    interval_s: u64,
    enabled: bool,
    running: AtomicBool,
    task: Mutex<Option<JoinHandle<()>>>,
    on_execute: RwLock<Option<Arc<dyn HeartbeatExecuteHandler>>>,
    on_notify: RwLock<Option<Arc<dyn HeartbeatNotifyHandler>>>,
}

impl HeartbeatService {
    pub fn new(
        workspace: PathBuf,
        provider: Arc<dyn LLMProvider>,
        model: String,
        interval_s: u64,
        enabled: bool,
    ) -> Self {
        Self {
            workspace,
            provider,
            model,
            interval_s,
            enabled,
            running: AtomicBool::new(false),
            task: Mutex::new(None),
            on_execute: RwLock::new(None),
            on_notify: RwLock::new(None),
        }
    }

    pub async fn register_on_execute_handler(&self, handler: Arc<dyn HeartbeatExecuteHandler>) {
        *self.on_execute.write().await = Some(handler);
    }

    pub async fn register_on_notify_handler(&self, handler: Arc<dyn HeartbeatNotifyHandler>) {
        *self.on_notify.write().await = Some(handler);
    }

    pub async fn start(self: &Arc<Self>) {
        if !self.enabled {
            info!(target: TARGET_HEARTBEAT, "heartbeat disabled");
            return;
        }
        if self.running.swap(true, Ordering::SeqCst) {
            return;
        }

        let this = self.clone();
        let handle = tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(this.interval_s)).await;
                if !this.running.load(Ordering::SeqCst) {
                    break;
                }
                if let Err(err) = this.tick().await {
                    error!(target: TARGET_HEARTBEAT, "heartbeat tick failed: {}", err);
                }
            }
        });

        *self.task.lock().await = Some(handle);
        info!(
            target: TARGET_HEARTBEAT,
            "heartbeat started (every {}s)",
            self.interval_s
        );
    }

    pub async fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(task) = self.task.lock().await.take() {
            task.abort();
        }
    }

    pub async fn trigger_now(&self) -> Option<String> {
        // Manual trigger path used by CLI/gateway control flow.
        let content = self.read_heartbeat_file()?;
        let (action, tasks) = match self.decide(&content).await {
            Ok(v) => v,
            Err(err) => {
                error!(target: TARGET_HEARTBEAT, "heartbeat decide failed: {}", err);
                return None;
            }
        };
        if action != "run" || tasks.trim().is_empty() {
            return None;
        }

        let handler = self.on_execute.read().await.clone()?;
        let response = match handler.on_execute(tasks).await {
            Ok(v) => v,
            Err(err) => {
                error!(target: TARGET_HEARTBEAT, "heartbeat execute failed: {}", err);
                return None;
            }
        };
        if response.trim().is_empty() {
            None
        } else {
            Some(response)
        }
    }

    async fn tick(&self) -> anyhow::Result<()> {
        // Background loop path; same decision pipeline with optional notify callback.
        let Some(content) = self.read_heartbeat_file() else {
            return Ok(());
        };

        let (action, tasks) = self.decide(&content).await?;
        if action != "run" || tasks.trim().is_empty() {
            return Ok(());
        }

        let execute_handler = self.on_execute.read().await.clone();
        let Some(execute_handler) = execute_handler else {
            return Ok(());
        };

        let response = execute_handler.on_execute(tasks).await?;
        if response.trim().is_empty() {
            return Ok(());
        }

        if let Some(notify_handler) = self.on_notify.read().await.clone() {
            notify_handler.on_notify(response).await;
        }

        Ok(())
    }

    fn read_heartbeat_file(&self) -> Option<String> {
        let path = self.workspace.join("HEARTBEAT.md");
        if !path.exists() {
            return None;
        }
        std::fs::read_to_string(path)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    async fn decide(&self, content: &str) -> Result<(String, String)> {
        // Ask model for a structured decision via a tool-call schema.
        let mut props = BTreeMap::new();
        props.insert(
            "action".to_string(),
            JsonSchema::string(None).with_enum(vec!["skip", "run"]),
        );
        props.insert("tasks".to_string(), JsonSchema::string(None));
        let tool = ToolDefinition::function(
            "heartbeat",
            "Report heartbeat decision after reviewing tasks.",
            JsonSchema::object(props, vec!["action"]),
        );

        let response = self
            .provider
            .chat(ChatRequest {
                session_key: Some(SessionKey::from("system:heartbeat")),
                messages: vec![
                    ChatMessage::system_text(
                        "You are a heartbeat agent. Call the heartbeat tool to report your decision.",
                    ),
                    ChatMessage::user_text(format!(
                        "Review the following HEARTBEAT.md and decide whether there are active tasks.\n\n{}",
                        content
                    )),
                ],
                tools: Some(vec![tool]),
                model: Some(self.model.clone()),
                max_tokens: 512,
                temperature: 0.0,
                reasoning_effort: None,
            })
            .await;

        if !response.has_tool_calls() {
            return Ok(("skip".to_string(), String::new()));
        }

        let parsed: HeartbeatDecisionArgs = parse_args(&response.tool_calls[0].arguments_json)?;

        Ok((parsed.action, parsed.tasks))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::types::provider::{LLMResponse, ToolCallRequest, UsageStats};

    struct StubProvider {
        responses: Mutex<Vec<LLMResponse>>,
    }

    impl StubProvider {
        fn new(responses: Vec<LLMResponse>) -> Self {
            Self {
                responses: Mutex::new(responses),
            }
        }
    }

    #[async_trait]
    impl LLMProvider for StubProvider {
        async fn chat(&self, _req: ChatRequest) -> LLMResponse {
            let mut responses = self.responses.lock().await;
            if responses.is_empty() {
                LLMResponse {
                    content: None,
                    tool_calls: Vec::new(),
                    finish_reason: "stop".to_string(),
                    usage: UsageStats::default(),
                    reasoning_content: None,
                    thinking_blocks: None,
                }
            } else {
                responses.remove(0)
            }
        }

        fn default_model(&self) -> &str {
            "openai/gpt-4o-mini"
        }
    }

    struct ExecRecorder {
        seen: Mutex<Vec<String>>,
        response: String,
    }

    #[async_trait]
    impl HeartbeatExecuteHandler for ExecRecorder {
        async fn on_execute(&self, tasks: String) -> Result<String> {
            self.seen.lock().await.push(tasks);
            Ok(self.response.clone())
        }
    }

    struct NotifyRecorder {
        seen: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl HeartbeatNotifyHandler for NotifyRecorder {
        async fn on_notify(&self, response: String) {
            self.seen.lock().await.push(response);
        }
    }

    fn temp_workspace(case: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "nanobot-rs-heartbeat-{}-{}",
            case,
            uuid::Uuid::new_v4()
        ))
    }

    fn run_decision(tasks: &str) -> LLMResponse {
        LLMResponse {
            content: None,
            tool_calls: vec![ToolCallRequest {
                id: "tc1".to_string(),
                name: "heartbeat".into(),
                arguments_json: format!(r#"{{"action":"run","tasks":"{}"}}"#, tasks),
            }],
            finish_reason: "tool_calls".to_string(),
            usage: UsageStats::default(),
            reasoning_content: None,
            thinking_blocks: None,
        }
    }

    fn skip_decision() -> LLMResponse {
        LLMResponse {
            content: None,
            tool_calls: vec![ToolCallRequest {
                id: "tc1".to_string(),
                name: "heartbeat".into(),
                arguments_json: r#"{"action":"skip","tasks":""}"#.to_string(),
            }],
            finish_reason: "tool_calls".to_string(),
            usage: UsageStats::default(),
            reasoning_content: None,
            thinking_blocks: None,
        }
    }

    #[tokio::test]
    async fn trigger_now_returns_none_when_heartbeat_file_missing() {
        let workspace = temp_workspace("missing");
        std::fs::create_dir_all(&workspace).expect("create workspace");
        let provider: Arc<dyn LLMProvider> = Arc::new(StubProvider::new(vec![run_decision("x")]));
        let service = HeartbeatService::new(
            workspace.clone(),
            provider,
            "openai/gpt-4o-mini".to_string(),
            60,
            true,
        );

        let out = service.trigger_now().await;
        assert!(out.is_none());

        let _ = std::fs::remove_dir_all(workspace);
    }

    #[tokio::test]
    async fn trigger_now_runs_execute_handler_on_run_decision() {
        let workspace = temp_workspace("run");
        std::fs::create_dir_all(&workspace).expect("create workspace");
        std::fs::write(workspace.join("HEARTBEAT.md"), "- check project")
            .expect("write heartbeat file");

        let provider: Arc<dyn LLMProvider> =
            Arc::new(StubProvider::new(vec![run_decision("finish docs")]));
        let service = HeartbeatService::new(
            workspace.clone(),
            provider,
            "openai/gpt-4o-mini".to_string(),
            60,
            true,
        );
        let exec = Arc::new(ExecRecorder {
            seen: Mutex::new(Vec::new()),
            response: "done".to_string(),
        });
        service.register_on_execute_handler(exec.clone()).await;

        let out = service.trigger_now().await;
        assert_eq!(out.as_deref(), Some("done"));

        let seen = exec.seen.lock().await.clone();
        assert_eq!(seen, vec!["finish docs".to_string()]);

        let _ = std::fs::remove_dir_all(workspace);
    }

    #[tokio::test]
    async fn tick_notifies_when_execute_returns_non_empty() {
        let workspace = temp_workspace("notify");
        std::fs::create_dir_all(&workspace).expect("create workspace");
        std::fs::write(workspace.join("HEARTBEAT.md"), "- task").expect("write heartbeat file");

        let provider: Arc<dyn LLMProvider> = Arc::new(StubProvider::new(vec![run_decision("t1")]));
        let service = HeartbeatService::new(
            workspace.clone(),
            provider,
            "openai/gpt-4o-mini".to_string(),
            60,
            true,
        );
        let exec = Arc::new(ExecRecorder {
            seen: Mutex::new(Vec::new()),
            response: "executed".to_string(),
        });
        let notify = Arc::new(NotifyRecorder {
            seen: Mutex::new(Vec::new()),
        });
        service.register_on_execute_handler(exec).await;
        service.register_on_notify_handler(notify.clone()).await;

        service.tick().await.expect("tick should succeed");

        let seen_notify = notify.seen.lock().await.clone();
        assert_eq!(seen_notify, vec!["executed".to_string()]);

        let _ = std::fs::remove_dir_all(workspace);
    }

    #[tokio::test]
    async fn trigger_now_returns_none_for_skip_decision() {
        let workspace = temp_workspace("skip");
        std::fs::create_dir_all(&workspace).expect("create workspace");
        std::fs::write(workspace.join("HEARTBEAT.md"), "- noop").expect("write heartbeat file");

        let provider: Arc<dyn LLMProvider> = Arc::new(StubProvider::new(vec![skip_decision()]));
        let service = HeartbeatService::new(
            workspace.clone(),
            provider,
            "openai/gpt-4o-mini".to_string(),
            60,
            true,
        );
        let exec = Arc::new(ExecRecorder {
            seen: Mutex::new(Vec::new()),
            response: "should-not-run".to_string(),
        });
        service.register_on_execute_handler(exec.clone()).await;

        let out = service.trigger_now().await;
        assert!(out.is_none());
        assert!(exec.seen.lock().await.is_empty());

        let _ = std::fs::remove_dir_all(workspace);
    }
}
