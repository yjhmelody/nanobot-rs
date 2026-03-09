use std::collections::BTreeMap;
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;

use async_trait::async_trait;
use http::{HeaderName, HeaderValue};
use rmcp::model::{
    CallToolRequestParams, CallToolResult, ClientInfo, ProtocolVersion, RawContent,
    Tool as MCPRemoteTool,
};
use rmcp::service::RunningService;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::{ConfigureCommandExt, StreamableHttpClientTransport, TokioChildProcess};
use rmcp::{RoleClient, ServiceExt};
use tokio::process::Command;
use tokio::sync::Mutex;
use tracing::{error, info, warn};

use crate::config::schema::MCPServerConfig;
use crate::error::{NanobotError, Result};
use crate::observability::TARGET_TOOLS;
use crate::tools::base::{JsonSchema, Tool, ToolContext, ToolDefinition};
use crate::tools::registry::ToolRegistry;

type MCPRunningClient = RunningService<RoleClient, ClientInfo>;

/// Manages MCP server lifecycle and dynamic tool registration.
pub struct MCPManager {
    servers: HashMap<String, MCPServerConfig>,
    state: Mutex<MCPManagerState>,
}

#[derive(Default)]
struct MCPManagerState {
    connection: ConnectionStatus,
    sessions: Vec<Arc<MCPClientSession>>,
    registered_tools: Vec<String>,
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy, Hash, Default)]
enum ConnectionStatus {
    #[default]
    Disconnect,
    Connected,
    Connecting,
}

impl ConnectionStatus {
    fn is_disconnect(self) -> bool {
        return self == ConnectionStatus::Disconnect;
    }
}

impl MCPManager {
    pub fn new(servers: HashMap<String, MCPServerConfig>) -> Self {
        Self {
            servers,
            state: Mutex::new(MCPManagerState::default()),
        }
    }

    pub async fn connect_if_needed(&self, registry: &ToolRegistry) -> Result<()> {
        if self.servers.is_empty() {
            return Ok(());
        }

        {
            let mut state = self.state.lock().await;
            if !state.connection.is_disconnect() {
                return Ok(());
            }
            state.connection = ConnectionStatus::Connecting;
        }

        let mut sessions = Vec::new();
        let mut registered = Vec::new();

        for (server_name, cfg) in &self.servers {
            if cfg.command.trim().is_empty() && cfg.url.trim().is_empty() {
                warn!(
                    target: TARGET_TOOLS,
                    "MCP server '{}': no command or url configured, skipping",
                    server_name
                );
                continue;
            }

            match MCPClientSession::connect(server_name, cfg).await {
                Ok(session) => {
                    let tool_defs = match session.list_tools().await {
                        Ok(v) => v,
                        Err(err) => {
                            error!(
                                target: TARGET_TOOLS,
                                "MCP server '{}': list_tools failed: {}",
                                server_name,
                                err
                            );
                            continue;
                        }
                    };

                    let tool_timeout = cfg.tool_timeout.max(1);
                    let mut count = 0usize;
                    for def in tool_defs {
                        let wrapper = Arc::new(MCPToolWrapper::new(
                            session.clone(),
                            server_name,
                            def,
                            tool_timeout,
                        ));
                        let name = wrapper.name().to_string();
                        if let Err(err) = registry.register_dynamic_tool(wrapper) {
                            warn!(
                                target: TARGET_TOOLS,
                                "MCP server '{}': failed to register tool '{}': {}",
                                server_name, name, err
                            );
                            continue;
                        }
                        registered.push(name);
                        count += 1;
                    }

                    info!(
                        target: TARGET_TOOLS,
                        "MCP server '{}': connected, {} tools registered",
                        server_name, count
                    );
                    sessions.push(session);
                }
                Err(err) => {
                    error!(
                        target: TARGET_TOOLS,
                        "MCP server '{}': failed to connect: {}",
                        server_name,
                        err
                    );
                }
            }
        }

        let mut state = self.state.lock().await;
        state.sessions = sessions;
        state.registered_tools = registered;
        state.connection = ConnectionStatus::Connected;
        Ok(())
    }

