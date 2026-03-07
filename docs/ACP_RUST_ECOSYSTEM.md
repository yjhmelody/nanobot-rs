# ACP 集成 - Rust 生态库选型

**文档版本**: 1.0  
**创建日期**: 2026-03-07  
**目标**: 优先使用 Rust 生态中已有的库，避免重复造轮子

---

## 1. 核心依赖选型

### 1.1 已有依赖（可复用）

从 `Cargo.toml` 看，项目已有：

```toml
[dependencies]
# 异步运行时
tokio = { version = "1.44", features = ["full"] }
async-trait = "0.1"

# 序列化
serde = { version = "1.0", features = ["derive"] }
serde_json = { version = "1.0", features = ["raw_value"] }

# 其他
chrono = { version = "0.4", features = ["serde"] }
uuid = { version = "1.11", features = ["v4", "serde"] }
```

**可以直接使用**：
- ✅ `tokio` - 异步运行时、进程管理、IO
- ✅ `serde` + `serde_json` - JSON 序列化
- ✅ `async-trait` - 异步 trait
- ✅ `uuid` - 会话 ID 生成

### 1.2 需要新增的依赖

#### 1.2.1 JSON-RPC 库

**选项 1: jsonrpc-core** (推荐)
```toml
jsonrpc-core = "18.0"
```
- ✅ 成熟稳定，广泛使用
- ✅ 支持 JSON-RPC 2.0
- ✅ 类型安全
- ❌ 较重，功能多

**选项 2: 手动实现**
- ✅ 轻量
- ✅ 完全控制
- ❌ 需要自己处理协议细节

**推荐**: 先用 `jsonrpc-core`，如果太重再考虑手动实现

#### 1.2.2 进程管理

**已有**: `tokio::process`
```rust
use tokio::process::{Command, Child, ChildStdin, ChildStdout};
```
- ✅ 异步进程管理
- ✅ stdio 重定向
- ✅ 与 tokio 生态集成

**无需额外依赖**

#### 1.2.3 流式处理

**已有**: `tokio_stream` (tokio 的一部分)
```rust
use tokio_stream::{Stream, StreamExt};
```
- ✅ 异步流
- ✅ 组合器（map, filter, etc）
- ✅ 与 tokio 集成

**或者使用**: `futures` (可能已有)
```toml
futures = "0.3"
```

#### 1.2.4 并发集合

**选项 1: dashmap** (推荐)
```toml
dashmap = "6.0"
```
- ✅ 并发安全的 HashMap
- ✅ 性能优秀
- ✅ API 简单

**选项 2: tokio::sync::RwLock + HashMap**
- ✅ 无额外依赖
- ❌ 性能较差
- ❌ API 复杂

**推荐**: `dashmap`

#### 1.2.5 错误处理

**已有**: `anyhow` 或 `thiserror`
```rust
use anyhow::{Result, Context};
```
- ✅ 简化错误处理
- ✅ 上下文信息

**无需额外依赖**

---

## 2. 改进后的架构

### 2.1 使用 jsonrpc-core

**协议定义**：
```rust
// src/acp/protocol.rs
use jsonrpc_core::{Params, Value};
use serde::{Deserialize, Serialize};

/// ACP 请求
#[derive(Debug, Serialize, Deserialize)]
pub struct ACPRequest {
    pub jsonrpc: String,  // "2.0"
    pub method: String,   // "execute", "cancel", "close"
    pub params: Params,
    pub id: u64,
}

/// ACP 响应
#[derive(Debug, Serialize, Deserialize)]
pub struct ACPResponse {
    pub jsonrpc: String,
    pub result: Option<Value>,
    pub error: Option<ACPError>,
    pub id: u64,
}

/// ACP 通知（事件）
#[derive(Debug, Serialize, Deserialize)]
pub struct ACPNotification {
    pub jsonrpc: String,
    pub method: String,  // "event"
    pub params: ACPEvent,
}

/// ACP 事件
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum ACPEvent {
    #[serde(rename = "thinking")]
    Thinking { text: String },
    
    #[serde(rename = "tool_call")]
    ToolCall { name: String, args: String },
    
    #[serde(rename = "tool_result")]
    ToolResult { name: String, result: String },
    
    #[serde(rename = "output")]
    Output { text: String },
    
    #[serde(rename = "error")]
    Error { message: String },
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ACPError {
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}
```

**优势**：
- ✅ 标准的 JSON-RPC 2.0 格式
- ✅ 类型安全
- ✅ 易于扩展

### 2.2 使用 tokio::process

