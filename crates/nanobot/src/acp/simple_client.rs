//! Local ACP client-side handler implementation.
//!
//! `SimpleClient` implements the `agent_client_protocol::Client` trait,
//! providing the local runtime that handles filesystem reads/writes,
//! terminal process lifecycle, session notification buffering, and
//! permission auto-approval for ACP agents.
//!
//! ## Capabilities
//!
//! By default `SimpleClient` advertises both filesystem and terminal
//! capabilities to the ACP agent. A `prompt_only` mode disables both,
//! restricting the agent to text generation.
//!
//! ## Session Buffering
//!
//! Each ACP turn accumulates agent output (message chunks, tool calls,
//! plans) into a session-specific string buffer. The buffer is created
//! by `begin_turn`, appended to by `session_notification`, and drained
//! by `take_turn_output`. Progress snapshots are available via
//! `turn_snapshot` for logging during long-running turns.
//!
//! ## Terminal Management
//!
//! The client tracks spawned terminal processes by `TerminalId`. Each
//! terminal has a background reader task that captures stdout/stderr
//! into an output buffer. Output can be truncated from the start if a
//! byte limit is configured.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Instant;

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

/// A local ACP client handler that provides filesystem, terminal, and
/// session buffering capabilities.
///
/// `SimpleClient` implements the `agent_client_protocol::Client` trait which
/// the ACP `ClientSideConnection` calls to fulfil agent requests. It is
/// thread-safe via `Arc<Mutex<SimpleClientState>>` and is `Clone` for
/// sharing across concurrent ACP sessions.
///
/// # Fields
///
/// * `state` — Shared mutable state: session buffers, turn metadata, terminals.
/// * `allow_fs` — Whether filesystem capabilities are advertised.
/// * `allow_terminal` — Whether terminal capabilities are advertised.
#[derive(Clone)]
pub struct SimpleClient {
    state: Arc<Mutex<SimpleClientState>>,
    allow_fs: bool,
    allow_terminal: bool,
}

/// Snapshot of an in-progress ACP turn for logging progress.
#[derive(Debug, Clone)]
pub struct TurnSnapshot {
    /// Seconds elapsed since `begin_turn`.
    pub elapsed_secs: u64,
    /// Seconds since the last session update.
    pub idle_secs: u64,
    /// Number of `SessionUpdate` notifications received this turn.
    pub updates_count: usize,
    /// Current size of the buffered output (in bytes).
    pub buffer_bytes: usize,
    /// Kind of the most recent session update (e.g., "agent_message_chunk").
    pub last_update_kind: Option<&'static str>,
}

impl SimpleClient {
    /// Create a new `SimpleClient` with full filesystem and terminal capabilities.
    ///
    /// # Arguments
    ///
    /// * `default_cwd` — Default working directory for spawned terminal processes.
    pub fn new(default_cwd: PathBuf) -> Self {
        Self::with_permissions(default_cwd, true, true)
    }

    /// Create a `SimpleClient` restricted to text-only prompts.
    ///
    /// No filesystem or terminal capabilities are advertised, so the ACP
    /// agent will only be able to produce text output.
    #[allow(unused)]
    pub fn prompt_only(default_cwd: PathBuf) -> Self {
        Self::with_permissions(default_cwd, false, false)
    }

    /// Internal constructor with explicit permission flags.
    fn with_permissions(default_cwd: PathBuf, allow_fs: bool, allow_terminal: bool) -> Self {
        Self {
            state: Arc::new(Mutex::new(SimpleClientState::new(default_cwd))),
            allow_fs,
            allow_terminal,
        }
    }

    /// Build the `ClientCapabilities` struct to advertise during ACP initialisation.
    ///
    /// The capabilities reflect the `allow_fs` and `allow_terminal` flags
    /// set at construction time.
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

    /// Initialise a new turn for the given session.
    ///
    /// Creates an empty output buffer and records the start time. Must be
    /// called before `take_turn_output`.
    pub async fn begin_turn(&self, session_id: &SessionId) {
        let mut state = self.state.lock().await;
        state
            .session_buffers
            .insert(session_id.clone(), String::new());
        state.session_turn_meta.insert(
            session_id.clone(),
            SessionTurnMeta {
                started_at: Instant::now(),
                last_update_at: Instant::now(),
                updates_count: 0,
                last_update_kind: None,
            },
        );
    }

