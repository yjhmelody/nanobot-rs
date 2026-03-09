//! ACP client-side handler implementation for local runtime integration.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;

use agent_client_protocol::{
    Client, ClientCapabilities, Content, ContentBlock, CreateTerminalRequest,
    CreateTerminalResponse, EmbeddedResourceResource, Error, FileSystemCapabilities,
    KillTerminalRequest, KillTerminalResponse, ReadTextFileRequest, ReadTextFileResponse,
    ReleaseTerminalRequest, ReleaseTerminalResponse, RequestPermissionOutcome,
    RequestPermissionRequest, RequestPermissionResponse, Result, SelectedPermissionOutcome,
    SessionId, SessionNotification, SessionUpdate, StopReason, TerminalExitStatus, TerminalId,
    TerminalOutputRequest, TerminalOutputResponse, ToolCallContent, WaitForTerminalExitRequest,
    WaitForTerminalExitResponse, WriteTextFileRequest, WriteTextFileResponse,
};
use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tracing::{debug, warn};

#[derive(Clone)]
pub struct SimpleClient {
    state: Arc<Mutex<SimpleClientState>>,
    allow_fs: bool,
    allow_terminal: bool,
}

impl SimpleClient {
    pub fn new(default_cwd: PathBuf) -> Self {
        Self::with_permissions(default_cwd, true, true)
    }

    pub fn prompt_only(default_cwd: PathBuf) -> Self {
        Self::with_permissions(default_cwd, false, false)
    }

    fn with_permissions(default_cwd: PathBuf, allow_fs: bool, allow_terminal: bool) -> Self {
        Self {
            state: Arc::new(Mutex::new(SimpleClientState::new(default_cwd))),
            allow_fs,
            allow_terminal,
        }
    }

    pub fn capabilities(&self) -> ClientCapabilities {
        let capabilities = ClientCapabilities::new();
        let capabilities = if self.allow_fs {
            capabilities.fs(FileSystemCapabilities::new()
                .read_text_file(true)
                .write_text_file(true))
        } else {
            capabilities
        };

        capabilities.terminal(self.allow_terminal)
    }

    pub async fn begin_turn(&self, session_id: &SessionId) {
        let mut state = self.state.lock().await;
        state
            .session_buffers
            .insert(session_id.clone(), String::new());
    }

    pub async fn take_turn_output(
        &self,
        session_id: &SessionId,
        stop_reason: StopReason,
    ) -> String {
        let mut state = self.state.lock().await;
        let output = state
            .session_buffers
            .remove(session_id)
            .unwrap_or_default()
            .trim()
            .to_string();
        if output.is_empty() {
            format!("(ACP turn finished: {})", stop_reason_label(stop_reason))
        } else {
            output
        }
    }

    pub async fn close_all_terminals(&self) {
        let entries = {
            let mut state = self.state.lock().await;
            state.terminals.drain().collect::<Vec<_>>()
        };

        for (_, entry) in entries {
            let mut child = entry.child.lock().await;
            let _ = kill_child_if_running(&mut child).await;
        }
    }
}

#[async_trait::async_trait(?Send)]
impl Client for SimpleClient {
    async fn request_permission(
        &self,
        args: RequestPermissionRequest,
    ) -> Result<RequestPermissionResponse> {
        let outcome = if let Some(first_option) = args.options.first() {
            RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(
                first_option.option_id.clone(),
            ))
        } else {
            RequestPermissionOutcome::Cancelled
        };

