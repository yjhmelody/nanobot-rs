//! MCP (Model Context Protocol) server manager and dynamic tool wrapper.
//!
//! This module provides the integration layer between the nanobot tool
//! system and external MCP servers. It manages the lifecycle of MCP
//! server connections (both stdio and HTTP/Streamable HTTP transports)
//! and wraps remote tools as local [`Tool`] implementations.
//!
//! ## Architecture
//!
//! ```text
//! MCPManager (lifecycle manager)
//!   └── MCPClientSession (per-server connection)
//!         └── MCPToolWrapper (per-tool adapter implementing Tool trait)
//!               └── ToolRegistry (dynamic registration)
//! ```
//!
//! ## Transport types
//!
//! - **STDIO**: Spawns a subprocess and communicates via stdin/stdout
//!   using the JSON-RPC MCP protocol.
//! - **HTTP/Streamable HTTP**: Connects to a remote MCP server via HTTP,
//!   supporting optional custom headers and proxy bypass.
//!
//! ## Tool naming
//!
//! MCP tools are registered with a prefixed name: `mcp_{server}_{tool}`,
//! e.g., `mcp_alpha_echo`. This prevents naming collisions between
//! different MCP servers and between MCP and builtin tools.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;

use async_trait::async_trait;
use http::{HeaderName, HeaderValue};
use rmcp::model::{
    CallToolRequestParams, CallToolResult, ClientInfo, ProtocolVersion, RawContent,
    ReadResourceRequestParams, ResourceContents, Tool as MCPRemoteTool,
};
use rmcp::service::RunningService;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::{ConfigureCommandExt, StreamableHttpClientTransport, TokioChildProcess};
use rmcp::{RoleClient, ServiceExt};
use tokio::process::Command;
use tokio::sync::Mutex;
use tracing::{error, info, warn};

use crate::error::{ToolError, ToolResult};
use nanobot_config::MCPServerConfig;

use crate::base::{JsonSchema, Tool, ToolContext, ToolDefinition};
use crate::registry::ToolRegistry;

const TARGET: &str = "nanobot::tools";

/// Type alias for a running MCP client.
type MCPRunningClient = RunningService<RoleClient, ClientInfo>;

/// Manages MCP server lifecycle and dynamic tool registration.
///
/// Handles connecting to configured MCP servers (both stdio and HTTP),
/// listing their available tools, registering them with a [`ToolRegistry`],
/// and cleaning up on shutdown.
///
/// Uses `tokio::sync::Mutex` for internal state because lock operations
/// cross await points during connection setup and teardown.
pub struct MCPManager {
    /// Server configurations, keyed by server name.
    servers: HashMap<String, MCPServerConfig>,
    /// Shared mutable state: connection status, active sessions, registered tool names.
    state: Mutex<MCPManagerState>,
}

/// Internal state of the MCP manager.
#[derive(Default)]
struct MCPManagerState {
    /// Current connection lifecycle status.
    connection: ConnectionStatus,
    /// Active MCP client sessions (one per server).
    sessions: Vec<Arc<MCPClientSession>>,
    /// Names of tools currently registered in the `ToolRegistry`.
    registered_tools: Vec<String>,
}

/// Tracks the connection lifecycle of an MCP server.
///
/// Prevents concurrent connection attempts via the `Connecting` intermediate state.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy, Hash, Default)]
enum ConnectionStatus {
    /// No connection established yet.
    #[default]
    Disconnect,
    /// Connection is active.
    Connected,
    /// Connection attempt is in progress.
    Connecting,
}

impl ConnectionStatus {
    fn is_disconnect(self) -> bool {
        self == ConnectionStatus::Disconnect
    }
}

impl MCPManager {
    /// Creates a new `MCPManager` with the given server configurations.
    ///
    /// # Arguments
    ///
    /// * `servers` - Map of server names to their configurations.
    pub fn new(servers: HashMap<String, MCPServerConfig>) -> Self {
        Self {
            servers,
            state: Mutex::new(MCPManagerState::default()),
        }
    }

