# ACP 官方 Rust SDK 集成方案

**文档版本**: 1.0  
**创建日期**: 2026-03-07  
**重要更新**: 发现 Zed 有官方 Rust SDK！

---

## 🎉 重大发现

Zed 官方提供了 **Rust SDK**！

- **Crate**: https://crates.io/crates/agent-client-protocol
- **GitHub**: https://github.com/agentclientprotocol/rust-sdk
- **示例**: 
  - [examples/agent.rs](https://github.com/agentclientprotocol/rust-sdk/blob/main/examples/agent.rs)
  - [examples/client.rs](https://github.com/agentclientprotocol/rust-sdk/blob/main/examples/client.rs)

---

## 1. 官方 SDK 概览

### 1.1 可用的官方 SDK

| 语言 | 包名 | 仓库 |
|------|------|------|
| **Rust** | `agent-client-protocol` | [rust-sdk](https://github.com/agentclientprotocol/rust-sdk) ⭐ |
| TypeScript | `@agentclientprotocol/sdk` | [typescript-sdk](https://github.com/agentclientprotocol/typescript-sdk) |
| Python | `agent-client-protocol` | [python-sdk](https://github.com/agentclientprotocol/python-sdk) |
| Kotlin | `agent-client-protocol` | [kotlin-sdk](https://github.com/agentclientprotocol/kotlin-sdk) |
| Java | `agent-client-protocol` | [java-sdk](https://github.com/agentclientprotocol/java-sdk) |

### 1.2 为什么用官方 SDK

**优势**：
- ✅ **官方维护** - Zed Industries 官方支持
- ✅ **完整实现** - 完整的 ACP 协议支持
- ✅ **类型安全** - Rust 类型系统
- ✅ **持续更新** - 跟随协议演进
- ✅ **示例丰富** - 官方示例代码
- ✅ **社区支持** - 生态系统支持

**对比手动实现**：
- ❌ 手动实现：需要自己处理协议细节、版本兼容
- ✅ 官方 SDK：开箱即用、协议兼容性有保证

---

## 2. 集成方案（使用官方 SDK）

### 2.1 依赖

```toml
[dependencies]
# ACP 官方 SDK
agent-client-protocol = "0.1"  # 使用最新版本

# 已有依赖（复用）
tokio = { version = "1.44", features = ["full"] }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
anyhow = "1.0"

# 会话管理
dashmap = "6.0"
```

**总结**：
- ✅ 使用官方 SDK：`agent-client-protocol`
- ✅ 会话管理：`dashmap`
- ✅ 其他：复用已有依赖

### 2.2 架构设计

```
┌─────────────────────────────────────────┐
│  Application Layer (应用层)             │
│  - Agent 调用 acp_execute 工具          │
└─────────────────────────────────────────┘
              ↓
┌─────────────────────────────────────────┐
│  Tool Layer (工具层)                    │
│  - ACPTool: 封装 ACP 调用               │
└─────────────────────────────────────────┘
              ↓
┌─────────────────────────────────────────┐
│  Session Layer (会话层)                 │
│  - ACPSessionManager: 管理多个会话      │
└─────────────────────────────────────────┘
              ↓
┌─────────────────────────────────────────┐
│  ACP SDK Layer (SDK 层)                 │
│  - agent_client_protocol crate          │
│  - Client/Agent 实现                    │
└─────────────────────────────────────────┘
```

### 2.3 核心实现

#### 2.3.1 使用官方 SDK

```rust
// src/acp/client.rs
use agent_client_protocol::{Client, ClientBuilder, Event, Request, Response};
use tokio::process::Command;

pub struct ACPClient {
    client: Client,
    session_id: String,
}

impl ACPClient {
    /// 启动 Agent 并创建 ACP Client
    pub async fn spawn(config: ACPConfig) -> Result<Self> {
        // 启动 agent 进程
        let mut cmd = Command::new(&config.command);
        
        if let Some(cwd) = &config.cwd {
            cmd.current_dir(cwd);
        }
        
        for (key, value) in &config.env {
            cmd.env(key, value);
        }
        
        cmd.stdin(Stdio::piped())
           .stdout(Stdio::piped())
           .stderr(Stdio::piped());
        
        let process = cmd.spawn()?;
        
        // 使用官方 SDK 创建 Client
        let client = ClientBuilder::new()
            .stdin(process.stdin.take().unwrap())
            .stdout(process.stdout.take().unwrap())
            .build()
            .await?;
        
        Ok(Self {
            client,
            session_id: Uuid::new_v4().to_string(),
        })
    }
    
    /// 执行任务
    pub async fn execute(&mut self, task: &str) -> Result<impl Stream<Item = Event>> {
        // 发送 execute 请求
        let request = Request::Execute {
            task: task.to_string(),
        };
        
        // 获取事件流
        let events = self.client.send_request(request).await?;
        
        Ok(events)
    }
    
    /// 关闭会话
    pub async fn close(mut self) -> Result<()> {
        self.client.close().await?;
        Ok(())
    }
}
```

#### 2.3.2 会话管理

```rust
// src/acp/session.rs
use dashmap::DashMap;
use std::sync::Arc;

pub struct ACPSessionManager {
    sessions: Arc<DashMap<String, ACPSession>>,
    config: ACPConfig,
}

pub struct ACPSession {
    pub id: String,
    pub agent_id: String,
    pub client: Arc<Mutex<ACPClient>>,
    pub created_at: DateTime<Utc>,
    pub last_active: Arc<Mutex<DateTime<Utc>>>,
}

impl ACPSessionManager {
    pub async fn create_session(
        &self,
        agent_id: &str,
        cwd: Option<PathBuf>,
    ) -> Result<String> {
        let config = ACPConfig {
            agent_id: agent_id.to_string(),
            command: self.get_agent_command(agent_id),
            cwd,
            env: self.get_agent_env(agent_id),
        };
        
        let client = ACPClient::spawn(config).await?;
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
    ) -> Result<impl Stream<Item = Event>> {
        let session = self.sessions
            .get(session_id)
            .ok_or_else(|| anyhow!("Session not found"))?;
        
        *session.last_active.lock().await = Utc::now();
        
        let mut client = session.client.lock().await;
        client.execute(task).await
    }
}
```

#### 2.3.3 工具集成

```rust
// src/tools/acp.rs
use agent_client_protocol::Event;

pub struct ACPTool {
    session_manager: Arc<ACPSessionManager>,
}

#[async_trait]
impl Tool for ACPTool {
    fn name(&self) -> &str {
        "acp_execute"
    }
    
    fn description(&self) -> &str {
        "Execute a task using an ACP agent (codex, claude, pi, gemini, opencode)"
    }
    
    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "agent_id": {
                    "type": "string",
                    "enum": ["codex", "claude", "pi", "gemini", "opencode"],
                    "description": "The ACP agent to use"
                },
                "task": {
                    "type": "string",
                    "description": "The task to execute"
                },
                "cwd": {
                    "type": "string",
                    "description": "Working directory (optional)"
                }
            },
            "required": ["agent_id", "task"]
        })
    }
    
    async fn execute(&self, args: &str, context: &ToolContext) -> Result<String> {
        let req: ACPExecuteRequest = serde_json::from_str(args)?;
        
        // 创建会话
        let session_id = self.session_manager
            .create_session(&req.agent_id, req.cwd)
            .await?;
        
        // 执行任务
        let mut events = self.session_manager
            .execute(&session_id, &req.task)
            .await?;
        
        let mut output = String::new();
        
        // 处理事件流
        while let Some(event) = events.next().await {
            match event {
                Event::Thinking { text } => {
                    info!(target: TARGET_AGENT, "💭 {}", text);
                }
                Event::ToolCall { name, args } => {
                    info!(target: TARGET_AGENT, "🔧 {}({})", name, args);
                }
                Event::Output { text } => {
                    output.push_str(&text);
                }
                Event::Error { message } => {
                    return Err(anyhow!("ACP error: {}", message));
                }
                _ => {}
            }
        }
        
        // 关闭会话
        self.session_manager.close_session(&session_id).await?;
        
        Ok(output)
    }
}
```

---

## 3. 实施计划（简化版）

### 3.1 Phase 1: 基础集成（1 周）

**目标**: 使用官方 SDK 实现基本功能

**任务**:
1. ✅ 添加依赖：`agent-client-protocol`
2. ✅ 实现 ACPClient（基于官方 SDK）
3. ✅ 支持单个 agent (codex)
4. ✅ 编写测试

**交付物**:
- `src/acp/client.rs`
- 基本测试

### 3.2 Phase 2: 会话管理（1 周）

**目标**: 实现会话管理

**任务**:
1. ✅ 实现 ACPSessionManager
2. ✅ 支持多会话并发
3. ✅ TTL 管理

**交付物**:
- `src/acp/session.rs`

### 3.3 Phase 3: 工具集成（1 周）

**目标**: 集成到 nanobot-rs

**任务**:
1. ✅ 实现 ACPTool
2. ✅ 注册到 ToolRegistry
3. ✅ 配置系统

**交付物**:
- `src/tools/acp.rs`
- 配置示例

### 3.4 Phase 4: 多 Agent 支持（1 周）

**目标**: 支持所有 agent

**任务**:
1. ✅ 支持 claude, pi, gemini, opencode
2. ✅ Agent 配置管理

**交付物**:
- 多 Agent 配置

**总计**: 4 周（比之前的 7 周减少了 3 周）

---

## 4. 优势对比

### 4.1 使用官方 SDK vs 手动实现

| 方面 | 官方 SDK | 手动实现 |
|------|---------|---------|
| 开发时间 | 4 周 | 7 周 |
| 协议兼容性 | ✅ 官方保证 | ⚠️ 需要自己维护 |
| 版本更新 | ✅ 自动跟随 | ❌ 需要手动更新 |
| 错误处理 | ✅ 完善 | ⚠️ 需要自己实现 |
| 文档 | ✅ 官方文档 | ❌ 需要自己写 |
| 社区支持 | ✅ 生态系统 | ❌ 孤立 |
| 依赖数量 | 1 个 | 2 个 |
| 代码量 | 少 | 多 |

**结论**: 使用官方 SDK 明显更好

### 4.2 新旧方案对比

| 方面 | 旧方案（手动实现） | 新方案（官方 SDK） |
|------|------------------|------------------|
| 核心依赖 | dashmap + tokio-stream | agent-client-protocol + dashmap |
| 协议实现 | 手动实现 JSON-RPC | 官方 SDK |
| 开发时间 | 7 周 | 4 周 |
| 维护成本 | 高 | 低 |
| 可靠性 | ⚠️ 需要测试 | ✅ 官方保证 |

---

## 5. 配置

```toml
# config.toml

[acp]
enabled = true
default_agent = "codex"
allowed_agents = ["codex", "claude", "pi", "gemini", "opencode"]
max_concurrent_sessions = 8
session_ttl_minutes = 120

[acp.agents.codex]
command = "codex"
env = { OPENAI_API_KEY = "${OPENAI_API_KEY}" }

[acp.agents.claude]
command = "claude"
env = { ANTHROPIC_API_KEY = "${ANTHROPIC_API_KEY}" }

[acp.agents.pi]
command = "pi"

[acp.agents.gemini]
command = "gemini"
env = { GOOGLE_API_KEY = "${GOOGLE_API_KEY}" }

[acp.agents.opencode]
command = "opencode"
```

---

## 6. 使用示例

### 6.1 从 Agent 调用

```rust
// 用户: "用 Codex 构建一个 Web 服务器"
// Agent 自动调用 acp_execute 工具
acp_execute({
    "agent_id": "codex",
    "task": "Build a web server with Rust and Axum"
})
```

### 6.2 直接调用工具

```rust
let result = tools.execute("acp_execute", r#"{
    "agent_id": "codex",
    "task": "Refactor the authentication module",
    "cwd": "/path/to/project"
}"#, &context).await?;
```

---

## 7. 总结

### 7.1 重大改进

**发现官方 Rust SDK**：
- ✅ 不需要手动实现协议
- ✅ 开发时间从 7 周减少到 4 周
- ✅ 协议兼容性有官方保证
- ✅ 持续更新和社区支持

### 7.2 最终依赖

```toml
# 新增（仅 2 个）
agent-client-protocol = "0.1"  # ACP 官方 SDK
dashmap = "6.0"                # 会话管理

# 已有（复用）
tokio = { version = "1.44", features = ["full"] }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
anyhow = "1.0"
```

### 7.3 下一步

1. 查看官方 SDK 文档和示例
2. 添加依赖：`agent-client-protocol`
3. 开始实施 Phase 1
4. 参考官方示例代码

---

**状态**: ✅ 方案更新完成
**推荐**: 使用官方 Rust SDK
**开发时间**: 4 周（减少 3 周）