    pub async fn close(&self, registry: &ToolRegistry) {
        let (sessions, tool_names) = {
            let mut state = self.state.lock().await;
            state.connection = ConnectionStatus::Disconnect;
            (
                std::mem::take(&mut state.sessions),
                std::mem::take(&mut state.registered_tools),
            )
        };

        for name in tool_names {
            registry.unregister_dynamic_tool(&name);
        }
        for session in sessions {
            session.close().await;
        }
    }
}

struct MCPClientSession {
    name: String,
    client: Mutex<Option<MCPRunningClient>>,
}

impl MCPClientSession {
    async fn connect(name: &str, cfg: &MCPServerConfig) -> Result<Arc<Self>> {
        if !cfg.command.trim().is_empty() {
            Self::connect_stdio(name, cfg).await
        } else {
            Self::connect_http(name, cfg).await
        }
    }

    async fn connect_stdio(name: &str, cfg: &MCPServerConfig) -> Result<Arc<Self>> {
        let transport = TokioChildProcess::new(Command::new(&cfg.command).configure(|cmd| {
            cmd.args(&cfg.args);
            cmd.stdin(Stdio::piped());
            cmd.stdout(Stdio::piped());
            cmd.stderr(Stdio::null());
            for (k, v) in &cfg.env {
                cmd.env(k, v);
            }
        }))
        .map_err(|e| {
            NanobotError::mcp_server(name, format!("spawning MCP server '{}': {}", name, e))
        })?;

        let client: MCPRunningClient = Self::client_info().serve(transport).await.map_err(|e| {
            NanobotError::mcp_server(
                name,
                format!("initializing MCP stdio server '{}': {}", name, e),
            )
        })?;

        Ok(Arc::new(Self {
            name: name.to_string(),
            client: Mutex::new(Some(client)),
        }))
    }

    async fn connect_http(name: &str, cfg: &MCPServerConfig) -> Result<Arc<Self>> {
        if cfg.url.trim().is_empty() {
            return Err(NanobotError::mcp_server(name, "missing url"));
        }

        let custom_headers = parse_custom_headers(&cfg.headers)?;
        let transport_cfg = StreamableHttpClientTransportConfig::with_uri(cfg.url.clone())
            .custom_headers(custom_headers);
        let http_client = reqwest::Client::builder()
            .no_proxy()
            .build()
            .map_err(|e| NanobotError::mcp_server(name, format!("build MCP HTTP client: {}", e)))?;
        let transport = StreamableHttpClientTransport::with_client(http_client, transport_cfg);

        let client = Self::client_info().serve(transport).await.map_err(|e| {
            NanobotError::mcp_server(
                name,
                format!("initializing MCP HTTP server '{}': {}", name, e),
            )
        })?;

        Ok(Arc::new(Self {
            name: name.to_string(),
            client: Mutex::new(Some(client)),
        }))
    }

    fn client_info() -> ClientInfo {
        let mut info = ClientInfo::default();
        info.protocol_version = ProtocolVersion::V_2024_11_05;
        info.client_info.name = "nanobot-rs".to_string();
        info.client_info.version = env!("CARGO_PKG_VERSION").to_string();
        info
    }

    async fn peer(&self) -> Result<rmcp::Peer<RoleClient>> {
        let guard = self.client.lock().await;
        let client = guard
            .as_ref()
            .ok_or_else(|| NanobotError::mcp_server(&self.name, "server is already closed"))?;
        Ok(client.peer().clone())
    }

    async fn list_tools(&self) -> Result<Vec<MCPRemoteTool>> {
        let peer = self.peer().await?;
        let tools = peer.list_all_tools().await.map_err(|e| {
            NanobotError::mcp_server(&self.name, format!("list tools failed: {}", e))
        })?;
        Ok(tools)
    }

    async fn call_tool(
        &self,
        name: &str,
        arguments: serde_json::Map<String, serde_json::Value>,
    ) -> Result<String> {
        let peer = self.peer().await?;
        let result = peer
            .call_tool(CallToolRequestParams::new(name.to_string()).with_arguments(arguments))
            .await
            .map_err(|e| {
                NanobotError::mcp_server(&self.name, format!("call tool '{}' failed: {}", name, e))
            })?;
        Ok(format_call_tool_result(result))
    }