    /// Connects to all configured MCP servers if not already connected.
    ///
    /// For each server:
    /// 1. Connects via the configured transport (stdio or HTTP).
    /// 2. Lists all available tools.
    /// 3. Wraps each tool as an [`MCPToolWrapper`] and registers it in the
    ///    [`ToolRegistry`].
    ///
    /// This is idempotent: calling multiple times only connects once. Uses
    /// a `Connecting` intermediate state to serialize concurrent attempts.
    ///
    /// # Errors
    ///
    /// Returns an error if any server connection fails hard (individual
    /// server failures are logged but do not block other servers).
    pub async fn connect_if_needed(&self, registry: &ToolRegistry) -> ToolResult<()> {
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
                    target: TARGET,
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
                                target: TARGET,
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
                                target: TARGET,
                                "MCP server '{}': failed to register tool '{}': {}",
                                server_name, name, err
                            );
                            continue;
                        }
                        registered.push(name);
                        count += 1;
                    }

                    info!(
                        target: TARGET,
                        "MCP server '{}': connected, {} tools registered",
                        server_name, count
                    );
                    sessions.push(session);
                }
                Err(err) => {
                    error!(
                        target: TARGET,
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

    /// Disconnects all MCP servers and unregisters their tools.
    ///
    /// Unregisters all dynamic tools from the registry first, then closes
    /// each session. This ensures the agent loop will not attempt to call
    /// stale tool references.
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

    /// Reads a resource from a connected MCP server by URI.
    ///
    /// MCP resources are addressable content (documents, files, etc.)
    /// exposed by the server alongside its tools.
    ///
    /// # Arguments
    ///
    /// * `server_name` - The name of the MCP server.
    /// * `uri` - The resource URI (e.g., `file:///config.json`).
    ///
    /// # Errors
    ///
    /// Returns an error if the server is not connected or the resource
    /// cannot be read.
    pub async fn read_resource(&self, server_name: &str, uri: &str) -> ToolResult<String> {
        let session = {
            let state = self.state.lock().await;
            state
                .sessions
                .iter()
                .find(|session| session.name == server_name)
                .cloned()
        };
        let Some(session) = session else {
            return Err(ToolError::mcp_server(
                server_name,
                "server is not connected",
            ));
        };
        session.read_resource(uri).await
    }
}

/// Represents a single MCP client connection to a server.
///
/// Wraps a [`RunningService`] client from the `rmcp` crate and provides
/// methods for listing tools, calling tools, reading resources, and
/// closing the connection.
///
/// Uses `tokio::sync::Mutex` to protect the client reference since
/// operations on it cross await points.
struct MCPClientSession {
    name: String,
    client: Mutex<Option<MCPRunningClient>>,
}

impl MCPClientSession {
    /// Connects to an MCP server using either stdio or HTTP transport.
    ///
    /// Transport selection is automatic based on the config:
    /// - If `command` is set, uses stdio transport (spawns subprocess).
    /// - Otherwise, uses HTTP/Streamable HTTP transport to `url`.
    async fn connect(name: &str, cfg: &MCPServerConfig) -> ToolResult<Arc<Self>> {
        if !cfg.command.trim().is_empty() {
            Self::connect_stdio(name, cfg).await
        } else {
            Self::connect_http(name, cfg).await
        }
    }

    /// Connects to an MCP server via stdio transport.
    ///
    /// Spawns the configured command as a child process and communicates
    /// via stdin/stdout using the JSON-RPC MCP protocol.
    async fn connect_stdio(name: &str, cfg: &MCPServerConfig) -> ToolResult<Arc<Self>> {
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
            ToolError::mcp_server(name, format!("spawning MCP server '{}': {}", name, e))
        })?;

        let client: MCPRunningClient = Self::client_info().serve(transport).await.map_err(|e| {
            ToolError::mcp_server(
                name,
                format!("initializing MCP stdio server '{}': {}", name, e),
            )
        })?;

        Ok(Arc::new(Self {
            name: name.to_string(),
            client: Mutex::new(Some(client)),
        }))
    }

    /// Connects to an MCP server via HTTP/Streamable HTTP transport.
    ///
    /// Supports custom headers and bypasses the system proxy (uses
    /// `reqwest::Client::builder().no_proxy()` for direct connections).
    async fn connect_http(name: &str, cfg: &MCPServerConfig) -> ToolResult<Arc<Self>> {
        if cfg.url.trim().is_empty() {
            return Err(ToolError::mcp_server(name, "missing url"));
        }

        let custom_headers = parse_custom_headers(&cfg.headers)?;
        let transport_cfg = StreamableHttpClientTransportConfig::with_uri(cfg.url.clone())
            .custom_headers(custom_headers);
        let http_client = reqwest::Client::builder()
            .no_proxy()
            .build()
            .map_err(|e| ToolError::mcp_server(name, format!("build MCP HTTP client: {}", e)))?;
        let transport = StreamableHttpClientTransport::with_client(http_client, transport_cfg);

        let client = Self::client_info().serve(transport).await.map_err(|e| {
            ToolError::mcp_server(
                name,
                format!("initializing MCP HTTP server '{}': {}", name, e),
            )
        })?;

        Ok(Arc::new(Self {
            name: name.to_string(),
            client: Mutex::new(Some(client)),
        }))
    }

    /// Builds the client info payload for the MCP initialization handshake.
    fn client_info() -> ClientInfo {
        let mut info = ClientInfo::default();
        info.protocol_version = ProtocolVersion::V_2024_11_05;
        info.client_info.name = "nanobot".to_string();
        info.client_info.version = env!("CARGO_PKG_VERSION").to_string();
        info
    }

    /// Returns a clone of the underlying MCP peer for making requests.
    async fn peer(&self) -> ToolResult<rmcp::Peer<RoleClient>> {
        let guard = self.client.lock().await;
        let client = guard
            .as_ref()
            .ok_or_else(|| ToolError::mcp_server(&self.name, "server is already closed"))?;
        Ok(client.peer().clone())
    }

    /// Lists all tools exposed by the MCP server.
    async fn list_tools(&self) -> ToolResult<Vec<MCPRemoteTool>> {
        let peer = self.peer().await?;
        let tools = peer
            .list_all_tools()
            .await
            .map_err(|e| ToolError::mcp_server(&self.name, format!("list tools failed: {}", e)))?;
        Ok(tools)
    }

    /// Calls a tool on the MCP server.
    async fn call_tool(
        &self,
        name: &str,
        arguments: serde_json::Map<String, serde_json::Value>,
    ) -> ToolResult<String> {
        let peer = self.peer().await?;
        let result = peer
            .call_tool(CallToolRequestParams::new(name.to_string()).with_arguments(arguments))
            .await
            .map_err(|e| {
                ToolError::mcp_server(&self.name, format!("call tool '{}' failed: {}", name, e))
            })?;
        Ok(format_call_tool_result(result))
    }

    /// Reads a resource from the MCP server by URI.
    async fn read_resource(&self, uri: &str) -> ToolResult<String> {
        let peer = self.peer().await?;
        let result = peer
            .read_resource(ReadResourceRequestParams::new(uri.to_string()))
            .await
            .map_err(|e| {
                ToolError::mcp_server(&self.name, format!("read resource '{}' failed: {}", uri, e))
            })?;
        Ok(format_read_resource_result(result.contents))
    }

    /// Gracefully closes the MCP connection.
    async fn close(&self) {
        let client = {
            let mut guard = self.client.lock().await;
            guard.take()
        };
        if let Some(client) = client
            && let Err(err) = client.cancel().await
        {
            warn!(
                target: TARGET,
                "MCP server '{}': close failed: {}",
                self.name,
                err
            );
        }
    }
}

