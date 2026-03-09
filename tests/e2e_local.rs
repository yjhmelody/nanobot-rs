use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use nanobot_rs::observability;
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{Mutex, oneshot};

struct MockChatServer {
    addr: SocketAddr,
    requests: Arc<Mutex<Vec<Value>>>,
    shutdown_tx: Option<oneshot::Sender<()>>,
    join_handle: Option<tokio::task::JoinHandle<()>>,
}

impl MockChatServer {
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

    fn api_base(&self) -> String {
        format!("http://{}", self.addr)
    }

    async fn request_count(&self) -> usize {
        self.requests.lock().await.len()
    }

    async fn shutdown(mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(handle) = self.join_handle.take() {
            let _ = handle.await;
        }
    }
}

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
        ("POST", "/chat/completions") => {
            let req: Value = match serde_json::from_slice(&body) {
                Ok(v) => v,
                Err(err) => {
                    let payload = json!({"error": format!("invalid json: {}", err)});
                    return Ok((400u16, payload));
                }
            };

            if req.get("messages").is_none() {
                (400u16, json!({"error": "missing messages"}))
            } else {
                requests.lock().await.push(req.clone());
                (200u16, build_response(&req))
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

fn find_header_end(input: &[u8]) -> Option<usize> {
    input.windows(4).position(|w| w == b"\r\n\r\n")
}

fn build_response(req: &Value) -> Value {
    let messages = req
        .get("messages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let tool_messages = messages
        .iter()
        .filter(|m| m.get("role").and_then(Value::as_str) == Some("tool"))
        .collect::<Vec<_>>();

    let wrote = tool_messages
        .iter()
        .any(|m| m.get("name").and_then(Value::as_str) == Some("write_file"));
    let read = tool_messages
        .iter()
        .any(|m| m.get("name").and_then(Value::as_str) == Some("read_file"));

    if !wrote {
        return response_tool_call(
            "call_write_1",
            "write_file",
            json!({"path": "e2e/result.txt", "content": "NANOBOT_E2E_OK"}),
        );
    }

    if !read {
        return response_tool_call(
            "call_read_1",
            "read_file",
            json!({"path": "e2e/result.txt"}),
        );
    }

    let mut read_content = String::new();
    for msg in tool_messages.iter().rev() {
        if msg.get("name").and_then(Value::as_str) == Some("read_file") {
            read_content = extract_text(msg.get("content")).to_string();
            break;
        }
    }

    if read_content.contains("NANOBOT_E2E_OK") {
        response_final("E2E_SUCCESS")
    } else {
        response_final(&format!(
            "E2E_FAILURE: unexpected read content: {:?}",
            read_content
        ))
    }
}

fn extract_text(content: Option<&Value>) -> &str {
    match content {
        Some(Value::String(s)) => s,
        _ => "",
    }
}

fn response_tool_call(id: &str, name: &str, arguments: Value) -> Value {
    json!({
        "id": format!("mock-{}", id),
        "object": "chat.completion",
        "choices": [
            {
                "index": 0,
                "message": {
                    "content": Value::Null,
                    "tool_calls": [
                        {
                            "id": id,
                            "type": "function",
                            "function": {
                                "name": name,
                                "arguments": arguments.to_string(),
                            }
                        }
                    ]
                },
                "finish_reason": "tool_calls"
            }
        ],
        "usage": {
            "prompt_tokens": 1,
            "completion_tokens": 1,
            "total_tokens": 2
        }
    })
}

fn response_final(content: &str) -> Value {
    json!({
        "id": "mock-final",
        "object": "chat.completion",
        "choices": [
            {
                "index": 0,
                "message": {
                    "content": content,
                },
                "finish_reason": "stop"
            }
        ],
        "usage": {
            "prompt_tokens": 1,
            "completion_tokens": 1,
            "total_tokens": 2
        }
    })
}

fn binary_path() -> PathBuf {
    if let Some(path) = std::env::var_os("CARGO_BIN_EXE") {
        return PathBuf::from(path);
    }
    PathBuf::from("target/debug/nanobot-rs")
}

async fn run_nanobot(
    bin: &Path,
    home: &Path,
    args: &[&str],
    extra_env: &[(&str, &str)],
) -> Result<Output> {
    let mut cmd = tokio::process::Command::new(bin);
    cmd.args(args)
        .env("HOME", home)
        .env(observability::ENV_NANOBOT_OTLP_TRACES, "false")
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

fn output_text(output: &Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    format!("{}\n{}", stdout, stderr)
}

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

fn write_config(
    config_path: &Path,
    workspace: &Path,
    api_base: &str,
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
                "provider": "custom",
                "model": "custom/mock-model",
                "maxToolIterations": 8,
                "temperature": 0.0
            }
        },
        "providers": {
            "custom": {
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
        None,
    )?;

    eprintln!("[e2e] step: status");
    let status = run_nanobot(&bin, home_path, &["status"], &[]).await?;
    assert_success(&status, "status");
    let status_text = output_text(&status);
    assert!(status_text.contains("nanobot-rs Status"));
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
        Some(&codex),
    )?;

    let bin = binary_path();
    let agent = run_nanobot(
        &bin,
        home_path,
        &["agent", "-m", "MCP connect smoke", "-s", "cli:mcp"],
        &[("RUST_LOG", "nanobot.tools=info")],
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
