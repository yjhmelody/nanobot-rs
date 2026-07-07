//! End-to-end integration tests for the nanobot CLI binary.
//!
//! These tests spawn the actual `nanobot` binary against a mock LLM API server
//! to verify the full tool-use -> session-persistence pipeline without needing
//! a real provider credential.
//!
//! ## Test strategy
//!
//! 1. A `MockChatServer` listens on a random local port and responds to
//!    chat completion API calls (both `anthropic/messages` and
//!    `custom/responses` protocols).
//! 2. The mock server simulates a multi-turn tool-use scenario:
//!    - First response: tool call `write_file`
//!    - Second response: tool call `read_file`
//!    - Third response: final text (containing "E2E_SUCCESS")
//! 3. A temp home directory is created with a minimal config pointing at
//!    the mock server.
//! 4. The `nanobot` binary is invoked with `--message` (one-shot mode).
//! 5. Assertions verify the output, generated files, and session persistence.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{Mutex, oneshot};

/// A minimal HTTP server that simulates an LLM chat API for testing.
///
/// Supports both Anthropic (`/messages`) and custom (`/responses`) protocol
/// endpoints. The mock implements a deterministic multi-turn script:
/// write_file -> read_file -> final answer containing "E2E_SUCCESS".
struct MockChatServer {
    /// Address the server is listening on.
    addr: SocketAddr,
    /// Record of all received request bodies (for post-test assertions).
    requests: Arc<Mutex<Vec<Value>>>,
    /// Sender to signal shutdown.
    shutdown_tx: Option<oneshot::Sender<()>>,
    /// Handle to the background server task.
    join_handle: Option<tokio::task::JoinHandle<()>>,
}

impl MockChatServer {
    /// Start the mock server on a random available port.
    async fn start() -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .context("bind mock chat server")?;
        let addr = listener.local_addr().context("get mock server addr")?;

        let requests = Arc::new(Mutex::new(Vec::new()));
        let requests_for_loop = requests.clone();

        let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();
        let join_handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => {
                        break;
                    }
                    accepted = listener.accept() => {
                        let Ok((mut stream, _peer)) = accepted else {
                            break;
                        };
                        let requests = requests_for_loop.clone();
                        tokio::spawn(async move {
                            let _ = handle_connection(&mut stream, requests).await;
                        });
                    }
                }
            }
        });

        Ok(Self {
            addr,
            requests,
            shutdown_tx: Some(shutdown_tx),
            join_handle: Some(join_handle),
        })
    }

    /// Return the base URL for the API server.
    fn api_base(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Return the number of requests received.
    async fn request_count(&self) -> usize {
        self.requests.lock().await.len()
    }

    /// Gracefully shut down the mock server.
    async fn shutdown(mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(handle) = self.join_handle.take() {
            let _ = handle.await;
        }
    }
}

/// Which chat completion protocol the mock server should emulate.
#[derive(Debug, Clone, Copy)]
enum MockProtocol {
    /// OpenAI-style `/responses` endpoint.
    Responses,
    /// Anthropic-style `/messages` endpoint.
    Anthropic,
}

/// Tracks which tools the mock agent has called so far in the conversation.
///
/// Used by `build_response` to decide what to reply next (write_file -> read_file -> final).
#[derive(Debug, Default)]
struct MockToolState {
    /// Whether `write_file` has been called by the agent.
    wrote: bool,
    /// Whether `read_file` has been called by the agent.
    read: bool,
    /// Content of the most recent read_file result.
    read_content: String,
}