/// Parses a `HashMap<String, String>` of custom headers into validated
/// HTTP header name/value pairs.
///
/// # Errors
///
/// Returns a configuration error if any header name or value is invalid
/// per the HTTP specification.
fn parse_custom_headers(
    input: &HashMap<String, String>,
) -> ToolResult<HashMap<HeaderName, HeaderValue>> {
    let mut out = HashMap::new();
    for (k, v) in input {
        let name = HeaderName::from_bytes(k.as_bytes())
            .map_err(|e| ToolError::config(format!("invalid MCP header name '{}': {}", k, e)))?;
        let value = HeaderValue::from_str(v).map_err(|e| {
            ToolError::config(format!("invalid MCP header value for '{}': {}", k, e))
        })?;
        out.insert(name, value);
    }
    Ok(out)
}

/// Formats an MCP `CallToolResult` into a plain text string.
///
/// Extracts text content blocks, falls back to JSON serialization for
/// unsupported content types, and handles structured content and empty
/// results gracefully.
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

/// Formats MCP resource contents into a plain text string.
fn format_read_resource_result(contents: Vec<ResourceContents>) -> String {
    let mut lines = Vec::new();
    for content in contents {
        match content {
            ResourceContents::TextResourceContents { text, .. } => lines.push(text),
            other => lines.push(
                serde_json::to_string(&other)
                    .unwrap_or_else(|_| "(unsupported MCP resource block)".to_string()),
            ),
        }
    }

    if lines.is_empty() {
        "(empty resource)".to_string()
    } else {
        lines.join("\n")
    }
}

/// Converts an MCP tool's input schema (as `Option<serde_json::Value>`)
/// into a [`JsonSchema`] for the local tool definition.
///
/// Falls back to an empty object schema if the input schema is `None` or
/// cannot be parsed, ensuring the tool definition is always valid even
/// when the remote server provides minimal metadata.
fn to_tool_schema(input_schema: Option<serde_json::Value>) -> JsonSchema {
    if let Some(v) = input_schema
        && let Ok(parsed) = serde_json::from_value::<JsonSchema>(v)
    {
        return parsed;
    }
    JsonSchema::object(BTreeMap::new(), Vec::new())
}