    async fn close(&self) {
        let client = {
            let mut guard = self.client.lock().await;
            guard.take()
        };
        if let Some(client) = client
            && let Err(err) = client.cancel().await
        {
            warn!(
                target: TARGET_TOOLS,
                "MCP server '{}': close failed: {}",
                self.name,
                err
            );
        }
    }
}

fn parse_custom_headers(
    input: &HashMap<String, String>,
) -> Result<HashMap<HeaderName, HeaderValue>> {
    let mut out = HashMap::new();
    for (k, v) in input {
        let name = HeaderName::from_bytes(k.as_bytes())
            .map_err(|e| NanobotError::config(format!("invalid MCP header name '{}': {}", k, e)))?;
        let value = HeaderValue::from_str(v).map_err(|e| {
            NanobotError::config(format!("invalid MCP header value for '{}': {}", k, e))
        })?;
        out.insert(name, value);
    }
    Ok(out)
}

fn format_call_tool_result(result: CallToolResult) -> String {
    let mut lines = Vec::new();
    for block in result.content {
        match &block.raw {
            RawContent::Text(text) => lines.push(text.text.clone()),
            _ => lines.push(
                serde_json::to_string(&block)
                    .unwrap_or_else(|_| "(unsupported MCP content block)".to_string()),
            ),
        }
    }

    if lines.is_empty()
        && let Some(structured) = result.structured_content
    {
        lines.push(structured.to_string());
    }

    if lines.is_empty() {
        "(no output)".to_string()
    } else {
        lines.join("\n")
    }
}

fn to_tool_schema(input_schema: Option<serde_json::Value>) -> JsonSchema {
    if let Some(v) = input_schema
        && let Ok(parsed) = serde_json::from_value::<JsonSchema>(v)
    {
        return parsed;
    }
    JsonSchema::object(BTreeMap::new(), Vec::new())
}

pub struct MCPToolWrapper {
    session: Arc<MCPClientSession>,
    original_name: String,
    name: String,
    description: String,
    parameters: JsonSchema,
    tool_timeout: u64,
}

impl MCPToolWrapper {
    fn new(
        session: Arc<MCPClientSession>,
        server_name: &str,
        tool_def: MCPRemoteTool,
        tool_timeout: u64,
    ) -> Self {
        let full_name = mcp_tool_name(server_name, tool_def.name.as_ref());
        Self {
            session,
            original_name: tool_def.name.to_string(),
            name: full_name,
            description: tool_def
                .description
                .map(|d| d.into_owned())
                .unwrap_or_else(|| tool_def.name.to_string()),
            parameters: to_tool_schema(Some(serde_json::Value::Object(
                tool_def.input_schema.as_ref().clone(),
            ))),
            tool_timeout,
        }
    }
}

fn mcp_tool_name(server_name: &str, tool_name: &str) -> String {
    format!("mcp_{}_{}", server_name, tool_name)
}

#[async_trait]
impl Tool for MCPToolWrapper {
    fn name(&self) -> &str {
        &self.name
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::function(&self.name, &self.description, self.parameters.clone())
    }