/// Handle a single HTTP connection to the mock server.
///
/// Parses the HTTP request, validates the protocol, records the request body,
/// and returns a deterministic multi-turn response simulating tool use.
async fn handle_connection(
    stream: &mut tokio::net::TcpStream,
    requests: Arc<Mutex<Vec<Value>>>,
) -> Result<(u16, Value)> {
    let mut raw = Vec::new();
    let mut buf = [0u8; 4096];

    let header_end = loop {
        let n = stream
            .read(&mut buf)
            .await
            .context("read incoming http request")?;
        if n == 0 {
            return Ok((200, json!({"ok": true})));
        }
        raw.extend_from_slice(&buf[..n]);
        if let Some(idx) = find_header_end(&raw) {
            break idx;
        }
        if raw.len() > 2 * 1024 * 1024 {
            bail!("request header too large")
        }
    };

    let headers_raw = &raw[..header_end];
    let headers = String::from_utf8_lossy(headers_raw);
    let mut lines = headers.split("\r\n");
    let start_line = lines.next().unwrap_or_default();
    let mut start_parts = start_line.split_whitespace();
    let method = start_parts.next().unwrap_or_default();
    let path = start_parts.next().unwrap_or_default();

    let mut content_length = 0usize;
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            content_length = value.trim().parse::<usize>().unwrap_or(0);
        }
    }

    let body_start = header_end + 4;
    let mut body = raw[body_start..].to_vec();
    while body.len() < content_length {
        let n = stream
            .read(&mut buf)
            .await
            .context("read http request body")?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&buf[..n]);
    }

    let (status, payload) = match (method, path) {
        ("GET", "/health") => (200u16, json!({"ok": true})),
        ("POST", _) => {
            let Some(protocol) = mock_protocol(path) else {
                return Ok((404u16, json!({"error": "not found"})));
            };

            let req: Value = match serde_json::from_slice(&body) {
                Ok(v) => v,
                Err(err) => {
                    let payload = json!({"error": format!("invalid json: {}", err)});
                    return Ok((400u16, payload));
                }
            };

            match protocol_request_valid(protocol, &req) {
                Ok(()) => {
                    requests.lock().await.push(req.clone());
                    (200u16, build_response(protocol, &req))
                }
                Err(message) => (400u16, json!({"error": message.to_string()})),
            }
        }
        _ => (404u16, json!({"error": "not found"})),
    };

    let payload_bytes = serde_json::to_vec(&payload).context("serialize mock response")?;
    let status_text = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        _ => "OK",
    };

    let head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        status,
        status_text,
        payload_bytes.len()
    );

    stream
        .write_all(head.as_bytes())
        .await
        .context("write response header")?;
    stream
        .write_all(&payload_bytes)
        .await
        .context("write response body")?;
    stream.flush().await.context("flush response")?;

    Ok((status, payload))
}

/// Find the end of HTTP headers (`\r\n\r\n`) in a byte buffer.
fn find_header_end(input: &[u8]) -> Option<usize> {
    input.windows(4).position(|w| w == b"\r\n\r\n")
}

/// Determine which protocol to use based on the request URL path.
fn mock_protocol(path: &str) -> Option<MockProtocol> {
    if path.ends_with("/responses") {
        Some(MockProtocol::Responses)
    } else if path.ends_with("/messages") {
        Some(MockProtocol::Anthropic)
    } else {
        None
    }
}

/// Validate that the request has the required fields for the given protocol.
fn protocol_request_valid(protocol: MockProtocol, req: &Value) -> Result<()> {
    match protocol {
        MockProtocol::Anthropic => {
            if req.get("messages").is_none() {
                bail!("missing messages");
            }
        }
        MockProtocol::Responses => {
            if req.get("input").is_none() {
                bail!("missing input");
            }
        }
    }

    Ok(())
}

/// Build a deterministic multi-turn mock response.
///
/// Implements the state machine:
/// 1. If `write_file` hasn't been called, respond with a `write_file` tool call.
/// 2. If `read_file` hasn't been called, respond with a `read_file` tool call.
/// 3. Otherwise, respond with final text containing "E2E_SUCCESS" (or "E2E_FAILURE").
fn build_response(protocol: MockProtocol, req: &Value) -> Value {
    let state = match protocol {
        MockProtocol::Responses => tool_state_from_responses(req),
        MockProtocol::Anthropic => tool_state_from_anthropic(req),
    };

    if !state.wrote {
        return response_tool_call(
            protocol,
            "call_write_1",
            "write_file",
            json!({"path": "e2e/result.txt", "content": "NANOBOT_E2E_OK"}),
        );
    }

    if !state.read {
        return response_tool_call(
            protocol,
            "call_read_1",
            "read_file",
            json!({"path": "e2e/result.txt"}),
        );
    }

    if state.read_content.contains("NANOBOT_E2E_OK") {
        response_final(protocol, "E2E_SUCCESS")
    } else {
        response_final(
            protocol,
            &format!(
                "E2E_FAILURE: unexpected read content: {:?}",
                state.read_content
            ),
        )
    }
}