/// Adapter that wraps a remote MCP tool as a local [`Tool`] implementation.
///
/// Each registered MCP tool gets its own `MCPToolWrapper` instance that:
/// - Translates local tool execution calls to MCP `tools/call` requests.
/// - Applies a configurable timeout to prevent hanging the agent loop.
/// - Formats the remote result for the local result channel.
pub struct MCPToolWrapper {
    session: Arc<MCPClientSession>,
    original_name: String,
    name: String,
    description: String,
    parameters: JsonSchema,
    tool_timeout: u64,
}

impl MCPToolWrapper {
    /// Creates a new `MCPToolWrapper`.
    ///
    /// The wrapper's local name is prefixed with `mcp_{server}_` to avoid
    /// collisions with other tools (e.g., `mcp_github_search_code`).
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

/// Generates a unique tool name for an MCP tool.
///
/// Format: `mcp_{server_name}_{tool_name}`
fn mcp_tool_name(server_name: &str, tool_name: &str) -> String {
    format!("mcp_{}_{}", server_name, tool_name)
}

#[async_trait]
impl Tool for MCPToolWrapper {
    fn name(&self) -> &str {
        &self.name
    }

    fn definition(&self) -> Arc<ToolDefinition> {
        Arc::new(ToolDefinition::function(
            &self.name,
            &self.description,
            self.parameters.clone(),
        ))
    }

    async fn execute(&self, args_json: &str, _ctx: &ToolContext) -> ToolResult<String> {
        let args_value: serde_json::Value = serde_json::from_str(args_json).map_err(|e| {
            ToolError::invalid_args(&self.name, format!("invalid MCP tool arguments: {}", e))
        })?;
        let args_obj = match args_value {
            serde_json::Value::Object(map) => map,
            serde_json::Value::Null => serde_json::Map::new(),
            _ => {
                return Err(ToolError::invalid_args(
                    &self.name,
                    "MCP tool arguments must be a JSON object",
                ));
            }
        };

        // Apply per-tool timeout to prevent slow MCP calls from hanging.
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
    use crate::base::ToolContext;
    use crate::registry::ToolRegistry;
    use nanobot_config::{ExecToolConfig, WebToolsConfig};
    use nanobot_types::SessionKey;

    fn definition_names(defs: Vec<Arc<ToolDefinition>>) -> HashSet<String> {
        defs.into_iter().map(|d| d.function.name.clone()).collect()
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
    ) -> ToolResult<Option<(HashMap<String, String>, serde_json::Value)>> {
        use tokio::io::{AsyncBufReadExt, AsyncReadExt};

        let mut request_line = String::new();
        let n = reader.read_line(&mut request_line).await.map_err(|e| {
            ToolError::execution("mcp_test", anyhow::anyhow!("read request line: {}", e))
        })?;
        if n == 0 {
            return Ok(None);
        }

        let mut headers = HashMap::new();
        loop {
            let mut line = String::new();
            let n = reader.read_line(&mut line).await.map_err(|e| {
                ToolError::execution("mcp_test", anyhow::anyhow!("read header line: {}", e))
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
                ToolError::execution("mcp_test", anyhow::anyhow!("read request body: {}", e))
            })?;
        }

        let value = if len == 0 {
            serde_json::Value::Null
        } else {
            serde_json::from_slice::<serde_json::Value>(&body).map_err(|e| {
                ToolError::execution("mcp_test", anyhow::anyhow!("decode request body: {}", e))
            })?
        };

        Ok(Some((headers, value)))
    }

    async fn write_http_response(
        stream: &mut tokio::net::TcpStream,
        status: &str,
        body: &[u8],
    ) -> ToolResult<()> {
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
            ToolError::execution("mcp_test", anyhow::anyhow!("write response: {}", e))
        })?;
        stream.flush().await.map_err(|e| {
            ToolError::execution("mcp_test", anyhow::anyhow!("flush response: {}", e))
        })?;
        Ok(())
    }

    async fn handle_mock_http_connection(
        socket: tokio::net::TcpStream,
        header_log: Arc<Mutex<Vec<HashMap<String, String>>>>,
    ) -> ToolResult<()> {
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
        .map_err(|e| ToolError::execution("mcp_test", anyhow::anyhow!("encode response: {}", e)))?;

        write_http_response(stream, "200 OK", &payload).await
    }

    async fn start_mock_http_server() -> ToolResult<(
        SocketAddr,
        tokio::sync::oneshot::Sender<()>,
        tokio::task::JoinHandle<()>,
        Arc<Mutex<Vec<HashMap<String, String>>>>,
    )> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|e| {
                ToolError::execution("mcp_test", anyhow::anyhow!("bind mock server: {}", e))
            })?;
        let addr = listener.local_addr().map_err(|e| {
            ToolError::execution("mcp_test", anyhow::anyhow!("read mock server addr: {}", e))
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
            nanobot_types::tools::JsonSchemaType::Object
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
