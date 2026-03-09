use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use tracing::debug;
use uuid::Uuid;

use crate::acp::{ACPClient, AgentConfig as ACPAgentConfig, build_acp_command};
use crate::observability::TARGET_PROVIDER;
use crate::provider::{
    ChatMessage, ChatRequest, ContentPart, LLMProvider, LLMResponse, MessageContent, MessageRole,
    ToolCallRequest, UsageStats,
};
use crate::types::provider_openai::deserialize_arguments_json;
const COPILOT_AGENT_ID: &str = "copilot";

#[derive(Debug, Clone)]
pub struct CopilotAcpProvider {
    default_model: String,
    workspace: PathBuf,
    command: String,
    args: Vec<String>,
    env: HashMap<String, String>,
}

impl CopilotAcpProvider {
    pub fn new(default_model: String, workspace: PathBuf, agent_config: ACPAgentConfig) -> Self {
        Self {
            default_model,
            workspace,
            command: agent_config.command,
            args: agent_config.args,
            env: agent_config.env,
        }
    }

    fn resolve_model(&self, requested: Option<&str>) -> String {
        let model = requested.unwrap_or(&self.default_model);
        if let Some(stripped) = model.strip_prefix("github-copilot/") {
            stripped.to_string()
        } else if let Some(stripped) = model.strip_prefix("github_copilot/") {
            stripped.to_string()
        } else {
            model.to_string()
        }
    }

    fn build_command_args(&self, model: &str) -> Vec<String> {
        let mut args = strip_flag_with_value(&self.args, "--model");
        if !args.iter().any(|arg| arg == "--acp") {
            args.push("--acp".to_string());
        }
        if !args.iter().any(|arg| arg == "--disable-builtin-mcps") {
            args.push("--disable-builtin-mcps".to_string());
        }
        if !args.iter().any(|arg| arg == "--no-ask-user") {
            args.push("--no-ask-user".to_string());
        }
        if !model.trim().is_empty() {
            args.push("--model".to_string());
            args.push(model.to_string());
        }
        args
    }

    fn build_prompt(&self, req: &ChatRequest) -> String {
        let tools_json = match req.tools.as_ref() {
            Some(tools) => serde_json::to_string_pretty(tools).unwrap_or_else(|_| "[]".to_string()),
            None => "[]".to_string(),
        };

        format!(
            concat!(
                "You are acting as the model backend for nanobot-rs.\n",
                "Return exactly one JSON object and nothing else.\n",
                "Do not use ACP tools, do not read or write files, and do not run commands.\n",
                "Use only the conversation transcript and available tool definitions below.\n\n",
                "Output schema:\n",
                "{{\n",
                "  \"content\": string | null,\n",
                "  \"tool_calls\": [{{\"id\": string | null, \"name\": string, \"arguments_json\": string}}],\n",
                "  \"finish_reason\": \"stop\" | \"tool_calls\",\n",
                "  \"reasoning_content\": string | null,\n",
                "  \"thinking_blocks\": string[] | null\n",
                "}}\n\n",
                "Rules:\n",
                "1. If you can answer directly, set tool_calls to [] and finish_reason to \"stop\".\n",
                "2. If you need host tools, emit one or more tool_calls and set finish_reason to \"tool_calls\".\n",
                "3. Each arguments_json must be valid JSON encoded as a string.\n",
                "4. Use only tool names present in the tool list.\n",
                "5. Do not wrap the JSON in markdown fences.\n\n",
                "Conversation transcript:\n",
                "{}\n\n",
                "Available tools:\n",
                "{}\n"
            ),
            render_messages(&req.messages),
            tools_json,
        )
    }

    async fn execute_prompt(&self, prompt: String, model: &str) -> Result<String> {
        let args = self.build_command_args(model);
        let (command, session_cwd) = build_acp_command(
            &self.command,
            &args,
            Some(self.workspace.clone()),
            &self.env,
        )?;

        let mut client =
            ACPClient::spawn_prompt_only(COPILOT_AGENT_ID.to_string(), command, session_cwd)
                .await?;

        let execution_result = client.execute(&prompt).await;
        let close_result = client.close().await;

        match (execution_result, close_result) {
            (Ok(output), Ok(())) => Ok(output),
            (Ok(_), Err(close_err)) => Err(close_err),
            (Err(exec_err), Ok(())) => Err(exec_err),
            (Err(exec_err), Err(close_err)) => Err(anyhow::anyhow!(
                "Copilot ACP execution failed: {}; additionally failed to close process: {}",
                exec_err,
                close_err
            )),
        }
    }
}

#[async_trait]
impl LLMProvider for CopilotAcpProvider {
    async fn chat(&self, req: ChatRequest) -> LLMResponse {
        let model = self.resolve_model(req.model.as_deref());
        let prompt = self.build_prompt(&req);

        debug!(
            target: TARGET_PROVIDER,
            "copilot acp request model={} tools={}",
            model,
            req.tools.as_ref().map(|t| t.len()).unwrap_or(0)
        );

        match self.execute_prompt(prompt, &model).await {
            Ok(output) => parse_output(&output),
            Err(err) => error_response(map_error_message(&err.to_string())),
        }
    }