    /// Finalise the turn and return the accumulated agent output.
    ///
    /// Removes and returns the session buffer. If the buffer is empty,
    /// returns a descriptive string like `"(ACP turn finished: end_turn)"`.
    ///
    /// # Arguments
    ///
    /// * `session_id` — The session whose output to take.
    /// * `stop_reason` — The reason the agent finished (used for the fallback string).
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
        state.session_turn_meta.remove(session_id);
        if output.is_empty() {
            format!("(ACP turn finished: {})", stop_reason_label(stop_reason))
        } else {
            output
        }
    }

    /// Return a `TurnSnapshot` for the given session, or `None` if no turn is active.
    ///
    /// Used by `ACPActor` for periodic progress logging during long-running prompts.
    pub async fn turn_snapshot(&self, session_id: &SessionId) -> Option<TurnSnapshot> {
        let state = self.state.lock().await;
        let meta = state.session_turn_meta.get(session_id)?;
        let buffer_bytes = state
            .session_buffers
            .get(session_id)
            .map(|value| value.len())
            .unwrap_or(0);
        Some(TurnSnapshot {
            elapsed_secs: meta.started_at.elapsed().as_secs(),
            idle_secs: meta.last_update_at.elapsed().as_secs(),
            updates_count: meta.updates_count,
            buffer_bytes,
            last_update_kind: meta.last_update_kind,
        })
    }

    /// Kill and clean up all tracked terminal processes.
    ///
    /// Drains the terminal map and attempts to kill each child process.
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

/// ACP `Client` trait implementation.
///
/// The `?Send` marker is required by the ACP SDK because the connection
/// object is not `Send`.
#[async_trait::async_trait(?Send)]
impl Client for SimpleClient {
    /// Auto-approve the first permission option (or cancel if none available).
    ///
    /// This means the ACP agent's permission requests are always granted
    /// by default — appropriate for a trusted local agent.
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

    /// Record a session update notification into the turn buffer.
    ///
    /// Handles agent message chunks, tool calls/updates, and plan entries.
    async fn session_notification(&self, args: SessionNotification) -> Result<()> {
        let mut state = self.state.lock().await;
        state.record_session_update(args);
        Ok(())
    }

    /// Write text content to a file (creating parent directories if needed).
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

    /// Read text content from a file, with optional line-range slicing.
    ///
    /// # Errors
    ///
    /// Returns a `ResourceNotFound` error if the file does not exist.
    /// Returns an `InvalidParams` error if `line` is 0 (lines are 1-based).
    async fn read_text_file(&self, args: ReadTextFileRequest) -> Result<ReadTextFileResponse> {
        let content = tokio::fs::read_to_string(&args.path).await.map_err(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                Error::resource_not_found(Some(args.path.to_string_lossy().to_string()))
            } else {
                Error::into_internal_error(err)
            }
        })?;

        // Lines are 1-based per the ACP spec.
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

    /// Create a terminal (subprocess) with the given command and environment.
    ///
    /// Spawns the process, captures stdout/stderr via background reader
    /// tasks, and returns a `TerminalId` for subsequent operations.
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

    /// Get the current output and exit status of a terminal.
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
            child
                .try_wait()
                .map_err(Error::into_internal_error)?
                .map(to_terminal_exit_status)
        };

        Ok(TerminalOutputResponse::new(output, truncated).exit_status(exit_status))
    }

    /// Release (kill and clean up) a terminal by its ID.
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

    /// Wait for a terminal process to exit and return its exit status.
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

    /// Kill a terminal process by its ID (non-blocking, no wait).
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

/// Mutable state shared across all session and terminal operations.
///
/// Protected by `tokio::sync::Mutex` because operations may hold the lock
/// across await points (e.g., during `record_session_update`).
#[derive(Default)]
struct SimpleClientState {
    default_cwd: PathBuf,
    /// Per-session output buffers, keyed by `SessionId`.
    session_buffers: HashMap<SessionId, String>,
    /// Per-session turn metadata (start time, update count, etc.).
    session_turn_meta: HashMap<SessionId, SessionTurnMeta>,
    /// Tracked terminal subprocesses, keyed by `TerminalId`.
    terminals: HashMap<TerminalId, TerminalEntry>,
    /// Monotonically increasing counter for generating terminal IDs.
    next_terminal_id: u64,
}

/// Metadata tracked for an in-progress ACP turn.
struct SessionTurnMeta {
    /// Instant when `begin_turn` was called.
    started_at: Instant,
    /// Instant of the most recent session update.
    last_update_at: Instant,
    /// Number of `SessionUpdate` notifications received.
    updates_count: usize,
    /// Kind of the most recent session update (for progress logging).
    last_update_kind: Option<&'static str>,
}

impl SimpleClientState {
    fn new(default_cwd: PathBuf) -> Self {
        Self {
            default_cwd,
            session_buffers: HashMap::new(),
            session_turn_meta: HashMap::new(),
            terminals: HashMap::new(),
            next_terminal_id: 0,
        }
    }

    /// Generate a new unique `TerminalId`.
    fn next_terminal_id(&mut self) -> TerminalId {
        self.next_terminal_id += 1;
        TerminalId::new(format!("nanobot-terminal-{}", self.next_terminal_id))
    }