**进程管理**：
```rust
// src/acp/client.rs
use tokio::process::{Command, Child, ChildStdin, ChildStdout};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

pub struct ACPClient {
    agent_id: String,
    process: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    session_id: String,
    next_id: u64,
}

impl ACPClient {
    pub async fn spawn(config: ACPConfig) -> Result<Self> {
        let mut cmd = Command::new(&config.command);
        
        // 设置工作目录
        if let Some(cwd) = &config.cwd {
            cmd.current_dir(cwd);
        }
        
        // 设置环境变量
        for (key, value) in &config.env {
            cmd.env(key, value);
        }
        
        // stdio 重定向
        cmd.stdin(Stdio::piped())
           .stdout(Stdio::piped())
           .stderr(Stdio::piped());
        
        let mut process = cmd.spawn()
            .context("Failed to spawn agent process")?;
        
        let stdin = process.stdin.take()
            .ok_or_else(|| anyhow!("Failed to get stdin"))?;
        let stdout = BufReader::new(
            process.stdout.take()
                .ok_or_else(|| anyhow!("Failed to get stdout"))?
        );
        
        Ok(Self {
            agent_id: config.agent_id,
            process,
            stdin,
            stdout,
            session_id: Uuid::new_v4().to_string(),
            next_id: 1,
        })
    }
    
    /// 发送 JSON-RPC 请求
    async fn send_request(&mut self, method: &str, params: Params) -> Result<u64> {
        let id = self.next_id;
        self.next_id += 1;
        
        let request = ACPRequest {
            jsonrpc: "2.0".to_string(),
            method: method.to_string(),
            params,
            id,
        };
        
        let json = serde_json::to_string(&request)?;
        self.stdin.write_all(json.as_bytes()).await?;
        self.stdin.write_all(b"\n").await?;
        self.stdin.flush().await?;
        
        Ok(id)
    }
    
    /// 接收 JSON-RPC 响应或通知
    async fn receive_message(&mut self) -> Result<ACPMessage> {
        let mut line = String::new();
        self.stdout.read_line(&mut line).await?;
        
        if line.is_empty() {
            return Err(anyhow!("EOF"));
        }
        
        // 尝试解析为响应
        if let Ok(response) = serde_json::from_str::<ACPResponse>(&line) {
            return Ok(ACPMessage::Response(response));
        }
        
        // 尝试解析为通知
        if let Ok(notification) = serde_json::from_str::<ACPNotification>(&line) {
            return Ok(ACPMessage::Notification(notification));
        }
        
        Err(anyhow!("Invalid JSON-RPC message: {}", line))
    }
}

pub enum ACPMessage {
    Response(ACPResponse),
    Notification(ACPNotification),
}
```

**优势**：
- ✅ 使用 tokio 的异步 IO
- ✅ 自动管理进程生命周期
- ✅ 类型安全

### 2.3 使用 tokio_stream

**流式处理**：
```rust
// src/acp/client.rs
use tokio_stream::{Stream, StreamExt};
use futures::stream;

impl ACPClient {
    /// 执行任务，返回事件流
    pub async fn execute(&mut self, task: &str) -> Result<impl Stream<Item = ACPEvent>> {
        // 发送 execute 请求
        let params = Params::Map(
            vec![("task".to_string(), Value::String(task.to_string()))]
                .into_iter()
                .collect()
        );
        let request_id = self.send_request("execute", params).await?;
        
        // 创建事件流
        let (tx, rx) = tokio::sync::mpsc::channel(100);
        
        // 启动接收任务
        let mut client = self.clone(); // 需要实现 Clone 或使用 Arc
        tokio::spawn(async move {
            loop {
                match client.receive_message().await {
                    Ok(ACPMessage::Notification(notif)) => {
                        if notif.method == "event" {
                            if let Ok(event) = serde_json::from_value(notif.params) {
                                if tx.send(event).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                    Ok(ACPMessage::Response(resp)) => {
                        if resp.id == request_id {
                            // 请求完成
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
        
        Ok(tokio_stream::wrappers::ReceiverStream::new(rx))
    }
}
```

**优势**：
- ✅ 异步流式处理
- ✅ 背压控制
- ✅ 可组合

### 2.4 使用 dashmap

