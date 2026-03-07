# ACP (Agent Client Protocol) 集成设计

**文档版本**: 1.0  
**创建日期**: 2026-03-07  
**目标**: 集成 Zed 的 ACP 协议，让 nanobot-rs 可以调用支持 ACP 的 agent 工具

---

## 目录

1. [ACP 协议概述](#1-acp-协议概述)
2. [OpenClaw 的 ACP 实现分析](#2-openclaw-的-acp-实现分析)
3. [nanobot-rs 集成方案](#3-nanobot-rs-集成方案)
4. [实施计划](#4-实施计划)

---

## 1. ACP 协议概述

### 1.1 什么是 ACP

**Agent Client Protocol (ACP)** 是 Zed Industries 开发的标准化协议，用于：
- **Code Editor (Client)** 和 **Coding Agent** 之间的通信
- 让编辑器可以调用外部 AI coding agents（如 Claude Code, Codex, Pi, Gemini CLI 等）

**官方资源**：
- 协议文档: https://agentclientprotocol.com
- TypeScript SDK: https://github.com/agentclientprotocol/typescript-sdk
- Rust 实现: 需要自己实现或使用 FFI

### 1.2 核心概念

```
┌─────────────────────────────────────────┐
│         Client (Code Editor)            │
│  - Zed                                  │
│  - OpenClaw (通过 acpx 插件)            │
│  - nanobot-rs (我们要实现)              │
└─────────────────────────────────────────┘
              ↕ ACP Protocol
┌─────────────────────────────────────────┐
│         Agent (Coding Harness)          │
│  - Claude Code (claude)                 │
│  - Codex (codex)                        │
│  - Pi Coding Agent (pi)                 │
│  - OpenCode (opencode)                  │
│  - Gemini CLI (gemini)                  │
└─────────────────────────────────────────┘
```

**角色**：
- **Client**: 发起请求，管理会话，展示结果
- **Agent**: 执行任务，调用工具，返回结果

### 1.3 协议特性

1. **标准化通信**
   - JSON-RPC 2.0 over stdio
   - 请求/响应模式
   - 事件流式传输

2. **会话管理**
   - 持久化会话
   - 会话恢复
   - 多会话并发

3. **工具调用**
   - 文件读写
   - 命令执行
   - 网络请求

4. **权限控制**
   - 审批策略
   - 沙箱模式
   - 工具白名单/黑名单

---

## 2. OpenClaw 的 ACP 实现分析

### 2.1 架构

OpenClaw 通过 **acpx 插件** 实现 ACP 支持：

```
OpenClaw Gateway
  ↓
acpx Plugin (@openclaw/acpx)
  ↓
acpx CLI (Node.js)
  ↓
ACP Protocol (stdio)
  ↓
Agent (claude/codex/pi/gemini/opencode)
```

### 2.2 核心组件

#### 2.2.1 acpx CLI

**功能**：
- ACP 协议适配器
- 管理 Agent 进程
- 处理 stdio 通信

**使用**：
```bash
# 启动 Agent
acpx --agent codex

# 指定工作目录
acpx --agent codex --cwd /path/to/project

# 自定义 Agent 命令
acpx --agent "custom-agent --flag"
```

#### 2.2.2 OpenClaw ACP Runtime

**配置**：
```json5
{
  "acp": {
    "enabled": true,
    "dispatch": { "enabled": true },
    "backend": "acpx",
    "defaultAgent": "codex",
    "allowedAgents": ["pi", "claude", "codex", "opencode", "gemini"],
    "maxConcurrentSessions": 8,
    "stream": {
      "coalesceIdleMs": 300,
      "maxChunkChars": 1200
    },
    "runtime": {
      "ttlMinutes": 120
    }
  }
}
```

**功能**：
- 会话管理
- 线程绑定（Discord 等）
- 流式输出
- 权限控制

#### 2.2.3 sessions_spawn 工具

**调用方式**：
```rust
// 从 Agent 调用
sessions_spawn({
    "task": "Build a web server",
    "runtime": "acp",
    "agentId": "codex",
    "thread": true,
    "mode": "session"
})
```

**参数**：
- `runtime`: "acp" (必须)
- `agentId`: "codex" | "claude" | "pi" | "gemini" | "opencode"
- `thread`: 是否绑定线程
- `mode`: "run" (一次性) | "session" (持久化)
- `cwd`: 工作目录
- `label`: 会话标签

#### 2.2.4 /acp 命令

**可用命令**：
```bash
/acp spawn codex --mode persistent --thread auto
/acp status
/acp model anthropic/claude-opus-4-5
/acp permissions strict
/acp timeout 120
/acp steer continue with better error handling
/acp cancel
/acp close
```

### 2.3 工作流程

#### 2.3.1 启动 ACP 会话

```
1. User: "用 Codex 构建一个 Web 服务器"
   ↓
2. OpenClaw Agent 识别需要 ACP
   ↓
3. 调用 sessions_spawn(runtime="acp", agentId="codex")
   ↓
4. acpx 插件启动 codex 进程
   ↓
5. 建立 ACP 连接 (stdio)
   ↓
6. 发送初始任务
   ↓
7. 流式接收输出
   ↓
8. 返回结果给用户
```

#### 2.3.2 持久化会话

```
1. 创建会话: /acp spawn codex --mode persistent --thread auto
   ↓
2. 绑定到线程（如果支持）
   ↓
3. 后续消息自动路由到该会话
   ↓
4. 会话保持活跃（TTL: 120 分钟）
   ↓
5. 关闭: /acp close
```

---

## 3. nanobot-rs 集成方案

### 3.1 设计目标

1. **兼容 ACP 协议** - 可以调用任何支持 ACP 的 Agent
2. **简单易用** - 用户无需了解 ACP 细节
3. **灵活配置** - 支持多种 Agent 和配置
4. **安全可控** - 权限管理和沙箱

### 3.2 架构设计

#### 方案 A: 直接实现 ACP Client（推荐）

```
nanobot-rs
  ↓
ACP Client (Rust 实现)
  ↓
ACP Protocol (stdio)
  ↓
Agent (claude/codex/pi/gemini)
```

**优势**：
- ✅ 无需依赖 Node.js
- ✅ 性能更好
- ✅ 更好的控制
- ✅ 纯 Rust 实现

**劣势**：
- ❌ 需要实现 ACP 协议
- ❌ 开发工作量大

#### 方案 B: 通过 acpx CLI（快速方案）

```
nanobot-rs
  ↓
acpx CLI (subprocess)
  ↓
ACP Protocol (stdio)
  ↓
Agent (claude/codex/pi/gemini)
```

**优势**：
- ✅ 快速实现
- ✅ 复用 OpenClaw 的 acpx
- ✅ 无需实现协议

**劣势**：
- ❌ 依赖 Node.js
- ❌ 多一层抽象
- ❌ 性能开销

### 3.3 推荐方案：方案 A（直接实现）

#### 3.3.1 核心模块

```rust
// src/acp/mod.rs
pub mod client;      // ACP Client 实现
pub mod protocol;    // 协议定义
pub mod session;     // 会话管理
pub mod agent;       // Agent 管理
pub mod transport;   // 传输层（stdio）
```

#### 3.3.2 ACP Client 实现

```rust
// src/acp/client.rs

use tokio::process::{Child, Command};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// ACP Client
pub struct ACPClient {
    agent_id: String,
    process: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    session_id: String,
}

impl ACPClient {
    /// 启动 Agent
    pub async fn spawn(config: ACPConfig) -> Result<Self> {
        let mut cmd = Command::new(&config.command);
        
        // 设置参数
        if let Some(cwd) = &config.cwd {
            cmd.current_dir(cwd);
        }
        
        // 设置环境变量
        for (key, value) in &config.env {
            cmd.env(key, value);
        }
        
        // stdio 模式
        cmd.stdin(Stdio::piped())
           .stdout(Stdio::piped())
           .stderr(Stdio::piped());
        
        let mut process = cmd.spawn()?;
        
        let stdin = process.stdin.take().unwrap();
        let stdout = BufReader::new(process.stdout.take().unwrap());
        
        Ok(Self {
            agent_id: config.agent_id,
            process,
            stdin,
            stdout,
            session_id: Uuid::new_v4().to_string(),
        })
    }
    
    /// 发送请求
    pub async fn send_request(&mut self, request: ACPRequest) -> Result<()> {
        let json = serde_json::to_string(&request)?;
        self.stdin.write_all(json.as_bytes()).await?;
        self.stdin.write_all(b"\n").await?;
        self.stdin.flush().await?;
        Ok(())
    }
    
    /// 接收响应（流式）
    pub async fn receive_response(&mut self) -> Result<ACPResponse> {
        let mut line = String::new();
        self.stdout.read_line(&mut line).await?;
        let response: ACPResponse = serde_json::from_str(&line)?;
        Ok(response)
    }
    
    /// 执行任务
    pub async fn execute(
        &mut self,
        task: &str,
    ) -> impl Stream<Item = ACPEvent> {
        // 发送初始请求
        self.send_request(ACPRequest::Execute {
            task: task.to_string(),
        }).await?;
        
        // 流式接收事件
        stream! {
            loop {
                match self.receive_response().await {
                    Ok(response) => {
                        match response {
                            ACPResponse::Event(event) => yield event,
                            ACPResponse::Complete => break,
                            ACPResponse::Error(err) => {
                                yield ACPEvent::Error(err);
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        yield ACPEvent::Error(e.to_string());
                        break;
                    }
                }
            }
        }
    }
    
    /// 关闭会话
    pub async fn close(mut self) -> Result<()> {
        self.send_request(ACPRequest::Close).await?;
        self.process.kill().await?;
        Ok(())
    }
}
```

#### 3.3.3 协议定义

```rust
// src/acp/protocol.rs

use serde::{Deserialize, Serialize};

/// ACP 请求
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "method", content = "params")]
pub enum ACPRequest {
    #[serde(rename = "execute")]
    Execute { task: String },
    
    #[serde(rename = "cancel")]
    Cancel,
    
    #[serde(rename = "close")]
    Close,
}

/// ACP 响应
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ACPResponse {
    #[serde(rename = "event")]
    Event(ACPEvent),
    
    #[serde(rename = "complete")]
    Complete,
    
    #[serde(rename = "error")]
    Error(String),
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
    
    #[serde(rename = "approval_needed")]
    ApprovalNeeded {
        action: String,
        risk_level: String,
    },
    
    #[serde(rename = "error")]
    Error(String),
}
```

#### 3.3.4 会话管理

```rust
// src/acp/session.rs

pub struct ACPSessionManager {
    sessions: Arc<DashMap<String, ACPSession>>,
    config: ACPConfig,
}

pub struct ACPSession {
    pub id: String,
    pub agent_id: String,
    pub client: ACPClient,
    pub created_at: DateTime<Utc>,
    pub last_active: DateTime<Utc>,
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
            client,
            created_at: Utc::now(),
            last_active: Utc::now(),
        };
        
        self.sessions.insert(session_id.clone(), session);
        
        Ok(session_id)
    }
    
    pub async fn execute(
        &self,
        session_id: &str,
        task: &str,
    ) -> Result<impl Stream<Item = ACPEvent>> {
        let mut session = self.sessions
            .get_mut(session_id)
            .ok_or_else(|| anyhow!("Session not found"))?;
        
        session.last_active = Utc::now();
        
        session.client.execute(task).await
    }
    
    pub async fn close_session(&self, session_id: &str) -> Result<()> {
        if let Some((_, session)) = self.sessions.remove(session_id) {
            session.client.close().await?;
        }
        Ok(())
    }
    
    fn get_agent_command(&self, agent_id: &str) -> String {
        match agent_id {
            "codex" => "codex".to_string(),
            "claude" => "claude".to_string(),
            "pi" => "pi".to_string(),
            "gemini" => "gemini".to_string(),
            "opencode" => "opencode".to_string(),
            _ => agent_id.to_string(),
        }
    }
    
    fn get_agent_env(&self, agent_id: &str) -> HashMap<String, String> {
        let mut env = HashMap::new();
        
        // 从配置读取 API keys
        if let Some(key) = self.config.get_api_key(agent_id) {
            match agent_id {
                "claude" => {
                    env.insert("ANTHROPIC_API_KEY".to_string(), key);
                }
                "codex" => {
                    env.insert("OPENAI_API_KEY".to_string(), key);
                }
                "gemini" => {
                    env.insert("GOOGLE_API_KEY".to_string(), key);
                }
                _ => {}
            }
        }
        
        env
    }
}
```

#### 3.3.5 工具集成

```rust
// src/tools/acp.rs

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
                },
                "session_id": {
                    "type": "string",
                    "description": "Existing session ID (optional, for persistent sessions)"
                }
            },
            "required": ["agent_id", "task"]
        })
    }
    
    async fn execute(
        &self,
        args: &str,
        context: &ToolContext,
    ) -> Result<String> {
        let req: ACPExecuteRequest = serde_json::from_str(args)?;
        
        // 获取或创建会话
        let session_id = if let Some(id) = req.session_id {
            id
        } else {
            self.session_manager
                .create_session(&req.agent_id, req.cwd)
                .await?
        };
        
        // 执行任务
        let mut events = self.session_manager
            .execute(&session_id, &req.task)
            .await?;
        
        let mut output = String::new();
        
        while let Some(event) = events.next().await {
            match event {
                ACPEvent::Thinking { text } => {
                    info!(target: TARGET_AGENT, "💭 {}", text);
                }
                ACPEvent::ToolCall { name, args } => {
                    info!(target: TARGET_AGENT, "🔧 {}({})", name, args);
                }
                ACPEvent::Output { text } => {
                    output.push_str(&text);
                }
                ACPEvent::Error(err) => {
                    return Err(anyhow!("ACP error: {}", err));
                }
                _ => {}
            }
        }
        
        Ok(output)
    }
}

#[derive(Deserialize)]
struct ACPExecuteRequest {
    agent_id: String,
    task: String,
    cwd: Option<PathBuf>,
    session_id: Option<String>,
}
```

### 3.4 配置

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

### 3.5 使用示例

#### 3.5.1 从 Agent 调用

```rust
// Agent 自动识别需要 ACP
let response = agent.execute("用 Codex 构建一个 Web 服务器").await?;

// 内部调用 acp_execute 工具
acp_execute({
    "agent_id": "codex",
    "task": "Build a web server with Rust and Axum"
})
```

#### 3.5.2 直接调用工具

```rust
let result = tools.execute("acp_execute", r#"{
    "agent_id": "codex",
    "task": "Refactor the authentication module",
    "cwd": "/path/to/project"
}"#, &context).await?;
```

#### 3.5.3 持久化会话

```rust
// 创建会话
let session_id = acp_manager.create_session("codex", Some("/project")).await?;

// 执行多个任务
acp_manager.execute(&session_id, "Task 1").await?;
acp_manager.execute(&session_id, "Task 2").await?;

// 关闭会话
acp_manager.close_session(&session_id).await?;
```

---

## 4. 实施计划

### 4.1 Phase 1: 基础实现（2 周）

**目标**: 实现基本的 ACP Client

**任务**:
1. ✅ 实现 ACP 协议定义
2. ✅ 实现 stdio 传输层
3. ✅ 实现基本的 Client
4. ✅ 支持单个 Agent (codex)
5. ✅ 编写测试

**交付物**:
- `src/acp/protocol.rs`
- `src/acp/transport.rs`
- `src/acp/client.rs`
- 基本测试

### 4.2 Phase 2: 会话管理（1 周）

**目标**: 实现会话管理

**任务**:
1. ✅ 实现 SessionManager
2. ✅ 支持多会话并发
3. ✅ 会话 TTL 管理
4. ✅ 会话持久化（可选）

**交付物**:
- `src/acp/session.rs`
- 会话管理测试

### 4.3 Phase 3: 工具集成（1 周）

**目标**: 集成到工具系统

**任务**:
1. ✅ 实现 ACPTool
2. ✅ 注册到 ToolRegistry
3. ✅ 配置系统
4. ✅ 文档

**交付物**:
- `src/tools/acp.rs`
- 配置示例
- 使用文档

### 4.4 Phase 4: 多 Agent 支持（1 周）

**目标**: 支持多种 Agent

**任务**:
1. ✅ 支持 claude, pi, gemini, opencode
2. ✅ Agent 配置管理
3. ✅ 环境变量管理
4. ✅ 测试所有 Agent

**交付物**:
- 多 Agent 配置
- 集成测试

### 4.5 Phase 5: 高级特性（2 周）

**目标**: 实现高级特性

**任务**:
1. ✅ 流式输出
2. ✅ 权限控制
3. ✅ 错误处理
4. ✅ 性能优化

**交付物**:
- 完整功能
- 性能测试

---

## 5. 参考资源

### 5.1 官方资源

- **ACP 协议**: https://agentclientprotocol.com
- **TypeScript SDK**: https://github.com/agentclientprotocol/typescript-sdk
- **Gemini CLI 实现**: https://github.com/google-gemini/gemini-cli

### 5.2 Agent 工具

- **Codex**: https://github.com/anthropics/codex
- **Claude Code**: https://claude.ai/code
- **Pi Coding Agent**: https://github.com/mariozechner/pi-coding-agent
- **Gemini CLI**: https://github.com/google-gemini/gemini-cli
- **OpenCode**: https://github.com/opencode/opencode

### 5.3 OpenClaw 实现

- **acpx 插件**: `/opt/homebrew/lib/node_modules/openclaw/extensions/acpx`
- **ACP 文档**: `/opt/homebrew/lib/node_modules/openclaw/docs/tools/acp-agents.md`

---

## 6. 总结

### 6.1 核心价值

1. **标准化** - 使用 ACP 协议，兼容所有支持 ACP 的 Agent
2. **灵活性** - 支持多种 Agent，可配置
3. **易用性** - 简单的工具接口，用户无需了解协议细节
4. **可扩展** - 易于添加新的 Agent

### 6.2 实施优先级

**P0 (必须)**:
- 基础 ACP Client 实现
- 支持 codex
- 工具集成

**P1 (重要)**:
- 会话管理
- 多 Agent 支持
- 配置系统

**P2 (有用)**:
- 流式输出
- 权限控制
- 性能优化

### 6.3 预期收益

- ✅ 可以调用所有支持 ACP 的 coding agents
- ✅ 无需为每个 agent 单独实现
- ✅ 标准化的接口和行为
- ✅ 更好的生态兼容性

---

**下一步**: 开始实施 Phase 1 - 基础 ACP Client 实现
