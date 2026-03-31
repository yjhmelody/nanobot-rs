use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::heartbeat::{HeartbeatError, HeartbeatResult};
use async_trait::async_trait;
use parking_lot::RwLock;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tracing::{error, info};

use nanobot_provider::{ChatMessage, ChatRequest, LLMProvider};
use nanobot_types::SessionKey;
use nanobot_types::heartbeat::HeartbeatDecisionArgs;

pub(crate) const LOG_TARGET: &str = "nanobot.heartbeat";

const HEARTBEAT_SYSTEM_PROMPT: &str = "You are a heartbeat agent. Review HEARTBEAT.md and reply with JSON only: {\"action\":\"run|skip\",\"tasks\":\"...\"}. Use action=skip and empty tasks when no active tasks exist.";
const HEARTBEAT_USER_PROMPT_PREFIX: &str =
    "Review the following HEARTBEAT.md and decide whether there are active tasks.\n\n";

#[async_trait]
pub trait HeartbeatExecuteHandler: Send + Sync {
    async fn on_execute(&self, tasks: String) -> HeartbeatResult<String>;
}

#[async_trait]
pub trait HeartbeatNotifyHandler: Send + Sync {
    async fn on_notify(&self, response: String);
}

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
        *self.on_execute.write() = Some(handler);
    }

    pub async fn register_on_notify_handler(&self, handler: Arc<dyn HeartbeatNotifyHandler>) {
        *self.on_notify.write() = Some(handler);
    }

    pub async fn start(self: &Arc<Self>) {
        if !self.enabled {
            info!(target: LOG_TARGET, "heartbeat disabled");
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
                    error!(target: LOG_TARGET, "heartbeat tick failed: {}", err);
                }
            }
        });

        *self.task.lock().await = Some(handle);
        info!(
            target: LOG_TARGET,
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

    #[allow(unused)]
    pub async fn trigger_now(&self) -> Option<String> {
        let content = self.read_heartbeat_file()?;
        let (action, tasks) = match self.decide(&content).await {
            Ok(v) => v,
            Err(err) => {
                error!(target: LOG_TARGET, "heartbeat decide failed: {}", err);
                return None;
            }
        };
        if action != "run" || tasks.trim().is_empty() {
            return None;
        }

        let handler = self.on_execute.read().clone()?;
        let response = match handler.on_execute(tasks).await {
            Ok(v) => v,
            Err(err) => {
                error!(target: LOG_TARGET, "heartbeat execute failed: {}", err);
                return None;
            }
        };
        if response.trim().is_empty() {
            None
        } else {
            Some(response)
        }
    }

    async fn tick(&self) -> HeartbeatResult<()> {
        let Some(content) = self.read_heartbeat_file() else {
            return Ok(());
        };

        let (action, tasks) = self.decide(&content).await?;
        if action != "run" || tasks.trim().is_empty() {
            return Ok(());
        }

        let execute_handler = self.on_execute.read().clone();
        let Some(execute_handler) = execute_handler else {
            return Ok(());
        };

        let response = execute_handler.on_execute(tasks).await?;
        if response.trim().is_empty() {
            return Ok(());
        }

        let notify_handler = self.on_notify.read().clone();
        if let Some(notify_handler) = notify_handler {
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

    async fn decide(&self, content: &str) -> HeartbeatResult<(String, String)> {
        let response = self
            .provider
            .chat(ChatRequest {
                session_key: Some(SessionKey::from("system:heartbeat")),
                messages: vec![
                    ChatMessage::system_text(HEARTBEAT_SYSTEM_PROMPT),
                    ChatMessage::user_text(format!("{HEARTBEAT_USER_PROMPT_PREFIX}{content}")),
                ],
                tools: None,
                model: Some(self.model.clone()),
                max_tokens: 512,
                temperature: 0.0,
                reasoning_effort: None,
            })
            .await?;

        let Some(content) = response.content.as_deref() else {
            return Ok(("skip".to_string(), String::new()));
        };

        let parsed = Self::parse_decision_response(content)?;
        Ok((parsed.action.trim().to_lowercase(), parsed.tasks))
    }

    fn parse_decision_response(content: &str) -> HeartbeatResult<HeartbeatDecisionArgs> {
        let trimmed = content.trim();
        if trimmed.is_empty() {
            return Err(HeartbeatError::response("heartbeat response was empty"));
        }

        if let Ok(parsed) = serde_json::from_str::<HeartbeatDecisionArgs>(trimmed) {
            return Ok(parsed);
        }

        if let Some(extracted) = Self::extract_json_block(trimmed) {
            if let Ok(parsed) = serde_json::from_str::<HeartbeatDecisionArgs>(&extracted) {
                return Ok(parsed);
            }
        }

        if let Some(extracted) = Self::extract_json_object(trimmed) {
            if let Ok(parsed) = serde_json::from_str::<HeartbeatDecisionArgs>(&extracted) {
                return Ok(parsed);
            }
        }

        Err(HeartbeatError::response(
            "heartbeat response did not contain valid JSON",
        ))
    }

    fn extract_json_block(content: &str) -> Option<String> {
        let mut parts = content.split("```");
        let mut index = 0;
        while let Some(part) = parts.next() {
            if index % 2 == 1 {
                let mut block = part.trim_start();
                if let Some(stripped) = block.strip_prefix("json") {
                    block = stripped;
                }
                let block = block.trim();
                if block.starts_with('{') && block.ends_with('}') {
                    return Some(block.to_string());
                }
            }
            index += 1;
        }
        None
    }

    fn extract_json_object(content: &str) -> Option<String> {
        let start = content.find('{')?;
        let end = content.rfind('}')?;
        if start >= end {
            return None;
        }
        Some(content[start..=end].to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use nanobot_provider::ProviderError;
    use nanobot_types::provider::{LLMResponse, UsageStats};

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
        async fn chat(&self, _req: ChatRequest) -> Result<LLMResponse, ProviderError> {
            let mut responses = self.responses.lock().await;
            if responses.is_empty() {
                Ok(LLMResponse {
                    content: None,
                    tool_calls: Vec::new(),
                    finish_reason: "stop".to_string(),
                    usage: UsageStats::default(),
                    reasoning_content: None,
                    thinking_blocks: None,
                })
            } else {
                Ok(responses.remove(0))
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
        async fn on_execute(&self, tasks: String) -> HeartbeatResult<String> {
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
            content: Some(format!("{{\"action\":\"run\",\"tasks\":\"{}\"}}", tasks)),
            tool_calls: Vec::new(),
            finish_reason: "stop".to_string(),
            usage: UsageStats::default(),
            reasoning_content: None,
            thinking_blocks: None,
        }
    }

    fn skip_decision() -> LLMResponse {
        LLMResponse {
            content: Some("{\"action\":\"skip\",\"tasks\":\"\"}".to_string()),
            tool_calls: Vec::new(),
            finish_reason: "stop".to_string(),
            usage: UsageStats::default(),
            reasoning_content: None,
            thinking_blocks: None,
        }
    }

    fn run_decision_block(tasks: &str) -> LLMResponse {
        LLMResponse {
            content: Some(format!(
                "Decision:\n```json\n{{\"action\":\"run\",\"tasks\":\"{}\"}}\n```",
                tasks
            )),
            tool_calls: Vec::new(),
            finish_reason: "stop".to_string(),
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
    async fn trigger_now_parses_json_from_code_block() {
        let workspace = temp_workspace("code-block");
        std::fs::create_dir_all(&workspace).expect("create workspace");
        std::fs::write(workspace.join("HEARTBEAT.md"), "- check project")
            .expect("write heartbeat file");

        let provider: Arc<dyn LLMProvider> =
            Arc::new(StubProvider::new(vec![run_decision_block("finish docs")]));
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