    fn default_model(&self) -> &str {
        &self.default_model
    }
}

#[derive(Debug, Deserialize)]
struct CopilotStructuredResponse {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<CopilotToolCall>,
    #[serde(default)]
    finish_reason: Option<String>,
    #[serde(default)]
    reasoning_content: Option<String>,
    #[serde(default)]
    thinking_blocks: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct CopilotToolCall {
    #[serde(default)]
    id: Option<String>,
    name: String,
    #[serde(deserialize_with = "deserialize_arguments_json")]
    arguments_json: String,
}

fn parse_output(raw: &str) -> LLMResponse {
    if let Some(parsed) = parse_structured_response(raw) {
        return parsed;
    }

    let text = raw.trim();
    LLMResponse {
        content: (!text.is_empty()).then_some(text.to_string()),
        tool_calls: Vec::new(),
        finish_reason: "stop".to_string(),
        usage: UsageStats::default(),
        reasoning_content: None,
        thinking_blocks: None,
    }
}

fn parse_structured_response(raw: &str) -> Option<LLMResponse> {
    for candidate in json_candidates(raw) {
        let Ok(parsed) = serde_json::from_str::<CopilotStructuredResponse>(candidate) else {
            continue;
        };
        let tool_calls = parsed
            .tool_calls
            .into_iter()
            .map(|tc| ToolCallRequest {
                id: tc.id.unwrap_or_else(|| Uuid::new_v4().to_string()),
                name: tc.name.into(),
                arguments_json: tc.arguments_json,
            })
            .collect::<Vec<_>>();

        let finish_reason = parsed.finish_reason.unwrap_or_else(|| {
            if tool_calls.is_empty() {
                "stop".to_string()
            } else {
                "tool_calls".to_string()
            }
        });

        return Some(LLMResponse {
            content: parsed.content.filter(|text| !text.trim().is_empty()),
            tool_calls,
            finish_reason,
            usage: UsageStats::default(),
            reasoning_content: parsed.reasoning_content,
            thinking_blocks: parsed.thinking_blocks.map(|blocks| {
                blocks
                    .into_iter()
                    .filter(|b| !b.trim().is_empty())
                    .collect()
            }),
        });
    }

    None
}

fn json_candidates(raw: &str) -> Vec<&str> {
    let trimmed = raw.trim();
    let mut candidates = Vec::new();
    if !trimmed.is_empty() {
        candidates.push(trimmed);
    }

    if let Some(fenced) = extract_fenced_json(trimmed) {
        candidates.push(fenced);
    }

    if let Some(object) = extract_json_object(trimmed) {
        candidates.push(object);
    }

    candidates
}

fn extract_fenced_json(raw: &str) -> Option<&str> {
    let stripped = raw.strip_prefix("```json")?.trim_start();
    stripped.strip_suffix("```").map(str::trim)
}

fn extract_json_object(raw: &str) -> Option<&str> {
    let mut start = None;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    for (idx, ch) in raw.char_indices() {
        match ch {
            '"' if !escaped => in_string = !in_string,
            '\\' if in_string => {
                escaped = !escaped;
                continue;
            }
            '{' if !in_string => {
                if start.is_none() {
                    start = Some(idx);
                }
                depth += 1;
            }
            '}' if !in_string => {
                if depth == 0 {
                    continue;
                }
                depth -= 1;
                if depth == 0 {
                    return start.map(|begin| &raw[begin..=idx]);
                }
            }
            _ => {}
        }

        if ch != '\\' || !in_string {
            escaped = false;
        }
    }

    None
}

fn strip_flag_with_value(args: &[String], flag: &str) -> Vec<String> {
    let mut out = Vec::with_capacity(args.len());
    let mut skip_next = false;
    for arg in args {
        if skip_next {
            skip_next = false;
            continue;
        }
        if arg == flag {
            skip_next = true;
            continue;
        }
        out.push(arg.clone());
    }
    out
}

fn render_messages(messages: &[ChatMessage]) -> String {
    let mut out = String::new();
    for message in messages {
        out.push_str("[");
        out.push_str(role_label(&message.role));
        out.push_str("]\n");

        if let Some(content) = &message.content {
            match content {
                MessageContent::Text(text) => {
                    if !text.trim().is_empty() {
                        out.push_str(text);
                        if !text.ends_with('\n') {
                            out.push('\n');
                        }
                    }
                }
                MessageContent::Parts(parts) => {
                    for part in parts {
                        let ContentPart::Text { text } = part;
                        if !text.trim().is_empty() {
                            out.push_str(text);
                            if !text.ends_with('\n') {
                                out.push('\n');
                            }
                        }
                    }
                }
            }
        }

        if let Some(tool_calls) = &message.tool_calls {
            for call in tool_calls {
                out.push_str("tool_call ");
                out.push_str(&call.function.name);
                out.push_str(": ");
                out.push_str(&call.function.arguments);
                out.push('\n');
            }
        }

        if let (Some(tool_name), Some(tool_call_id)) = (&message.name, &message.tool_call_id) {
            out.push_str("tool_result ");
            out.push_str(tool_name);
            out.push_str(" [");
            out.push_str(tool_call_id);
            out.push_str("]\n");
        }

        if let Some(reasoning) = &message.reasoning_content
            && !reasoning.trim().is_empty()
        {
            out.push_str("reasoning: ");
            out.push_str(reasoning);
            out.push('\n');
        }

        if let Some(blocks) = &message.thinking_blocks {
            for block in blocks {
                if !block.trim().is_empty() {
                    out.push_str("thinking: ");
                    out.push_str(block);
                    out.push('\n');
                }
            }
        }

        if !out.ends_with('\n') {
            out.push('\n');
        }
    }

    out.trim().to_string()
}

fn role_label(role: &MessageRole) -> &'static str {
    match role {
        MessageRole::System => "system",
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::Tool => "tool",
    }
}

