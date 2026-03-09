# ACP Phase 3 简化实施方案

**文档版本**: 1.0  
**创建日期**: 2026-03-08  
**目标**: 基于官方 SDK 实现简化版 ACP 集成

---

## 1. 问题分析

### 1.1 官方 SDK 复杂度

**发现**:
- 官方 SDK API 较复杂
- 需要实现完整的 Client trait
- 涉及权限请求、终端管理等高级功能

**我们的需求**:
- ✅ 基本的任务执行
- ✅ 获取执行结果
- ⏳ 流式输出（可选）
- ⏳ 会话管理（可选）

### 1.2 简化策略

**Phase 3A**（本次实施，1 天）:
- 使用官方 SDK 的基础功能
- 实现简单的 Client 实现
- 支持基本的任务执行

**Phase 3B**（后续，1 周）:
- 完整的 Client trait 实现
- 流式输出
- 高级功能

---

## 2. Phase 3A 实施方案

### 2.1 最小 Client 实现

```rust
// src/acp/simple_client.rs

use agent_client_protocol::{Client, SessionNotification, RequestPermissionRequest, RequestPermissionResponse};
use agent_client_protocol_schema::{Result, Error};

/// 简化的 Client 实现
pub struct SimpleClient;

#[async_trait::async_trait(?Send)]
impl Client for SimpleClient {
    /// 权限请求 - 自动批准
    async fn request_permission(
        &self,
        _args: RequestPermissionRequest,
    ) -> Result<RequestPermissionResponse> {
        // 简化：自动批准所有请求
        Ok(RequestPermissionResponse {
            outcome: RequestPermissionOutcome::Approved,
        })
    }
    
    /// 会话通知 - 记录日志
    async fn session_notification(&self, args: SessionNotification) -> Result<()> {
        // 简化：只记录日志
        log::debug!("Session notification: {:?}", args);
        Ok(())
    }
    
    // 其他方法使用默认实现（返回 method_not_found）
}
```

### 2.2 简化的 ACPClient

```rust
// src/acp/client.rs

use agent_client_protocol::{ClientSideConnection, Agent, InitializeRequest, NewSessionRequest, PromptRequest};
use tokio::process::{Command, Child};
use std::path::PathBuf;
use std::collections::HashMap;
use anyhow::{Result, Context};

pub struct ACPClient {
    agent_id: String,
    process: Child,
    connection: ClientSideConnection,
    session_id: Option<String>,
}

impl ACPClient {
    /// 启动 ACP agent
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
            .context(format!("Failed to spawn ACP agent: ", agent_id))?;
        
        // 2. 获取 stdio
        let stdin = process.stdin.take()
            .ok_or_else(|| anyhow!("Failed to get stdin"))?;
        let stdout = process.stdout.take()
            .ok_or_else(|| anyhow!("Failed to get stdout"))?;
        
        // 3. 创建连接
        let client = SimpleClient;
        let (connection, io_task) = ClientSideConnection::new(
            client,
            stdin,
            stdout,
            |fut| { tokio::spawn(fut); },
        );
        
        // 4. 启动 IO 任务
        tokio::spawn(async move {
            if let Err(e) = io_task.await {
                log::error!("ACP IO task error: {}", e);
            }
        });
        
        // 5. Initialize
        let init_response = connection.initialize(InitializeRequest {
            protocol_version: "0.1.0".to_string(),
            client_info: ClientInfo {
                name: "nanobot-rs".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
            capabilities: ClientCapabilities::default(),
        }).await?;
        
        log::info!("ACP agent initialized: {:?}", init_response);
        
        Ok(Self {
            agent_id,
            process,
            connection,
            session_id: None,
        })
    }
    
    /// 执行任务
    pub async fn execute(&mut self, task: &str) -> Result<String> {
        // 1. 创建会话（如果还没有）
        if self.session_id.is_none() {
            let session_response = self.connection.new_session(NewSessionRequest {
                // 会话参数
            }).await?;
            self.session_id = Some(session_response.session_id);
        }
        
        // 2. 发送 prompt
        let prompt_response = self.connection.prompt(PromptRequest {
            session_id: self.session_id.clone().unwrap(),
            prompt: task.to_string(),
        }).await?;
        
        // 3. 提取结果
        let output = self.extract_output(&prompt_response)?;
        
        Ok(output)
    }
    
    /// 提取输出
    fn extract_output(&self, response: &PromptResponse) -> Result<String> {
        // 从响应中提取文本输出
        let mut output = String::new();
        
        for message in &response.messages {
            if let Some(content) = &message.content {
                output.push_str(content);
                output.push('\n');
            }
        }
        
        Ok(output)
    }
    
    /// 关闭
    pub async fn close(mut self) -> Result<()> {
        // 关闭进程
        self.process.kill().await?;
        Ok(())
    }
}
```

---

## 3. 更简单的方案：直接使用 stdio

由于官方 SDK 较复杂，我们可以先实现一个更简单的版本：