**会话管理**：
```rust
// src/acp/session.rs
use dashmap::DashMap;
use std::sync::Arc;

pub struct ACPSessionManager {
    sessions: Arc<DashMap<String, ACPSession>>,
    config: ACPConfig,
    max_concurrent: usize,
}

pub struct ACPSession {
    pub id: String,
    pub agent_id: String,
    pub client: Arc<Mutex<ACPClient>>,
    pub created_at: DateTime<Utc>,
    pub last_active: Arc<Mutex<DateTime<Utc>>>,
}

impl ACPSessionManager {
    pub fn new(config: ACPConfig) -> Self {
        Self {
            sessions: Arc::new(DashMap::new()),
            config,
            max_concurrent: 8,
        }
    }
    
    pub async fn create_session(
        &self,
        agent_id: &str,
        cwd: Option<PathBuf>,
    ) -> Result<String> {
        // 检查并发限制
        if self.sessions.len() >= self.max_concurrent {
            return Err(anyhow!("Max concurrent sessions reached"));
        }
        
        let agent_config = ACPConfig {
            agent_id: agent_id.to_string(),
            command: self.get_agent_command(agent_id),
            cwd,
            env: self.get_agent_env(agent_id),
        };
        
        let client = ACPClient::spawn(agent_config).await?;
        let session_id = client.session_id.clone();
        
        let session = ACPSession {
            id: session_id.clone(),
            agent_id: agent_id.to_string(),
            client: Arc::new(Mutex::new(client)),
            created_at: Utc::now(),
            last_active: Arc::new(Mutex::new(Utc::now())),
        };
        
        self.sessions.insert(session_id.clone(), session);
        
        Ok(session_id)
    }
    
    pub async fn execute(
        &self,
        session_id: &str,
        task: &str,
    ) -> Result<impl Stream<Item = ACPEvent>> {
        let session = self.sessions
            .get(session_id)
            .ok_or_else(|| anyhow!("Session not found"))?;
        
        // 更新最后活跃时间
        *session.last_active.lock().await = Utc::now();
        
        // 执行任务
        let mut client = session.client.lock().await;
        client.execute(task).await
    }
    
    pub async fn close_session(&self, session_id: &str) -> Result<()> {
        if let Some((_, session)) = self.sessions.remove(session_id) {
            let mut client = session.client.lock().await;
            client.close().await?;
        }
        Ok(())
    }
    
    /// 清理过期会话
    pub async fn cleanup_expired(&self, ttl: Duration) {
        let now = Utc::now();
        let expired: Vec<String> = self.sessions
            .iter()
            .filter(|entry| {
                let last_active = entry.value().last_active.blocking_lock();
                now.signed_duration_since(*last_active).to_std().unwrap() > ttl
            })
            .map(|entry| entry.key().clone())
            .collect();
        
        for session_id in expired {
            let _ = self.close_session(&session_id).await;
        }
    }
}
```

**优势**：
- ✅ 并发安全
- ✅ 无锁竞争
- ✅ 性能优秀

---

## 3. 完整依赖清单

### 3.1 新增依赖

```toml
[dependencies]
# 已有（无需新增）
tokio = { version = "1.44", features = ["full"] }
serde = { version = "1.0", features = ["derive"] }
serde_json = { version = "1.0", features = ["raw_value"] }
async-trait = "0.1"
uuid = { version = "1.11", features = ["v4", "serde"] }
chrono = { version = "0.4", features = ["serde"] }
anyhow = "1.0"

# 新增
dashmap = "6.0"              # 并发安全的 HashMap
tokio-stream = "0.1"         # 异步流（可能已有）

# 可选（如果需要 JSON-RPC 库）
jsonrpc-core = "18.0"        # JSON-RPC 2.0 实现
```

### 3.2 依赖说明

| 依赖 | 用途 | 是否必需 |
|------|------|---------|
| tokio | 异步运行时、进程管理 | ✅ 已有 |
| serde/serde_json | JSON 序列化 | ✅ 已有 |
| async-trait | 异步 trait | ✅ 已有 |
| uuid | 会话 ID | ✅ 已有 |
| chrono | 时间处理 | ✅ 已有 |
| anyhow | 错误处理 | ✅ 已有 |
| dashmap | 并发集合 | ⭐ 新增 |
| tokio-stream | 异步流 | ⭐ 新增 |
| jsonrpc-core | JSON-RPC | ⚠️ 可选 |

**最小新增**：只需要 `dashmap` 和 `tokio-stream`

---

## 4. 实施建议

### 4.1 Phase 1: 最小实现（1 周）

**目标**: 验证可行性

**依赖**:
```toml
dashmap = "6.0"
tokio-stream = "0.1"
```

