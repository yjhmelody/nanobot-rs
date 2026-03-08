# ACP Phase 3 实施计划 - 官方 SDK 集成

**文档版本**: 1.0  
**创建日期**: 2026-03-08  
**目标**: 使用官方 SDK 实现完整的 ACP 协议

---

## 1. 当前状态

### 1.1 已完成

- ✅ MVP 实现（占位符）
- ✅ 系统集成（动态注册）
- ✅ 官方 SDK 依赖已添加
- ✅ 5 个主流 agents 配置

### 1.2 当前限制

- ❌ 只返回占位符文本
- ❌ 不会真正调用 ACP agent
- ❌ 无会话管理
- ❌ 无流式输出
- ❌ 无错误恢复

---

## 2. Phase 3 目标

### 2.1 核心功能

**必须实现**:
1. ✅ 使用官方 SDK 重构 ACPClient
2. ✅ 实现完整的 ACP 协议通信
3. ✅ 支持流式输出
4. ✅ 基本的会话管理
5. ✅ 错误处理和恢复

**可选功能**:
6. ⏳ 会话持久化
7. ⏳ 审批请求处理
8. ⏳ 高级会话管理

---

## 3. 官方 SDK 分析

### 3.1 核心类型

```rust
// agent-client-protocol crate

// 客户端
pub struct Client {
    // stdio 通信
}

// 请求类型
pub enum Request {
    Initialize(InitializeParams),
    Execute(ExecuteParams),
    // ...
}

// 响应类型
pub enum Response {
    Initialize(InitializeResult),
    Execute(ExecuteResult),
    // ...
}

// 事件类型
pub enum Event {
    Thinking(ThinkingEvent),
    ToolCall(ToolCallEvent),
    Output(OutputEvent),
    // ...
}
```

### 3.2 通信流程

```
1. 启动进程
   ↓
2. Initialize 握手
   ↓
3. Execute 请求
   ↓
4. 接收事件流
   - Thinking
   - ToolCall
   - Output
   ↓
5. 获取最终结果
   ↓
6. 关闭会话
```

---

## 4. 实施方案

### 4.1 重构 ACPClient

```rust
// src/acp/client.rs

use agent_client_protocol::{Client, Request, Response, Event};
use tokio::process::{Command, Child};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use std::path::PathBuf;
use std::collections::HashMap;
use anyhow::{Result, Context};

pub struct ACPClient {
    agent_id: String,
    process: Child,
    client: Client,
    session_id: String,
}

impl ACPClient {
    /// 启动 ACP agent 并初始化
    pub async fn spawn(
        agent_id: String,
        command: String,
        cwd: Option<PathBuf>,
        env: HashMap<String, String>,
    ) -> Result<Self> {
        // 1. 启动进程
        let mut cmd = Command::new(&command);
        
        if let Some(cwd) = &cwd {
            cmd.current_dir(cwd);
        }
        
        for (key, value) in env.iter() {
            cmd.env(key, value);
        }
        
        cmd.stdin(Stdio::piped())
           .stdout(Stdio::piped())
           .stderr(Stdio::piped());
        
        let mut process = cmd.spawn()
            .context(format!("Failed to spawn ACP agent: {}", agent_id))?;
        
        // 2. 获取 stdio
        let stdin = process.stdin.take()
            .ok_or_else(|| anyhow!("Failed to get stdin"))?;
        let stdout = process.stdout.take()
            .ok_or_else(|| anyhow!("Failed to get stdout"))?;
        
        // 3. 创建 ACP Client
        let client = Client::new(stdin, stdout);
        
        // 4. Initialize 握手
        let init_params = InitializeParams {
            client_info: ClientInfo {
                name: "nanobot-rs".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
            capabilities: ClientCapabilities {
                // 支持的能力
            },
        };
        
        let init_result = client.initialize(init_params).await?;
        let session_id = init_result.session_id;
        
        Ok(Self {
            agent_id,
            process,
            client,
            session_id,
        })
    }
    
    /// 执行任务
    pub async fn execute(&mut self, task: &str) -> Result<String> {
        // 1. 发送 Execute 请求
        let execute_params = ExecuteParams {
            session_id: self.session_id.clone(),
            task: task.to_string(),
            context: None,
        };
        
        let request_id = self.client.execute(execute_params).await?;
        
        // 2. 接收事件流
        let mut output = String::new();
        let mut thinking = Vec::new();
        let mut tool_calls = Vec::new();
        
        loop {
            match self.client.recv_event().await? {
                Event::Thinking(event) => {
                    thinking.push(event.content);
                }
                Event::ToolCall(event) => {
                    tool_calls.push(event);
                }
                Event::Output(event) => {
                    output.push_str(&event.content);
                }
                Event::Done(event) => {
                    if event.request_id == request_id {
                        break;
                    }
                }
                Event::Error(event) => {
                    return Err(anyhow!("ACP agent error: {}", event.message));
                }
                _ => {}
            }
        }
        
        // 3. 返回结果
        Ok(output)
    }
    
    /// 执行任务（流式）
    pub async fn execute_stream(
        &mut self,
        task: &str,
    ) -> Result<impl Stream<Item = ACPEvent>> {
        let execute_params = ExecuteParams {
            session_id: self.session_id.clone(),
            task: task.to_string(),
            context: None,
        };
        
        let request_id = self.client.execute(execute_params).await?;
        
        // 返回事件流
        Ok(self.client.event_stream(request_id))
    }
    
    /// 关闭会话
    pub async fn close(mut self) -> Result<()> {
        // 1. 发送 Shutdown 请求
        self.client.shutdown().await?;
        
        // 2. 等待进程退出
        self.process.wait().await?;
        
        Ok(())
    }
}

/// ACP 事件
#[derive(Debug, Clone)]
pub enum ACPEvent {
    Thinking(String),
    ToolCall { name: String, args: String },
    Output(String),
    Done,
    Error(String),
}
```