        Ok(RequestPermissionResponse::new(outcome))
    }

    async fn session_notification(&self, args: SessionNotification) -> Result<()> {
        let mut state = self.state.lock().await;
        state.record_session_update(args);
        Ok(())
    }

    async fn write_text_file(&self, args: WriteTextFileRequest) -> Result<WriteTextFileResponse> {
        if let Some(parent) = args.path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(Error::into_internal_error)?;
        }

        tokio::fs::write(&args.path, args.content)
            .await
            .map_err(Error::into_internal_error)?;
        Ok(WriteTextFileResponse::new())
    }

    async fn read_text_file(&self, args: ReadTextFileRequest) -> Result<ReadTextFileResponse> {
        let content = tokio::fs::read_to_string(&args.path).await.map_err(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                Error::resource_not_found(Some(args.path.to_string_lossy().to_string()))
            } else {
                Error::into_internal_error(err)
            }
        })?;

        if let Some(line) = args.line
            && line == 0
        {
            return Err(Error::invalid_params().data("line must be 1-based"));
        }

        let start = args.line.unwrap_or(1) as usize;
        let limit = args.limit.unwrap_or(u32::MAX) as usize;
        let sliced = slice_lines(&content, start, limit);
        Ok(ReadTextFileResponse::new(sliced))
    }

    async fn create_terminal(&self, args: CreateTerminalRequest) -> Result<CreateTerminalResponse> {
        let (terminal_id, default_cwd) = {
            let mut state = self.state.lock().await;
            let terminal_id = state.next_terminal_id();
            (terminal_id, state.default_cwd.clone())
        };

        let mut command = Command::new(&args.command);
        command.args(&args.args);
        for kv in &args.env {
            command.env(&kv.name, &kv.value);
        }
        let cwd = args.cwd.unwrap_or(default_cwd);
        command.current_dir(cwd);
        command.stdin(Stdio::null());
        command.stdout(Stdio::piped());
        command.stderr(Stdio::piped());

        let mut child = command.spawn().map_err(Error::into_internal_error)?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| Error::internal_error().data("terminal stdout is unavailable"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| Error::internal_error().data("terminal stderr is unavailable"))?;

        let output = Arc::new(Mutex::new(TerminalOutputState {
            content: String::new(),
            truncated: false,
            output_byte_limit: args.output_byte_limit.map(|v| v as usize),
        }));
        let child = Arc::new(Mutex::new(child));

        spawn_terminal_reader(stdout, output.clone());
        spawn_terminal_reader(stderr, output.clone());

        let entry = TerminalEntry { child, output };

        {
            let mut state = self.state.lock().await;
            state.terminals.insert(terminal_id.clone(), entry);
        }

        Ok(CreateTerminalResponse::new(terminal_id))
    }

    async fn terminal_output(&self, args: TerminalOutputRequest) -> Result<TerminalOutputResponse> {
        let entry = {
            let state = self.state.lock().await;
            state
                .terminals
                .get(&args.terminal_id)
                .cloned()
                .ok_or_else(|| Error::resource_not_found(Some(args.terminal_id.to_string())))?
        };

        let (output, truncated) = {
            let output = entry.output.lock().await;
            (output.content.clone(), output.truncated)
        };

        let exit_status = {
            let mut child = entry.child.lock().await;
            match child.try_wait().map_err(Error::into_internal_error)? {
                Some(status) => Some(to_terminal_exit_status(status)),
                None => None,
            }
        };

        Ok(TerminalOutputResponse::new(output, truncated).exit_status(exit_status))
    }

    async fn release_terminal(
        &self,
        args: ReleaseTerminalRequest,
    ) -> Result<ReleaseTerminalResponse> {
        let entry = {
            let mut state = self.state.lock().await;
            state.terminals.remove(&args.terminal_id)
        };

        if let Some(entry) = entry {
            let mut child = entry.child.lock().await;
            kill_child_if_running(&mut child)
                .await
                .map_err(Error::into_internal_error)?;
        }

        Ok(ReleaseTerminalResponse::new())
    }

    async fn wait_for_terminal_exit(
        &self,
        args: WaitForTerminalExitRequest,
    ) -> Result<WaitForTerminalExitResponse> {
        let entry = {
            let state = self.state.lock().await;
            state
                .terminals
                .get(&args.terminal_id)
                .cloned()
                .ok_or_else(|| Error::resource_not_found(Some(args.terminal_id.to_string())))?
        };

        let status = {
            let mut child = entry.child.lock().await;
            child.wait().await.map_err(Error::into_internal_error)?
        };

        Ok(WaitForTerminalExitResponse::new(to_terminal_exit_status(
            status,
        )))
    }

    async fn kill_terminal(&self, args: KillTerminalRequest) -> Result<KillTerminalResponse> {
        let entry = {
            let state = self.state.lock().await;
            state
                .terminals
                .get(&args.terminal_id)
                .cloned()
                .ok_or_else(|| Error::resource_not_found(Some(args.terminal_id.to_string())))?
        };

        let mut child = entry.child.lock().await;
        kill_child_if_running(&mut child)
            .await
            .map_err(Error::into_internal_error)?;
        Ok(KillTerminalResponse::new())
    }
}