**实现**:
- 手动实现简化的 JSON-RPC 协议
- 使用 tokio::process 管理进程
- 使用 tokio-stream 处理事件流
- 使用 dashmap 管理会话

**优势**:
- ✅ 依赖最少
- ✅ 完全控制
- ✅ 快速验证

### 4.2 Phase 2: 优化（可选）

**如果手动实现太复杂**，再考虑：
```toml
jsonrpc-core = "18.0"
```

**优势**:
- ✅ 标准的 JSON-RPC 2.0
- ✅ 类型安全
- ✅ 减少代码量

**劣势**:
- ❌ 增加依赖
- ❌ 可能过重

---

## 5. 代码示例（使用生态库）

### 5.1 简化的协议实现

```rust
// src/acp/protocol.rs
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// JSON-RPC 2.0 请求
#[derive(Debug, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub method: String,
    pub params: Value,
    pub id: u64,
}

/// JSON-RPC 2.0 响应
#[derive(Debug, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
    pub id: u64,
}

/// JSON-RPC 2.0 通知
#[derive(Debug, Serialize, Deserialize)]
pub struct JsonRpcNotification {
    pub jsonrpc: String,
    pub method: String,
    pub params: Value,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// ACP 事件（通知的 params）
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum ACPEvent {
    #[serde(rename = "thinking")]
    Thinking { text: String },
    
    #[serde(rename = "tool_call")]
    ToolCall { name: String, args: String },
    
    #[serde(rename = "output")]
    Output { text: String },
    
    #[serde(rename = "error")]
    Error { message: String },
}
```

**优势**:
- ✅ 只用 serde/serde_json（已有）
- ✅ 简单直接
- ✅ 足够用

### 5.2 完整的 Client 实现

```rust
// src/acp/client.rs
use tokio::process::{Command, Child};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio_stream::{Stream, StreamExt};
use anyhow::{Result, Context};

pub struct ACPClient {
    process: Child,
    stdin: tokio::process::ChildStdin,
    stdout: BufReader<tokio::process::ChildStdout>,
    next_id: u64,
}

impl ACPClient {
    pub async fn spawn(command: &str, args: &[String]) -> Result<Self> {
        let mut cmd = Command::new(command);
        cmd.args(args)
           .stdin(Stdio::piped())
           .stdout(Stdio::piped())
           .stderr(Stdio::piped());
        
        let mut process = cmd.spawn()?;
        let stdin = process.stdin.take().unwrap();
        let stdout = BufReader::new(process.stdout.take().unwrap());
        
        Ok(Self {
            process,
            stdin,
            stdout,
            next_id: 1,
        })
    }
    
    pub async fn execute(&mut self, task: &str) -> Result<impl Stream<Item = ACPEvent>> {
        // 发送请求
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "execute".to_string(),
            params: serde_json::json!({ "task": task }),
            id: self.next_id,
        };
        self.next_id += 1;
        
        let json = serde_json::to_string(&request)?;
        self.stdin.write_all(json.as_bytes()).await?;
        self.stdin.write_all(b"\n").await?;
        self.stdin.flush().await?;
        
        // 创建事件流
        let (tx, rx) = tokio::sync::mpsc::channel(100);
        
        // 后台任务接收事件
        tokio::spawn(async move {
            loop {
                let mut line = String::new();
                if self.stdout.read_line(&mut line).await.is_err() {
                    break;
                }
                
                // 解析通知
                if let Ok(notif) = serde_json::from_str::<JsonRpcNotification>(&line) {
                    if notif.method == "event" {
                        if let Ok(event) = serde_json::from_value(notif.params) {
                            if tx.send(event).await.is_err() {
                                break;
                            }
                        }
                    }
                }
            }
        });
        
        Ok(tokio_stream::wrappers::ReceiverStream::new(rx))
    }
}
```

---

## 6. 总结

### 6.1 推荐方案

**最小依赖方案**（推荐）:
```toml
# 新增
dashmap = "6.0"
tokio-stream = "0.1"

# 已有（复用）
tokio = { version = "1.44", features = ["full"] }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
anyhow = "1.0"
```

**优势**:
- ✅ 只新增 2 个依赖
- ✅ 充分利用已有依赖
- ✅ 简单直接
- ✅ 完全控制

### 6.2 可选优化

如果手动实现 JSON-RPC 太复杂，可以考虑：
```toml
jsonrpc-core = "18.0"
```

但建议先尝试手动实现，因为：
- ACP 协议相对简单
- 我们只需要用到一小部分功能
- 手动实现更轻量

---

**下一步**: 使用推荐的依赖开始实现