    async fn execute(&self, args_json: &str, _ctx: &ToolContext) -> Result<String> {
        let args_value: serde_json::Value = serde_json::from_str(args_json).map_err(|e| {
            NanobotError::invalid_tool_args(
                &self.name,
                format!("invalid MCP tool arguments: {}", e),
            )
        })?;
        let args_obj = match args_value {
            serde_json::Value::Object(map) => map,
            serde_json::Value::Null => serde_json::Map::new(),
            _ => {
                return Err(NanobotError::invalid_tool_args(
                    &self.name,
                    "MCP tool arguments must be a JSON object",
                ));
            }
        };

        match tokio::time::timeout(
            std::time::Duration::from_secs(self.tool_timeout),
            self.session.call_tool(&self.original_name, args_obj),
        )
        .await
        {
            Ok(res) => res,
            Err(_) => Ok(format!(
                "(MCP tool call timed out after {}s)",
                self.tool_timeout
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};
    use std::net::SocketAddr;
    use std::path::PathBuf;
    use std::sync::Arc;

    use super::*;
    use crate::config::schema::{ExecToolConfig, WebToolsConfig};
    use crate::tools::base::ToolContext;
    use crate::tools::registry::ToolRegistry;
    use crate::types::SessionKey;

    fn definition_names(defs: Vec<ToolDefinition>) -> HashSet<String> {
        defs.into_iter().map(|d| d.function.name).collect()
    }

    fn temp_path(prefix: &str) -> PathBuf {
        std::env::temp_dir().join(format!("{}-{}", prefix, uuid::Uuid::new_v4()))
    }

    fn find_python() -> Option<String> {
        which::which("python3")
            .or_else(|_| which::which("python"))
            .ok()
            .map(|p| p.to_string_lossy().to_string())
    }

    fn write_mock_stdio_server(path: &std::path::Path) {
        let code = r#"
import json
import sys
import time


def read_msg():
    line = sys.stdin.buffer.readline()
    if not line:
        return None
    return json.loads(line.decode("utf-8"))


def send_msg(msg):
    data = (json.dumps(msg) + "\n").encode("utf-8")
    sys.stdout.buffer.write(data)
    sys.stdout.buffer.flush()


while True:
    msg = read_msg()
    if msg is None:
        break

    method = msg.get("method")
    req_id = msg.get("id")

    if method == "initialize":
        send_msg({
            "jsonrpc": "2.0",
            "id": req_id,
            "result": {
                "protocolVersion": "2024-11-05",
                "capabilities": {"tools": {"listChanged": False}},
                "serverInfo": {"name": "mock-stdio", "version": "0.1.0"}
            }
        })
    elif method == "notifications/initialized":
        continue
    elif method == "tools/list":
        send_msg({
            "jsonrpc": "2.0",
            "id": req_id,
            "result": {
                "tools": [
                    {
                        "name": "echo",
                        "description": "Echo text",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "text": {"type": "string"}
                            },
                            "required": ["text"]
                        }
                    },
                    {
                        "name": "sleepy",
                        "description": "Sleep and return",
                        "inputSchema": {"type": "object", "properties": {}}
                    }
                ]
            }
        })
    elif method == "tools/call":
        params = msg.get("params", {})
        tool_name = params.get("name")
        args = params.get("arguments") or {}

        if tool_name == "echo":
            text = args.get("text", "")
            send_msg({
                "jsonrpc": "2.0",
                "id": req_id,
                "result": {
                    "content": [
                        {"type": "text", "text": f"echo:{text}"}
                    ]
                }
            })
        elif tool_name == "sleepy":
            time.sleep(2.0)
            send_msg({
                "jsonrpc": "2.0",
                "id": req_id,
                "result": {
                    "content": [
                        {"type": "text", "text": "done"}
                    ]
                }
            })
        else:
            send_msg({
                "jsonrpc": "2.0",
                "id": req_id,
                "result": {
                    "content": [
                        {"type": "text", "text": "unknown"}
                    ],
                    "isError": True
                }
            })
"#;
        std::fs::write(path, code).expect("write mock stdio server");
    }

    async fn read_http_request(
        reader: &mut tokio::io::BufReader<tokio::net::TcpStream>,
    ) -> Result<Option<(HashMap<String, String>, serde_json::Value)>> {
        use tokio::io::{AsyncBufReadExt, AsyncReadExt};

        let mut request_line = String::new();
        let n = reader.read_line(&mut request_line).await.map_err(|e| {
            NanobotError::tool_execution("mcp_test", anyhow::anyhow!("read request line: {}", e))
        })?;
        if n == 0 {
            return Ok(None);
        }

        let mut headers = HashMap::new();
        loop {
            let mut line = String::new();
            let n = reader.read_line(&mut line).await.map_err(|e| {
                NanobotError::tool_execution("mcp_test", anyhow::anyhow!("read header line: {}", e))
            })?;
            if n == 0 {
                return Ok(None);
            }
            let trimmed = line.trim_end_matches(['\r', '\n']);
            if trimmed.is_empty() {
                break;
            }
            if let Some((k, v)) = trimmed.split_once(':') {
                headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
            }
        }

        let len = headers
            .get("content-length")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(0);
        let mut body = vec![0u8; len];
        if len > 0 {
            reader.read_exact(&mut body).await.map_err(|e| {
                NanobotError::tool_execution(
                    "mcp_test",
                    anyhow::anyhow!("read request body: {}", e),
                )
            })?;
        }

        let value = if len == 0 {
            serde_json::Value::Null
        } else {
            serde_json::from_slice::<serde_json::Value>(&body).map_err(|e| {
                NanobotError::tool_execution(
                    "mcp_test",
                    anyhow::anyhow!("decode request body: {}", e),
                )
            })?
        };

        Ok(Some((headers, value)))
    }

    async fn write_http_response(
        stream: &mut tokio::net::TcpStream,
        status: &str,
        body: &[u8],
    ) -> Result<()> {
        use tokio::io::AsyncWriteExt;

        let mut response = format!(
            "HTTP/1.1 {}\r\nContent-Length: {}\r\nConnection: close\r\n",
            status,
            body.len()
        )
        .into_bytes();
        if !body.is_empty() {
            response.extend_from_slice(b"Content-Type: application/json\r\n");
        }
        response.extend_from_slice(b"\r\n");
        response.extend_from_slice(body);

        stream.write_all(&response).await.map_err(|e| {
            NanobotError::tool_execution("mcp_test", anyhow::anyhow!("write response: {}", e))
        })?;
        stream.flush().await.map_err(|e| {
            NanobotError::tool_execution("mcp_test", anyhow::anyhow!("flush response: {}", e))
        })?;
        Ok(())
    }

    async fn handle_mock_http_connection(
        socket: tokio::net::TcpStream,
        header_log: Arc<Mutex<Vec<HashMap<String, String>>>>,
    ) -> Result<()> {
        let mut reader = tokio::io::BufReader::new(socket);
        let Some((headers, body)) = read_http_request(&mut reader).await? else {
            return Ok(());
        };
        header_log.lock().await.push(headers);

        let stream = reader.get_mut();
        let method = body.get("method").and_then(|v| v.as_str()).unwrap_or("");
        let id = body.get("id").cloned();

        if id.is_none() || method == "notifications/initialized" {
            write_http_response(stream, "202 Accepted", b"").await?;
            return Ok(());
        }

        let id = id.expect("id exists");
        let result = match method {
            "initialize" => serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {"tools": {"listChanged": false}},
                "serverInfo": {"name": "mock-http", "version": "0.1.0"}
            }),
            "tools/list" => serde_json::json!({
                "tools": [
                    {
                        "name": "echo",
                        "description": "Echo text",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "text": {"type": "string"}
                            },
                            "required": ["text"]
                        }
                    }
                ]
            }),
            "tools/call" => {
                let text = body
                    .pointer("/params/arguments/text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                serde_json::json!({
                    "content": [
                        {"type": "text", "text": format!("echo:{text}")}
                    ]
                })
            }
            _ => serde_json::json!({"content": [{"type": "text", "text": "unknown"}]}),
        };

        let payload = serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result
        }))
        .map_err(|e| {
            NanobotError::tool_execution("mcp_test", anyhow::anyhow!("encode response: {}", e))
        })?;

        write_http_response(stream, "200 OK", &payload).await
    }

    async fn start_mock_http_server() -> Result<(
        SocketAddr,
        tokio::sync::oneshot::Sender<()>,
        tokio::task::JoinHandle<()>,
        Arc<Mutex<Vec<HashMap<String, String>>>>,
    )> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|e| {
                NanobotError::tool_execution("mcp_test", anyhow::anyhow!("bind mock server: {}", e))
            })?;
        let addr = listener.local_addr().map_err(|e| {
            NanobotError::tool_execution(
                "mcp_test",
                anyhow::anyhow!("read mock server addr: {}", e),
            )
        })?;
        let headers = Arc::new(Mutex::new(Vec::new()));
        let headers_for_task = headers.clone();
        let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel::<()>();

        let handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => {
                        break;
                    }
                    accepted = listener.accept() => {
                        let Ok((socket, _)) = accepted else {
                            break;
                        };
                        let log = headers_for_task.clone();
                        tokio::spawn(async move {
                            let _ = handle_mock_http_connection(socket, log).await;
                        });
                    }
                }
            }
        });

        Ok((addr, shutdown_tx, handle, headers))
    }

    #[test]
    fn to_tool_schema_falls_back_to_object_schema() {
        let schema = to_tool_schema(Some(serde_json::json!({
            "unexpected": true
        })));
        assert!(matches!(
            schema.schema_type,
            crate::tools::base::JsonSchemaType::Object
        ));
    }

    #[test]
    fn tool_name_is_prefixed_with_server() {
        assert_eq!(mcp_tool_name("alpha", "search"), "mcp_alpha_search");
    }

    #[tokio::test]
    async fn manager_registers_executes_and_closes_stdio_tools() {
        let Some(python) = find_python() else {
            return;
        };

        let root = temp_path("nanobot-mcp-stdio");
        std::fs::create_dir_all(&root).expect("create temp root");
        let script = root.join("mock_stdio_mcp.py");
        write_mock_stdio_server(&script);

        let mut servers = HashMap::new();
        servers.insert(
            "alpha".to_string(),
            MCPServerConfig {
                command: python,
                args: vec![script.to_string_lossy().to_string()],
                tool_timeout: 1,
                ..Default::default()
            },
        );

        let manager = MCPManager::new(servers);
        let workspace = root.join("workspace");
        std::fs::create_dir_all(&workspace).expect("create workspace");
        let registry = ToolRegistry::new(
            workspace,
            false,
            ExecToolConfig::default(),
            WebToolsConfig::default(),
            None,
            None,
            None,
        );

        manager
            .connect_if_needed(&registry)
            .await
            .expect("connect stdio MCP");

        let names = definition_names(registry.definitions());
        assert!(names.contains("mcp_alpha_echo"));
        assert!(names.contains("mcp_alpha_sleepy"));

        let ctx = ToolContext {
            channel: "test".to_string(),
            chat_id: "test".to_string(),
            session_key: SessionKey::from("test:test"),
            message_id: None,
        };

        let out = registry
            .execute("mcp_alpha_echo", r#"{"text":"hi"}"#, &ctx)
            .await
            .expect("execute echo tool");
        assert_eq!(out, "echo:hi");

        let timeout = registry
            .execute("mcp_alpha_sleepy", "{}", &ctx)
            .await
            .expect("execute sleepy tool");
        assert!(timeout.contains("timed out after 1s"));

        manager.close(&registry).await;

        let names_after_close = definition_names(registry.definitions());
        assert!(!names_after_close.contains("mcp_alpha_echo"));
        assert!(
            registry
                .execute("mcp_alpha_echo", "{}", &ctx)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn manager_registers_and_executes_http_tools() {
        let (addr, shutdown_tx, handle, header_log) = start_mock_http_server()
            .await
            .expect("start mock http server");

        let mut cfg = MCPServerConfig {
            url: format!("http://{addr}/mcp"),
            tool_timeout: 2,
            ..Default::default()
        };
        cfg.headers
            .insert("x-test-header".to_string(), "abc123".to_string());

        let mut servers = HashMap::new();
        servers.insert("http".to_string(), cfg);

        let manager = MCPManager::new(servers);
        let workspace = temp_path("nanobot-mcp-http-workspace");
        std::fs::create_dir_all(&workspace).expect("create workspace");
        let registry = ToolRegistry::new(
            workspace,
            false,
            ExecToolConfig::default(),
            WebToolsConfig::default(),
            None,
            None,
            None,
        );

        manager
            .connect_if_needed(&registry)
            .await
            .expect("connect http MCP");

        let names = definition_names(registry.definitions());
        assert!(names.contains("mcp_http_echo"));

        let ctx = ToolContext {
            channel: "test".to_string(),
            chat_id: "test".to_string(),
            session_key: SessionKey::from("test:test"),
            message_id: None,
        };

        let out = registry
            .execute("mcp_http_echo", r#"{"text":"world"}"#, &ctx)
            .await
            .expect("execute http echo tool");
        assert_eq!(out, "echo:world");

        let seen_header = header_log.lock().await.iter().any(|h| {
            h.get("x-test-header")
                .map(|v| v == "abc123")
                .unwrap_or(false)
        });
        assert!(seen_header);

        manager.close(&registry).await;

        let _ = shutdown_tx.send(());
        let _ = handle.await;

        let names_after_close = definition_names(registry.definitions());
        assert!(!names_after_close.contains("mcp_http_echo"));
    }
}