fn map_error_message(message: &str) -> String {
    let lower = message.to_lowercase();
    if lower.contains("not authenticated")
        || lower.contains("authentication")
        || lower.contains("login")
        || lower.contains("oauth")
    {
        return "GitHub Copilot is not authenticated. Run `copilot login` or `nanobot-rs provider login github_copilot` first.".to_string();
    }

    format!("Error calling GitHub Copilot via ACP: {}", message)
}

fn error_response(message: String) -> LLMResponse {
    LLMResponse {
        content: Some(message),
        tool_calls: Vec::new(),
        finish_reason: "error".to_string(),
        usage: UsageStats::default(),
        reasoning_content: None,
        thinking_blocks: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{ChatMessage, MessageContent};

    fn make_provider() -> CopilotAcpProvider {
        CopilotAcpProvider::new(
            "github-copilot/gpt-5.2-codex".to_string(),
            PathBuf::from("/tmp"),
            ACPAgentConfig {
                command: "copilot".to_string(),
                args: vec!["--acp".to_string()],
                env: HashMap::new(),
            },
        )
    }

    #[test]
    fn resolve_model_strips_provider_prefix() {
        let provider = make_provider();
        assert_eq!(
            provider.resolve_model(Some("github-copilot/gpt-5.3-codex")),
            "gpt-5.3-codex"
        );
        assert_eq!(
            provider.resolve_model(Some("github_copilot/gpt-5.2")),
            "gpt-5.2"
        );
    }

    #[test]
    fn build_command_args_enforces_prompt_only_runtime_flags() {
        let provider = make_provider();
        let args = provider.build_command_args("gpt-5.2-codex");
        assert!(args.iter().any(|arg| arg == "--acp"));
        assert!(args.iter().any(|arg| arg == "--disable-builtin-mcps"));
        assert!(args.iter().any(|arg| arg == "--no-ask-user"));
        assert_eq!(
            args.windows(2)
                .find(|pair| pair[0] == "--model")
                .map(|pair| pair[1].as_str()),
            Some("gpt-5.2-codex")
        );
    }

    #[test]
    fn parse_output_accepts_strict_json() {
        let out = parse_output(
            r#"{"content":"ok","tool_calls":[{"name":"read_file","arguments_json":"{\"path\":\"Cargo.toml\"}"}],"finish_reason":"tool_calls"}"#,
        );
        assert_eq!(out.content.as_deref(), Some("ok"));
        assert_eq!(out.tool_calls.len(), 1);
        assert_eq!(out.tool_calls[0].name.as_str(), "read_file");
        assert_eq!(out.finish_reason, "tool_calls");
    }

    #[test]
    fn parse_output_extracts_json_from_prefixed_text() {
        let out = parse_output(
            "[plan]\n- inspect files\n{\"content\":\"done\",\"tool_calls\":[],\"finish_reason\":\"stop\"}",
        );
        assert_eq!(out.content.as_deref(), Some("done"));
        assert!(out.tool_calls.is_empty());
        assert_eq!(out.finish_reason, "stop");
    }

    #[test]
    fn parse_output_falls_back_to_plain_text() {
        let out = parse_output("plain answer");
        assert_eq!(out.content.as_deref(), Some("plain answer"));
        assert!(out.tool_calls.is_empty());
        assert_eq!(out.finish_reason, "stop");
    }

    #[test]
    fn render_messages_keeps_tool_context() {
        let assistant = ChatMessage {
            role: MessageRole::Assistant,
            content: Some(MessageContent::Text("Need a file".to_string())),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            reasoning_content: None,
            thinking_blocks: None,
        };
        let tool = ChatMessage::tool_result("tc1", "read_file", "contents");

        let rendered = render_messages(&[assistant, tool]);
        assert!(rendered.contains("[assistant]"));
        assert!(rendered.contains("[tool]"));
        assert!(rendered.contains("tool_result read_file [tc1]"));
    }
}