### 4.2 更新 ACPTool

```rust
// src/tools/acp.rs

impl Tool for ACPTool {
    async fn execute(&self, args: &str, _context: &ToolContext) -> Result<String> {
        let req: ACPExecuteRequest = serde_json::from_str(args)?;
        
        // 验证 agent_id
        if !self.config.allowed_agents.contains(&req.agent_id) {
            return Err(NanobotError::invalid_tool_args(
                self.name(),
                format!("Agent '{}' is not allowed", req.agent_id)
            ));
        }
        
        // 获取 agent 配置
        let agent_config = self.config.agents.get(&req.agent_id)
            .ok_or_else(|| NanobotError::invalid_tool_args(
                self.name(),
                format!("Agent '{}' not configured", req.agent_id)
            ))?;
        
        // 创建 ACP Client
        let mut client = ACPClient::spawn(
            req.agent_id.clone(),
            agent_config.command.clone(),
            req.cwd.map(|s| s.into()),
            agent_config.env.clone(),
        ).await
        .map_err(|e| NanobotError::tool_execution(self.name(), e))?;
        
        // 执行任务
        let result = client.execute(&req.task).await
            .map_err(|e| NanobotError::tool_execution(self.name(), e))?;
        
        // 关闭 client
        client.close().await
            .map_err(|e| NanobotError::tool_execution(self.name(), e))?;
        
        Ok(result)
    }
}
```

---

## 5. 会话管理

### 5.1 ACPSessionManager

```rust
// src/acp/session.rs

use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use chrono::{DateTime, Utc};

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
    pub fn new(config: ACPConfig) -> Self {
        Self {
            sessions: Arc::new(DashMap::new()),
            config,
        }
    }
    
    /// 获取或创建会话
    pub async fn get_or_create_session(
        &self,
        agent_id: &str,
        cwd: Option<PathBuf>,
    ) -> Result<String> {
        // 查找现有会话
        for entry in self.sessions.iter() {
            if entry.value().agent_id == agent_id {
                // 更新最后活跃时间
                let mut last_active = entry.value().last_active.lock().await;
                *last_active = Utc::now();
                return Ok(entry.key().clone());
            }
        }
        
        // 创建新会话
        self.create_session(agent_id, cwd).await
    }
    
    /// 创建新会话
    async fn create_session(
        &self,
        agent_id: &str,
        cwd: Option<PathBuf>,
    ) -> Result<String> {
        let agent_config = self.config.agents.get(agent_id)
            .ok_or_else(|| anyhow!("Agent '{}' not configured", agent_id))?;
        
        let client = ACPClient::spawn(
            agent_id.to_string(),
            agent_config.command.clone(),
            cwd,
            agent_config.env.clone(),
        ).await?;
        
        let session_id = uuid::Uuid::new_v4().to_string();
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
    
    /// 执行任务
    pub async fn execute(
        &self,
        session_id: &str,
        task: &str,
    ) -> Result<String> {
        let session = self.sessions.get(session_id)
            .ok_or_else(|| anyhow!("Session '{}' not found", session_id))?;
        
        let mut client = session.client.lock().await;
        let result = client.execute(task).await?;
        
        // 更新最后活跃时间
        let mut last_active = session.last_active.lock().await;
        *last_active = Utc::now();
        
        Ok(result)
    }
    
    /// 关闭会话
    pub async fn close_session(&self, session_id: &str) -> Result<()> {
        if let Some((_, session)) = self.sessions.remove(session_id) {
            let client = Arc::try_unwrap(session.client)
                .map_err(|_| anyhow!("Session still in use"))?
                .into_inner();
            client.close().await?;
        }
        Ok(())
    }
    
    /// 清理过期会话
    pub async fn cleanup_expired(&self, ttl_minutes: i64) -> Result<()> {
        let now = Utc::now();
        let mut expired = Vec::new();
        
        for entry in self.sessions.iter() {
            let last_active = entry.value().last_active.lock().await;
            let duration = now.signed_duration_since(*last_active);
            
            if duration.num_minutes() > ttl_minutes {
                expired.push(entry.key().clone());
            }
        }
        
        for session_id in expired {
            self.close_session(&session_id).await?;
        }
        
        Ok(())
    }
}
```

---

## 6. 错误处理

### 6.1 错误类型

