//! Subagent management — independent agent tasks spawned from the main loop.
//!
//! [`SubagentManager`] implements the [`SpawnService`] trait, allowing
//! the main agent to spawn parallel agentic tasks. Each subagent runs its
//! own simplified ReAct loop with the provider and tools shared from the
//! parent, and its result is delivered back via the message bus as a
//! system message.
//!
//! # Design Notes
//!
//! - **Shared dependencies**: Subagents reuse the parent's LLM provider
//!   and tool registry, avoiding redundant setup.
//! - **Task tracking**: Running tasks are tracked in [`DashMap`]s keyed by
//!   `TaskId` and `SessionKey`, enabling bulk cancellation via
//!   [`cancel_by_session`].
//! - **Result routing**: Subagent results are published as system messages
//!   with a `chat_id` of `"origin_channel:origin_chat_id"`, which the
//!   main loop's [`process_message`](crate::loop_core::AgentLoop::process_message)
//!   unpacks and routes to the correct channel.
//! - **Think-block stripping**: Subagent responses are cleaned of
//!   `<think>` blocks before being returned, since they may contain
//!   internal monologue not intended for the user.

use std::sync::Arc;

use async_trait::async_trait;
use dashmap::DashMap;
use regex::Regex;
use tokio::task::JoinHandle;
use tracing::{error, info};

use crate::skills::SkillsLoader;
use nanobot_bus::{InboundMessage, MessageBus, MessageMetadata};
use nanobot_provider::{ChatRequest, LLMProvider};
use nanobot_tools::{ToolContext, ToolRegistry, spawn::SpawnService};
use nanobot_types::SessionKey;
use nanobot_types::provider::{AssistantFunctionCall, AssistantToolCall, ChatMessage};
use nanobot_types::task::TaskId;

/// Tracing target for log messages from this module.
const TARGET: &str = "nanobot::subagent";
/// System prompt template for subagents, with `{runtime}` and `{workspace}`
/// placeholders.
const SUBAGENT_PROMPT_TEMPLATE: &str = "# Subagent\n\nCurrent Time: {runtime}\n\nYou are a subagent spawned by the main agent to complete a specific task. Stay focused and provide a concise final result.\n\n## Workspace\n{workspace}";
/// Preamble informing the subagent about skills it can use.
const SUBAGENT_SKILLS_PREAMBLE: &str =
    "## Skills\n\nRead SKILL.md with read_file to use a skill.\n\n";

/// Internal shared state behind [`SubagentManager`].
struct SubagentManagerInner {
    provider: Arc<dyn LLMProvider>,
    workspace: std::path::PathBuf,
    bus: MessageBus,
    tools: Arc<ToolRegistry>,
    model: String,
    temperature: f32,
    max_tokens: i32,
    reasoning_effort: Option<nanobot_provider::ReasoningConfig>,
    max_iterations: usize,
    /// Map of all running subagent task handles, keyed by task ID.
    running_tasks: DashMap<TaskId, JoinHandle<()>>,
    /// Map of task IDs to their originating session, for session-scoped
    /// cancellation.
    session_tasks: DashMap<SessionKey, DashMap<TaskId, ()>>,
}

impl std::fmt::Debug for SubagentManagerInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SubagentManagerInner")
            .field("workspace", &self.workspace)
            .field("model", &self.model)
            .field("temperature", &self.temperature)
            .field("max_tokens", &self.max_tokens)
            .field("reasoning_effort", &self.reasoning_effort)
            .field("running_tasks", &"<DashMap>")
            .field("session_tasks", &"<DashMap>")
            .finish()
    }
}

/// Manages subagent lifecycle: spawn, track, cancel, and cleanup.
///
/// Implements [`SpawnService`] so it can be registered with the tool
/// registry as the backend for the `spawn_subagent` tool.
#[derive(Clone, Debug)]
pub struct SubagentManager {
    inner: Arc<SubagentManagerInner>,
}

impl SubagentManager {
    /// Creates a new `SubagentManager`.
    ///
    /// This constructor is `pub(crate)` — callers go through the
    /// [`AgentLoopBuilder`](crate::builder::AgentLoopBuilder).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        provider: Arc<dyn LLMProvider>,
        workspace: std::path::PathBuf,
        bus: MessageBus,
        tools: Arc<ToolRegistry>,
        model: String,
        temperature: f32,
        max_tokens: i32,
        reasoning_effort: Option<nanobot_provider::ReasoningConfig>,
        max_iterations: usize,
    ) -> Self {
        Self {
            inner: Arc::new(SubagentManagerInner {
                provider,
                workspace,
                bus,
                tools,
                model,
                temperature,
                max_tokens,
                reasoning_effort,
                max_iterations,
                running_tasks: DashMap::new(),
                session_tasks: DashMap::new(),
            }),
        }
    }

    /// Cancels all subagent tasks associated with the given session.
    pub async fn cancel_by_session(&self, session_key: &SessionKey) -> usize {
        self.inner.cancel_by_session(session_key).await
    }
}