### 3.1 基于 JSON-RPC 的简单实现

```rust
// src/acp/simple_rpc.rs

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Command, Child, ChildStdin, ChildStdout};
use serde::{Serialize, Deserialize};
use serde_json::json;

#[derive(Serialize, Deserialize)]
struct JsonRpcRequest {
    jsonrpc: String,
    id: u64,
    method: String,
    params: serde_json::Value,
}

#[derive(Serialize, Deserialize)]
struct JsonRpcResponse {
    jsonrpc: String,
    id: u64,
    result: Option<serde_json::Value>,
    error: Option<JsonRpcError>,
}

#[derive(Serialize, Deserialize)]
struct JsonRpcError {
    code: i32,
    message: String,
}

pub struct SimpleACPClient {
    agent_id: String,
    process: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    request_id: u64,
}

impl SimpleACPClient {
    pub async fn spawn(
        agent_id: String,
        command: String,
        cwd: Option<PathBuf>,
        env: HashMap<String, String>,
    ) -> Result<Self> {
        // 启动进程
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
        
        let mut process = cmd.spawn()?;
        
        let stdin = process.stdin.take().unwrap();
        let stdout = BufReader::new(process.stdout.take().unwrap());
        
        Ok(Self {
            agent_id,
            process,
            stdin,
            stdout,
            request_id: 0,
        })
    }
    
    /// 发送请求
    async fn send_request(&mut self, method: &str, params: serde_json::Value) -> Result<serde_json::Value> {
        self.request_id += 1;
        
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: self.request_id,
            method: method.to_string(),
            params,
        };
        
        // 发送
        let request_json = serde_json::to_string(&request)?;
        self.stdin.write_all(request_json.as_bytes()).await?;
        self.stdin.write_all(b"\n").await?;
        self.stdin.flush().await?;
        
        // 接收响应
        let mut line = String::new();
        self.stdout.read_line(&mut line).await?;
        
        let response: JsonRpcResponse = serde_json::from_str(&line)?;
        
        if let Some(error) = response.error {
            return Err(anyhow!("RPC error: {}", error.message));
        }
        
        Ok(response.result.unwrap_or(json!(null)))
    }
    
    /// 执行任务
    pub async fn execute(&mut self, task: &str) -> Result<String> {
        // 1. Initialize（如果需要）
        // 2. 发送任务
        let result = self.send_request("execute", json!({
            "task": task
        })).await?;
        
        // 3. 提取结果
        Ok(result.as_str().unwrap_or("").to_string())
    }
    
    pub async fn close(mut self) -> Result<()> {
        self.process.kill().await?;
        Ok(())
    }
}
```

---

## 4. 推荐方案

### 4.1 方案对比

| 方案 | 复杂度 | 功能 | 推荐 |
|------|--------|------|------|
| 官方 SDK 完整实现 | 高 | 完整 | Phase 3B |
| 官方 SDK 简化实现 | 中 | 基本 | Phase 3A ✅ |
| 简单 JSON-RPC | 低 | 最小 | 备选 |

### 4.2 推荐：Phase 3A

**理由**:
- 使用官方 SDK（保证兼容性）
- 实现简化（快速交付）
- 可扩展（后续升级到 Phase 3B）

**实施**:
1. 实现 SimpleClient（最小 Client trait）
2. 使用 ClientSideConnection
3. 支持基本的 initialize + prompt
4. 获取执行结果

---

## 5. 实施步骤（Phase 3A）

### Step 1: 创建 SimpleClient（1 小时）

```bash
# 创建 src/acp/simple_client.rs
# 实现最小的 Client trait
```

### Step 2: 重构 ACPClient（2 小时）

```bash
# 修改 src/acp/client.rs
# 使用 ClientSideConnection
# 实现 initialize + prompt
```

### Step 3: 测试（1 小时）

```bash
cargo test --lib acp
```

### Step 4: 集成测试（1 小时）

```bash
# 测试真实的 agent 调用
```

**总计**: 5 小时（半天）

---

## 6. 后续计划

### Phase 3B（1 周）

**目标**: 完整的 Client 实现

**任务**:
1. 完整的 Client trait 实现
2. 流式输出支持
3. 权限请求处理
4. 终端管理
5. 文件操作

---

## 7. 总结

### 7.1 Phase 3A 交付物

**代码**:
- SimpleClient（约 50 行）
- 重构的 ACPClient（约 150 行）
- **总计**: 约 200 行

**功能**:
- ✅ 真实的 agent 调用
- ✅ 基本的任务执行
- ✅ 获取执行结果
- ⏳ 流式输出（Phase 3B）
- ⏳ 高级功能（Phase 3B）

### 7.2 优势

1. **快速交付**: 半天完成
2. **使用官方 SDK**: 保证兼容性
3. **可扩展**: 后续升级到完整实现
4. **低风险**: 简化实现，易于测试

---

**状态**: ✅ 方案确定  
**预计时间**: 半天（5 小时）  
**难度**: 中等  
**优先级**: 高