    /// Process a `SessionNotification` and append the content to the session buffer.
    ///
    /// Handles the following update types:
    /// - `AgentMessageChunk` — agent text output
    /// - `ToolCall` / `ToolCallUpdate` — tool invocation metadata
    /// - `Plan` — structured plan entries
    fn record_session_update(&mut self, notification: SessionNotification) {
        let Some(buffer) = self.session_buffers.get_mut(&notification.session_id) else {
            // No active turn for this session — ignore the update.
            return;
        };

        if let Some(meta) = self.session_turn_meta.get_mut(&notification.session_id) {
            meta.last_update_at = Instant::now();
            meta.updates_count += 1;
            meta.last_update_kind = Some(session_update_kind(&notification.update));
        }

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
            SessionUpdate::Plan(plan) if !plan.entries.is_empty() => {
                buffer.push_str("\n[plan]\n");
                for entry in plan.entries {
                    buffer.push_str("- ");
                    buffer.push_str(entry.content.trim());
                    buffer.push('\n');
                }
            }
            // Other update types (e.g., background tasks) are not buffered.
            _ => {}
        }
    }
}

/// Map a `SessionUpdate` variant to a human-readable label for progress logging.
fn session_update_kind(update: &SessionUpdate) -> &'static str {
    match update {
        SessionUpdate::AgentMessageChunk(_) => "agent_message_chunk",
        SessionUpdate::ToolCall(_) => "tool_call",
        SessionUpdate::ToolCallUpdate(_) => "tool_call_update",
        SessionUpdate::Plan(_) => "plan",
        _ => "other",
    }
}

/// A tracked terminal subprocess with its output buffer.
///
/// Both fields are `Arc<Mutex<...>>` because the background reader task
/// shares ownership with the ACP client.
#[derive(Clone)]
struct TerminalEntry {
    child: Arc<Mutex<Child>>,
    output: Arc<Mutex<TerminalOutputState>>,
}

/// Output buffer for a single terminal process.
struct TerminalOutputState {
    /// Accumulated stdout/stderr content.
    content: String,
    /// Whether the output has been truncated due to `output_byte_limit`.
    truncated: bool,
    /// Optional maximum byte limit for buffered output.
    output_byte_limit: Option<usize>,
}

/// Spawn a background task that reads from a terminal's stdout or stderr
/// pipe and appends the data to the shared output buffer.
///
/// Applies output byte limiting (truncation from the start) when configured.
fn spawn_terminal_reader<R>(mut reader: R, output: Arc<Mutex<TerminalOutputState>>)
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut chunk = vec![0u8; 8192];
        loop {
            match reader.read(&mut chunk).await {
                Ok(0) => break, // EOF
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

/// Truncate a string from the start to fit within `max_bytes`, working at
/// UTF-8 character boundaries to avoid splitting multi-byte sequences.
///
/// Sets `truncated` to `true` if any data was removed.
fn truncate_from_start(value: &mut String, max_bytes: usize, truncated: &mut bool) {
    if value.len() <= max_bytes {
        return;
    }
    *truncated = true;

    // Find the start position that gives us the last `max_bytes`, aligned
    // to a UTF-8 char boundary.
    let mut start = value.len().saturating_sub(max_bytes);
    while start < value.len() && !value.is_char_boundary(start) {
        start += 1;
    }
    *value = value[start..].to_string();
}

/// Append the text representation of a `ToolCallContent` to the buffer.
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
        // Other variants (background tasks, etc.) are not rendered.
        _ => {}
    }
}

/// Append the text content of a `ContentBlock` to the buffer, ensuring a
/// trailing newline.
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

/// Extract the text content from a `ContentBlock`, returning an empty string
/// for blocks that do not contain text (e.g., blob resources).
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

/// Slice lines of a string by 1-based start line and maximum line count.
///
/// Returns all remaining lines when `max_lines` is `usize::MAX`.
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

/// Kill a child process if it is still running, then wait for it to exit.
async fn kill_child_if_running(child: &mut Child) -> std::io::Result<()> {
    if child.try_wait()?.is_none() {
        debug!("terminating ACP terminal process");
        child.kill().await?;
    }
    let _ = child.wait().await;
    Ok(())
}

/// Convert a `std::process::ExitStatus` to the ACP `TerminalExitStatus` type.
///
/// On Unix, also captures the signal that terminated the process (if any).
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

/// Convert a `StopReason` to a human-readable label.
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

    #[tokio::test]
    async fn begin_turn_creates_snapshot_and_take_turn_cleans_it() {
        let session_id = SessionId::new("demo-session");
        let client = SimpleClient::new(std::env::temp_dir());
        client.begin_turn(&session_id).await;

        let before = client
            .turn_snapshot(&session_id)
            .await
            .expect("snapshot exists after begin_turn");
        assert_eq!(before.updates_count, 0);
        assert_eq!(before.buffer_bytes, 0);

        let _ = client
            .take_turn_output(&session_id, StopReason::EndTurn)
            .await;
        assert!(client.turn_snapshot(&session_id).await.is_none());
    }
}