#[derive(Default)]
struct SimpleClientState {
    default_cwd: PathBuf,
    session_buffers: HashMap<SessionId, String>,
    terminals: HashMap<TerminalId, TerminalEntry>,
    next_terminal_id: u64,
}

impl SimpleClientState {
    fn new(default_cwd: PathBuf) -> Self {
        Self {
            default_cwd,
            session_buffers: HashMap::new(),
            terminals: HashMap::new(),
            next_terminal_id: 0,
        }
    }

    fn next_terminal_id(&mut self) -> TerminalId {
        self.next_terminal_id += 1;
        TerminalId::new(format!("nanobot-terminal-{}", self.next_terminal_id))
    }

    fn record_session_update(&mut self, notification: SessionNotification) {
        let Some(buffer) = self.session_buffers.get_mut(&notification.session_id) else {
            return;
        };

        match notification.update {
            SessionUpdate::AgentMessageChunk(chunk) => {
                append_content_block(buffer, &chunk.content);
            }
            SessionUpdate::ToolCall(tool_call) => {
                if !tool_call.title.trim().is_empty() {
                    buffer.push_str(&format!("\n[tool] {}\n", tool_call.title.trim()));
                }
                for content in tool_call.content {
                    append_tool_call_content(buffer, &content);
                }
            }
            SessionUpdate::ToolCallUpdate(update) => {
                if let Some(content) = update.fields.content {
                    for block in content {
                        append_tool_call_content(buffer, &block);
                    }
                }
            }
            SessionUpdate::Plan(plan) => {
                if !plan.entries.is_empty() {
                    buffer.push_str("\n[plan]\n");
                    for entry in plan.entries {
                        buffer.push_str("- ");
                        buffer.push_str(entry.content.trim());
                        buffer.push('\n');
                    }
                }
            }
            _ => {}
        }
    }
}

#[derive(Clone)]
struct TerminalEntry {
    child: Arc<Mutex<Child>>,
    output: Arc<Mutex<TerminalOutputState>>,
}

struct TerminalOutputState {
    content: String,
    truncated: bool,
    output_byte_limit: Option<usize>,
}

fn spawn_terminal_reader<R>(mut reader: R, output: Arc<Mutex<TerminalOutputState>>)
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut chunk = vec![0u8; 8192];
        loop {
            match reader.read(&mut chunk).await {
                Ok(0) => break,
                Ok(len) => {
                    let part = String::from_utf8_lossy(&chunk[..len]);
                    let mut output_state = output.lock().await;
                    output_state.content.push_str(&part);
                    if let Some(limit) = output_state.output_byte_limit {
                        let mut truncated = output_state.truncated;
                        truncate_from_start(&mut output_state.content, limit, &mut truncated);
                        output_state.truncated = truncated;
                    }
                }
                Err(err) => {
                    warn!("terminal reader failed: {}", err);
                    break;
                }
            }
        }
    });
}

fn truncate_from_start(value: &mut String, max_bytes: usize, truncated: &mut bool) {
    if value.len() <= max_bytes {
        return;
    }
    *truncated = true;

    let mut start = value.len().saturating_sub(max_bytes);
    while start < value.len() && !value.is_char_boundary(start) {
        start += 1;
    }
    *value = value[start..].to_string();
}