impl SubagentManagerInner {
    /// Spawns a subagent in a new tokio task, registers it for lifecycle
    /// tracking, and returns a notification string.
    async fn spawn_impl(
        self: &Arc<Self>,
        task: String,
        label: Option<String>,
        origin_channel: String,
        origin_chat_id: String,
        session_key: Option<SessionKey>,
    ) -> String {
        let task_id = TaskId::new();
        let display_label = label.unwrap_or_else(|| truncate(&task, 30));

        let this = self.clone();
        let handle = tokio::spawn({
            let session_key = session_key.clone();
            let display_label = display_label.clone();
            async move {
                this.run_subagent(
                    &task_id,
                    &task,
                    &display_label,
                    &origin_channel,
                    &origin_chat_id,
                )
                .await;
                this.cleanup_task(&task_id, session_key.as_ref()).await;
            }
        });

        self.running_tasks.insert(task_id, handle);
        if let Some(session) = session_key {
            self.session_tasks
                .entry(session)
                .or_default()
                .insert(task_id, ());
        }

        info!(
            target: TARGET,
            "spawned subagent [{}]: {}",
            task_id,
            display_label
        );
        format!(
            "Subagent [{}] started (id: {}). I'll notify you when it completes.",
            display_label, task_id
        )
    }

    /// Cancels all subagent tasks for a given session by aborting their
    /// join handles.
    async fn cancel_by_session(&self, session_key: &SessionKey) -> usize {
        let ids = if let Some((_, tasks)) = self.session_tasks.remove(session_key) {
            tasks.into_iter().map(|(id, _)| id).collect::<Vec<_>>()
        } else {
            return 0;
        };

        let mut cancelled = 0usize;
        for id in ids {
            if let Some((_, handle)) = self.running_tasks.remove(&id)
                && !handle.is_finished()
            {
                handle.abort();
                cancelled += 1;
            }
        }
        cancelled
    }

    /// Removes a task from both tracking maps after it completes.
    async fn cleanup_task(&self, task_id: &TaskId, session_key: Option<&SessionKey>) {
        self.running_tasks.remove(task_id);
        if let Some(session_key) = session_key
            && let Some(tasks) = self.session_tasks.get(session_key)
        {
            tasks.remove(task_id);
            if tasks.is_empty() {
                drop(tasks);
                self.session_tasks.remove(session_key);
            }
        }
    }

    /// Runs a single subagent: builds the prompt, runs the loop, and
    /// announces the result via the message bus.
    async fn run_subagent(
        &self,
        task_id: &TaskId,
        task: &str,
        label: &str,
        origin_channel: &str,
        origin_chat_id: &str,
    ) {
        info!(target: TARGET, "subagent [{}] starting: {}", task_id, label);

        let tool_context = ToolContext {
            channel: origin_channel.to_string(),
            chat_id: origin_chat_id.to_string(),
            session_key: SessionKey::new(origin_channel, origin_chat_id),
            message_id: None,
        };

        let outcome = run_subagent_loop_impl(
            task,
            &tool_context,
            self.provider.as_ref(),
            &self.workspace,
            self.tools.as_ref(),
            &self.model,
            self.temperature,
            self.max_tokens,
            self.reasoning_effort.clone(),
            self.max_iterations,
        )
        .await;

        match outcome {
            Ok(result) => {
                info!(target: TARGET, "subagent [{}] completed", task_id);
                announce_result_impl(
                    &task_id.to_string(),
                    label,
                    task,
                    &result,
                    origin_channel,
                    origin_chat_id,
                    "ok",
                    &self.bus,
                );
            }
            Err(err) => {
                error!(target: TARGET, task_id = %task_id, error = %err, "subagent failed");
                announce_result_impl(
                    &task_id.to_string(),
                    label,
                    task,
                    &format!("Error: {}", err),
                    origin_channel,
                    origin_chat_id,
                    "error",
                    &self.bus,
                );
            }
        }
    }
}

#[async_trait]
impl SpawnService for SubagentManager {
    /// Spawns a new subagent and returns a human-readable confirmation.
    async fn spawn(
        &self,
        task: String,
        label: Option<String>,
        origin_channel: String,
        origin_chat_id: String,
        session_key: Option<SessionKey>,
    ) -> String {
        self.inner
            .spawn_impl(task, label, origin_channel, origin_chat_id, session_key)
            .await
    }

    /// Cancels all subagents for a session.
    async fn cancel_by_session(&self, session_key: &SessionKey) -> anyhow::Result<usize> {
        Ok(self.inner.cancel_by_session(session_key).await)
    }
}