/// Extract tool call tracking state from an OpenAI-style `/responses` request body.
fn tool_state_from_responses(req: &Value) -> MockToolState {
    let input = req
        .get("input")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut call_names = HashMap::<String, String>::new();
    let mut state = MockToolState::default();

    for item in input {
        match item.get("type").and_then(Value::as_str) {
            Some("function_call") => {
                if let (Some(call_id), Some(name)) = (
                    item.get("call_id").and_then(Value::as_str),
                    item.get("name").and_then(Value::as_str),
                ) {
                    call_names.insert(call_id.to_string(), name.to_string());
                }
            }
            Some("function_call_output") => {
                let Some(call_id) = item.get("call_id").and_then(Value::as_str) else {
                    continue;
                };
                let Some(name) = call_names.get(call_id).map(String::as_str) else {
                    continue;
                };

                match name {
                    "write_file" => state.wrote = true,
                    "read_file" => {
                        state.read = true;
                        state.read_content = item
                            .get("output")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    state
}

/// Extract tool call tracking state from an Anthropic-style `/messages` request body.
fn tool_state_from_anthropic(req: &Value) -> MockToolState {
    let messages = req
        .get("messages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut tool_names = HashMap::<String, String>::new();
    let mut state = MockToolState::default();

    for message in messages {
        let Some(content) = message.get("content").and_then(Value::as_array) else {
            continue;
        };

        for block in content {
            match block.get("type").and_then(Value::as_str) {
                Some("tool_use") => {
                    if let (Some(id), Some(name)) = (
                        block.get("id").and_then(Value::as_str),
                        block.get("name").and_then(Value::as_str),
                    ) {
                        tool_names.insert(id.to_string(), name.to_string());
                    }
                }
                Some("tool_result") => {
                    let Some(tool_use_id) = block.get("tool_use_id").and_then(Value::as_str) else {
                        continue;
                    };
                    let Some(name) = tool_names.get(tool_use_id).map(String::as_str) else {
                        continue;
                    };

                    match name {
                        "write_file" => state.wrote = true,
                        "read_file" => {
                            state.read = true;
                            state.read_content =
                                anthropic_tool_result_text(block.get("content")).to_string();
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }

    state
}

/// Extract the text content from an Anthropic tool result response block.
fn anthropic_tool_result_text(content: Option<&Value>) -> &str {
    match content {
        Some(Value::String(s)) => s,
        Some(Value::Array(items)) => items
            .iter()
            .find_map(|item| item.get("text").and_then(Value::as_str))
            .unwrap_or_default(),
        _ => "",
    }
}

/// Build a JSON response for a tool call, in the format expected by the given protocol.
fn response_tool_call(protocol: MockProtocol, id: &str, name: &str, arguments: Value) -> Value {
    match protocol {
        MockProtocol::Responses => json!({
            "output": [
                {
                    "type": "function_call",
                    "call_id": id,
                    "name": name,
                    "arguments": arguments.to_string()
                }
            ],
            "usage": {
                "input_tokens": 1,
                "output_tokens": 1,
                "total_tokens": 2
            }
        }),
        MockProtocol::Anthropic => json!({
            "content": [
                {
                    "type": "tool_use",
                    "id": id,
                    "name": name,
                    "input": arguments
                }
            ],
            "stop_reason": "tool_use",
            "usage": {
                "input_tokens": 1,
                "output_tokens": 1
            }
        }),
    }
}

/// Build a JSON response for a final text answer, in the format expected by the given protocol.
fn response_final(protocol: MockProtocol, content: &str) -> Value {
    match protocol {
        MockProtocol::Responses => json!({
            "output": [
                {
                    "type": "message",
                    "role": "assistant",
                    "content": [
                        {
                            "type": "output_text",
                            "text": content
                        }
                    ]
                }
            ],
            "usage": {
                "input_tokens": 1,
                "output_tokens": 1,
                "total_tokens": 2
            }
        }),
        MockProtocol::Anthropic => json!({
            "content": [
                {
                    "type": "text",
                    "text": content
                }
            ],
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 1,
                "output_tokens": 1
            }
        }),
    }
}

/// Resolve the path to the `nanobot` binary for tests.
///
/// Uses `CARGO_BIN_EXE_nanobot` if available (set by `cargo test`), otherwise
/// falls back to `target/debug/nanobot` relative to the workspace root.
fn binary_path() -> PathBuf {
    if let Some(path) = std::env::var_os("CARGO_BIN_EXE_nanobot") {
        return PathBuf::from(path);
    }

    // Integration tests run from the package directory, not necessarily from the
    // workspace root, so derive an absolute fallback path from the manifest dir.
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root from manifest dir")
        .to_path_buf();

    #[cfg(target_os = "windows")]
    {
        return workspace_root.join("target/debug/nanobot.exe");
    }

    #[cfg(not(target_os = "windows"))]
    {
        workspace_root.join("target/debug/nanobot")
    }
}

/// Run the `nanobot` binary as a subprocess with the given arguments and env.
///
/// Captures stdout and stderr, with a 120-second timeout. Both `HOME` and
/// `USERPROFILE` are set to `home` for cross-platform home-directory resolution.
async fn run_nanobot(
    bin: &Path,
    home: &Path,
    args: &[&str],
    extra_env: &[(&str, &str)],
) -> Result<Output> {
    let mut cmd = tokio::process::Command::new(bin);
    // `dirs::home_dir()` relies on different environment variables depending on
    // the platform, so the tests pin both to the temp directory.
    cmd.args(args)
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("NO_PROXY", "127.0.0.1,localhost")
        .env("no_proxy", "127.0.0.1,localhost")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    for (k, v) in extra_env {
        cmd.env(k, v);
    }

    let mut child = cmd
        .spawn()
        .with_context(|| format!("spawn {:?} {:?}", bin, args))?;

    let stdout = child
        .stdout
        .take()
        .context("child process stdout is not piped")?;
    let stderr = child
        .stderr
        .take()
        .context("child process stderr is not piped")?;

    let stdout_task = tokio::spawn(async move {
        let mut reader = tokio::io::BufReader::new(stdout);
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).await.map(|_| buf)
    });
    let stderr_task = tokio::spawn(async move {
        let mut reader = tokio::io::BufReader::new(stderr);
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).await.map(|_| buf)
    });

    let mut timed_out = false;
    let status = match tokio::time::timeout(Duration::from_secs(120), child.wait()).await {
        Ok(wait) => wait.context("wait child process")?,
        Err(_) => {
            timed_out = true;
            let _ = child.kill().await;
            child.wait().await.context("wait killed child process")?
        }
    };

    let stdout = stdout_task
        .await
        .context("join child stdout task")?
        .context("read child stdout")?;
    let stderr = stderr_task
        .await
        .context("join child stderr task")?
        .context("read child stderr")?;

    let out = Output {
        status,
        stdout,
        stderr,
    };

    if timed_out {
        bail!(
            "command timed out after 120s: {:?} {:?}\n{}",
            bin,
            args,
            output_text(&out)
        );
    }

    Ok(out)
}

/// Format a process `Output` as a readable string (stdout + stderr with UTF-8 lossy).
fn output_text(output: &Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    format!("{}\n{}", stdout, stderr)
}

/// Assert that the subprocess exited successfully, panicking with full output on failure.
fn assert_success(output: &Output, label: &str) {
    if !output.status.success() {
        panic!(
            "{} failed with status {:?}\n{}",
            label,
            output.status.code(),
            output_text(output)
        );
    }
}

/// Write a minimal config JSON file pointing to the mock server.
fn write_config(
    config_path: &Path,
    workspace: &Path,
    api_base: &str,
    provider: &str,
    model: &str,
    mcp_codex_path: Option<&Path>,
) -> Result<()> {
    let mcp_servers = if let Some(path) = mcp_codex_path {
        json!({
            "codex": {
                "command": path.to_string_lossy(),
                "args": ["mcp-server"],
                "env": {},
                "toolTimeout": 30
            }
        })
    } else {
        json!({})
    };

    let config = json!({
        "agents": {
            "defaults": {
                "workspace": workspace,
                "provider": provider,
                "model": model,
                "maxToolIterations": 8,
                "temperature": 0.0
            }
        },
        "providers": {
            provider: {
                "apiBase": api_base,
                "apiKey": "e2e-local"
            }
        },
        "tools": {
            "restrictToWorkspace": true,
            "exec": {
                "timeout": 20,
                "pathAppend": ""
            },
            "mcpServers": mcp_servers
        }
    });

    std::fs::write(config_path, serde_json::to_vec_pretty(&config)?)
        .with_context(|| format!("write config {}", config_path.display()))?;
    Ok(())
}

/// Full E2E test: onboard, status, one-shot agent with mock custom provider.
///
/// Verifies:
/// - `onboard --overwrite` creates config and workspace templates.
/// - `status` prints expected output.
/// - One-shot agent processes a task end-to-end using tool calls.
/// - Session file contains tool-call records.
/// - Mock server received at least 3 requests (write_file, read_file, final).
#[tokio::test]
async fn e2e_cli_runtime_tools_session_offline() -> Result<()> {
    let server = MockChatServer::start().await?;

    let home = TempDir::new().context("create temp home")?;
    let home_path = home.path();
    let nanobot_home = home_path.join(".nanobot");
    std::fs::create_dir_all(&nanobot_home).context("create ~/.nanobot")?;

    let bin = binary_path();

    eprintln!("[e2e] step: onboard");
    let onboard = run_nanobot(&bin, home_path, &["onboard", "--overwrite"], &[]).await?;
    assert_success(&onboard, "onboard");

    let default_workspace = nanobot_home.join("workspace");
    for rel in [
        "AGENTS.md",
        "SOUL.md",
        "USER.md",
        "TOOLS.md",
        "HEARTBEAT.md",
        "memory/MEMORY.md",
        "memory/HISTORY.md",
    ] {
        let path = default_workspace.join(rel);
        assert!(path.exists(), "missing template {}", path.display());
    }

    let workspace = home_path.join("runtime-workspace");
    std::fs::create_dir_all(&workspace).context("create runtime workspace")?;
    let workspace = std::fs::canonicalize(&workspace).context("canonicalize runtime workspace")?;
    write_config(
        &nanobot_home.join("config.json"),
        &workspace,
        &server.api_base(),
        "custom",
        "custom/mock-model",
        None,
    )?;

    eprintln!("[e2e] step: status");
    let status = run_nanobot(&bin, home_path, &["status"], &[]).await?;
    assert_success(&status, "status");
    let status_text = output_text(&status);
    assert!(status_text.contains("nanobot Status"));
    assert!(status_text.contains("Workspace:"));

    eprintln!("[e2e] step: agent");
    let agent = run_nanobot(
        &bin,
        home_path,
        &["agent", "-m", "执行本地E2E场景", "-s", "cli:e2e"],
        &[],
    )
    .await?;
    assert_success(&agent, "agent");

    let agent_text = output_text(&agent);
    assert!(
        agent_text.contains("E2E_SUCCESS"),
        "agent output missing E2E_SUCCESS:\n{}",
        agent_text
    );

    let generated = workspace.join("e2e/result.txt");
    assert!(
        generated.exists(),
        "missing generated file {}",
        generated.display()
    );
    let generated_text = std::fs::read_to_string(&generated)
        .with_context(|| format!("read generated file {}", generated.display()))?;
    assert_eq!(generated_text, "NANOBOT_E2E_OK");

    let session_file = workspace.join("sessions/cli_e2e.jsonl");
    assert!(
        session_file.exists(),
        "missing session file {}",
        session_file.display()
    );
    let session_text = std::fs::read_to_string(&session_file)
        .with_context(|| format!("read session file {}", session_file.display()))?;
    let mut has_tool_calls = false;
    for line in session_text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let is_assistant = v.get("role").and_then(Value::as_str) == Some("assistant");
        let tool_calls = v.get("toolCalls").and_then(Value::as_array);
        if is_assistant && tool_calls.map(|x| !x.is_empty()).unwrap_or(false) {
            has_tool_calls = true;
            break;
        }
    }
    assert!(
        has_tool_calls,
        "session file missing assistant toolCalls records"
    );

    assert!(
        server.request_count().await >= 3,
        "mock server request count too low"
    );

    server.shutdown().await;
    Ok(())
}

/// Same E2E test as above but using the Anthropic `/messages` API protocol.
#[tokio::test]
async fn e2e_cli_runtime_tools_session_offline_anthropic() -> Result<()> {
    let server = MockChatServer::start().await?;

    let home = TempDir::new().context("create temp home")?;
    let home_path = home.path();
    let nanobot_home = home_path.join(".nanobot");
    std::fs::create_dir_all(&nanobot_home).context("create ~/.nanobot")?;

    let bin = binary_path();
    let onboard = run_nanobot(&bin, home_path, &["onboard", "--overwrite"], &[]).await?;
    assert_success(&onboard, "onboard anthropic");

    let workspace = home_path.join("runtime-workspace");
    std::fs::create_dir_all(&workspace).context("create runtime workspace")?;
    let workspace = std::fs::canonicalize(&workspace).context("canonicalize runtime workspace")?;

    write_config(
        &nanobot_home.join("config.json"),
        &workspace,
        &server.api_base(),
        "anthropic",
        "anthropic/claude-opus-4-5",
        None,
    )?;

    let agent = run_nanobot(
        &bin,
        home_path,
        &[
            "agent",
            "-m",
            "执行 Claude 本地E2E场景",
            "-s",
            "cli:anthropic-e2e",
        ],
        &[],
    )
    .await?;
    assert_success(&agent, "agent anthropic");

    let agent_text = output_text(&agent);
    assert!(
        agent_text.contains("E2E_SUCCESS"),
        "anthropic agent output missing E2E_SUCCESS:\n{}",
        agent_text
    );

    let generated = workspace.join("e2e/result.txt");
    assert!(
        generated.exists(),
        "missing anthropic generated file {}",
        generated.display()
    );

    assert!(
        server.request_count().await >= 3,
        "anthropic mock server request count too low"
    );

    server.shutdown().await;
    Ok(())
}

/// MCP connectivity smoke test: verifies the agent can connect to a `codex`
/// MCP server.
///
/// Ignored by default — requires `codex` to be installed on the system PATH.
#[tokio::test]
#[ignore = "requires local `codex` binaries installed"]
async fn codex_mcp_connect_smoke() -> Result<()> {
    let codex = which::which("codex").context("codex not found in PATH")?;

    let server = MockChatServer::start().await?;

    let home = TempDir::new().context("create temp home")?;
    let home_path = home.path();
    let nanobot_home = home_path.join(".nanobot");
    std::fs::create_dir_all(&nanobot_home).context("create ~/.nanobot")?;

    let workspace = home_path.join("runtime-workspace");
    std::fs::create_dir_all(&workspace).context("create runtime workspace")?;
    let workspace = std::fs::canonicalize(&workspace).context("canonicalize runtime workspace")?;
    write_config(
        &nanobot_home.join("config.json"),
        &workspace,
        &server.api_base(),
        "custom",
        "custom/mock-model",
        Some(&codex),
    )?;

    let bin = binary_path();
    let agent = run_nanobot(
        &bin,
        home_path,
        &["agent", "-m", "MCP connect smoke", "-s", "cli:mcp"],
        &[("RUST_LOG", "nanobot::tools=info")],
    )
    .await?;

    let text = output_text(&agent);
    assert!(
        text.contains("MCP server 'codex': connected"),
        "MCP connect log missing:\n{}",
        text
    );

    server.shutdown().await;
    Ok(())
}