```rust
// src/acp/error.rs

use thiserror::Error;

#[derive(Error, Debug)]
pub enum ACPError {
    #[error("Failed to spawn agent: {0}")]
    SpawnError(String),
    
    #[error("Initialize failed: {0}")]
    InitializeError(String),
    
    #[error("Execute failed: {0}")]
    ExecuteError(String),
    
    #[error("Communication error: {0}")]
    CommunicationError(String),
    
    #[error("Timeout after {0}s")]
    Timeout(u64),
    
    #[error("Agent crashed: {0}")]
    AgentCrashed(String),
    
    #[error("Session not found: {0}")]
    SessionNotFound(String),
}
```

### 6.2 错误恢复

```rust
impl ACPClient {
    /// 执行任务（带重试）
    pub async fn execute_with_retry(
        &mut self,
        task: &str,
        max_retries: u32,
    ) -> Result<String> {
        let mut retries = 0;
        
        loop {
            match self.execute(task).await {
                Ok(result) => return Ok(result),
                Err(e) if retries < max_retries => {
                    retries += 1;
                    eprintln!("Retry {}/{}: {}", retries, max_retries, e);
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
                Err(e) => return Err(e),
            }
        }
    }
}
```

---

## 7. 测试计划

### 7.1 单元测试

```rust
#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_acp_client_spawn() {
        // 测试启动 agent
    }
    
    #[tokio::test]
    async fn test_acp_client_execute() {
        // 测试执行任务
    }
    
    #[tokio::test]
    async fn test_session_manager() {
        // 测试会话管理
    }
}
```

### 7.2 集成测试

```bash
# 测试 1: 基本执行
nanobot-rs agent -m "用 Claude 创建一个 hello world"

# 测试 2: 流式输出
nanobot-rs agent -m "用 Codex 生成一个复杂算法，显示思考过程"

# 测试 3: 会话复用
nanobot-rs agent -m "用 Cursor 修改代码"
nanobot-rs agent -m "继续修改"  # 复用会话

# 测试 4: 错误恢复
# 模拟 agent 崩溃，验证重试机制
```

---

## 8. 实施步骤

### Step 1: 分析官方 SDK（1 小时）

```bash
# 查看 SDK 文档
cargo doc --open -p agent-client-protocol

# 查看示例
find ~/.cargo/registry -name "agent-client-protocol*" -type d
```

### Step 2: 重构 ACPClient（4 小时）

```bash
# 1. 实现基本通信
# 2. 实现 Initialize 握手
# 3. 实现 Execute 请求
# 4. 实现事件处理
```

### Step 3: 实现会话管理（2 小时）

```bash
# 1. 创建 ACPSessionManager
# 2. 实现会话创建和复用
# 3. 实现会话清理
```

### Step 4: 错误处理（1 小时）

```bash
# 1. 定义错误类型
# 2. 实现错误恢复
# 3. 添加超时控制
```

### Step 5: 测试（2 小时）

```bash
cargo test --lib acp
cargo test --test integration_acp
```

### Step 6: 文档（1 小时）

```bash
# 更新 README.md
# 添加使用示例
# 更新配置文档
```

**总计**: 约 11 小时（1.5 天）

---

## 9. 配置更新

### 9.1 会话管理配置

```toml
# config.toml

[acp]
enabled = true
defaultAgent = "claude"
allowedAgents = ["codex", "claude", "cursor", "windsurf", "cline"]

[acp.session]
enabled = true           # 启用会话管理
ttlMinutes = 30         # 会话 TTL（分钟）
maxSessions = 10        # 最大会话数
cleanupIntervalMinutes = 5  # 清理间隔

[acp.execution]
timeoutSeconds = 300    # 执行超时（秒）
maxRetries = 3          # 最大重试次数
retryDelaySeconds = 1   # 重试延迟
```

---

## 10. 预期效果

### 10.1 功能对比

| 功能 | MVP | Phase 3 |
|------|-----|---------|
| 真实执行 | ❌ | ✅ |
| 流式输出 | ❌ | ✅ |
| 会话管理 | ❌ | ✅ |
| 错误恢复 | ❌ | ✅ |
| 超时控制 | ❌ | ✅ |

### 10.2 性能指标

| 指标 | 目标 |
|------|------|
| 启动时间 | < 2s |
| 响应延迟 | < 100ms |
| 会话复用 | 节省 50% 启动时间 |
| 错误恢复 | 90% 成功率 |

---

## 11. 总结

### 11.1 Phase 3 交付物

**代码**:
- 重构后的 ACPClient（约 300 行）
- ACPSessionManager（约 200 行）
- 错误处理（约 100 行）
- **总计**: 约 600 行

**测试**:
- 单元测试（10+ 个）
- 集成测试（5+ 个）

**文档**:
- 实施计划
- API 文档
- 使用示例

### 11.2 下一步

**Phase 4** (可选):
- 审批请求处理
- 会话持久化
- 高级会话管理
- 性能优化

---

**状态**: ✅ 计划完成  
**预计时间**: 1.5 天（11 小时）  
**难度**: 中高  
**优先级**: 高