/// Truncates text to `max` characters, appending `"..."` if truncated.
fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out = String::new();
    for c in text.chars().take(max) {
        out.push(c);
    }
    out.push_str("...");
    out
}

/// Strips `<think>...</think>` blocks from an optional text string.
///
/// If the cleaned text is empty, returns `None`.
fn strip_think(text: Option<&str>) -> Option<String> {
    let t = text?;
    let re = Regex::new(r"<think>[\s\S]*?</think>").ok()?;
    let cleaned = re.replace_all(t, "").trim().to_string();
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

/// The core subagent loop: sends a system prompt + task to the LLM,
/// executes any tool calls, and repeats until a final answer is produced
/// or `max_iterations` is reached.
#[allow(clippy::too_many_arguments)]
async fn run_subagent_loop_impl(
    task: &str,
    tool_context: &ToolContext,
    provider: &dyn LLMProvider,
    workspace: &std::path::Path,
    tools: &ToolRegistry,
    model: &str,
    temperature: f32,
    max_tokens: i32,
    reasoning_effort: Option<nanobot_provider::ReasoningConfig>,
    max_iterations: usize,
) -> anyhow::Result<String> {
    let tool_defs = tools.definitions();

    let runtime = chrono::Local::now()
        .format("%Y-%m-%d %H:%M (%A)")
        .to_string();
    let mut parts = vec![
        SUBAGENT_PROMPT_TEMPLATE
            .replace("{runtime}", &runtime)
            .replace("{workspace}", &workspace.display().to_string()),
    ];

    // Subagents can also use available skills
    let skills = SkillsLoader::new(workspace).build_skills_summary().await;
    if !skills.trim().is_empty() {
        parts.push(format!("{SUBAGENT_SKILLS_PREAMBLE}{skills}"));
    }

    let system_prompt = parts.join("\n\n");
    let mut messages = vec![
        ChatMessage::system_text(system_prompt),
        ChatMessage::user_text(task),
    ];

    let mut final_result = None;
    for _ in 0..max_iterations {
        let response = provider
            .chat(ChatRequest {
                session_key: Some(tool_context.session_key.clone()),
                messages: messages.clone(),
                tools: Some(tool_defs.clone()),
                model: Some(model.to_string()),
                max_tokens,
                temperature,
                reasoning_effort: reasoning_effort.clone(),
            })
            .await
            .map_err(|e| anyhow::anyhow!("Subagent LLM provider error: {}", e))?;

        if response.has_tool_calls() {
            let tool_calls = response
                .tool_calls
                .iter()
                .map(|tc| AssistantToolCall {
                    id: tc.id.clone(),
                    kind: "function".to_string(),
                    function: AssistantFunctionCall {
                        name: tc.name.to_string(),
                        arguments: tc.arguments_json.clone(),
                    },
                })
                .collect::<Vec<_>>();

            messages.push(ChatMessage::assistant(
                response.content,
                Some(tool_calls),
                response.reasoning_content,
                response.thinking_blocks,
            ));

            for call in response.tool_calls {
                let result = tools
                    .execute(call.name.as_str(), &call.arguments_json, tool_context)
                    .await;
                let rendered = match result {
                    Ok(v) => v,
                    Err(err) => format!("Error: {}", err),
                };
                messages.push(ChatMessage::tool_result(
                    call.id,
                    call.name.to_string(),
                    rendered,
                ));
            }
        } else {
            final_result = strip_think(response.content.as_deref());
            break;
        }
    }

    Ok(final_result
        .unwrap_or_else(|| "Task completed but no final response was generated.".to_string()))
}

/// Publishes a subagent result back to the main agent as a system message.
///
/// The `chat_id` is encoded as `"origin_channel:origin_chat_id"` so the
/// main loop can route the response to the correct channel.
#[allow(clippy::too_many_arguments)]
fn announce_result_impl(
    _task_id: &str,
    label: &str,
    task: &str,
    result: &str,
    origin_channel: &str,
    origin_chat_id: &str,
    status: &str,
    bus: &MessageBus,
) {
    let status_text = if status == "ok" {
        "completed successfully"
    } else {
        "failed"
    };
    let content = format!(
        "[Subagent '{}' {}]\n\nTask: {}\n\nResult:\n{}\n\nSummarize this naturally for the user. Keep it brief (1-2 sentences). Do not mention technical details like subagent or task IDs.",
        label, status_text, task, result
    );
    let msg = InboundMessage {
        channel: "system".to_string(),
        sender_id: "subagent".to_string(),
        chat_id: format!("{}:{}", origin_channel, origin_chat_id),
        content: content.into(),
        timestamp: chrono::Utc::now(),
        media: Vec::new(),
        metadata: MessageMetadata::default(),
        session_key_override: None,
    };
    if let Err(err) = bus.publish_inbound(msg) {
        error!(target: TARGET, "failed to publish subagent result: {}", err);
    }
}