fn append_tool_call_content(buffer: &mut String, content: &ToolCallContent) {
    match content {
        ToolCallContent::Content(Content { content, .. }) => append_content_block(buffer, content),
        ToolCallContent::Diff(diff) => {
            buffer.push_str("\n[diff] ");
            buffer.push_str(&diff.path.to_string_lossy());
            buffer.push('\n');
        }
        ToolCallContent::Terminal(terminal) => {
            buffer.push_str("\n[terminal] ");
            buffer.push_str(&terminal.terminal_id.to_string());
            buffer.push('\n');
        }
        _ => {}
    }
}

fn append_content_block(buffer: &mut String, block: &ContentBlock) {
    let text = extract_content_text(block);
    if text.trim().is_empty() {
        return;
    }

    if !buffer.is_empty() && !buffer.ends_with('\n') {
        buffer.push('\n');
    }
    buffer.push_str(text.trim_end());
    if !buffer.ends_with('\n') {
        buffer.push('\n');
    }
}

fn extract_content_text(block: &ContentBlock) -> &str {
    match block {
        ContentBlock::Text(text) => text.text.as_str(),
        ContentBlock::Resource(resource) => match &resource.resource {
            EmbeddedResourceResource::TextResourceContents(text) => text.text.as_str(),
            EmbeddedResourceResource::BlobResourceContents(_) => "",
            _ => "",
        },
        _ => "",
    }
}

fn slice_lines(content: &str, start_line: usize, max_lines: usize) -> String {
    let skip = start_line.saturating_sub(1);
    let mut lines = content.lines().skip(skip);
    if max_lines != usize::MAX {
        lines
            .by_ref()
            .take(max_lines)
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        lines.collect::<Vec<_>>().join("\n")
    }
}

async fn kill_child_if_running(child: &mut Child) -> std::io::Result<()> {
    if child.try_wait()?.is_none() {
        debug!("terminating ACP terminal process");
        child.kill().await?;
    }
    let _ = child.wait().await;
    Ok(())
}

fn to_terminal_exit_status(status: std::process::ExitStatus) -> TerminalExitStatus {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;

        let signal = status.signal().map(|s| s.to_string());
        TerminalExitStatus::new()
            .exit_code(status.code().map(|c| c as u32))
            .signal(signal)
    }

    #[cfg(not(unix))]
    {
        TerminalExitStatus::new().exit_code(status.code().map(|c| c as u32))
    }
}

fn stop_reason_label(stop_reason: StopReason) -> &'static str {
    match stop_reason {
        StopReason::EndTurn => "end_turn",
        StopReason::MaxTokens => "max_tokens",
        StopReason::MaxTurnRequests => "max_turn_requests",
        StopReason::Refusal => "refusal",
        StopReason::Cancelled => "cancelled",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_from_start_respects_utf8_boundary() {
        let mut value = "abc你好".to_string();
        let mut truncated = false;
        truncate_from_start(&mut value, 5, &mut truncated);
        assert!(truncated);
        assert_eq!(value, "好");
    }

    #[test]
    fn slice_lines_applies_start_and_limit() {
        let content = "l1\nl2\nl3\nl4";
        let sliced = slice_lines(content, 2, 2);
        assert_eq!(sliced, "l2\nl3");
    }

    #[test]
    fn extract_content_text_supports_text_and_resource() {
        let text = ContentBlock::from("hello");
        assert_eq!(extract_content_text(&text), "hello");

        let resource = ContentBlock::Resource(agent_client_protocol::EmbeddedResource::new(
            EmbeddedResourceResource::TextResourceContents(
                agent_client_protocol::TextResourceContents::new("body", "uri://demo"),
            ),
        ));
        assert_eq!(extract_content_text(&resource), "body");
    }
}
