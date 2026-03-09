# ACP 集成设计思路

**文档版本**: 1.0  
**创建日期**: 2026-03-07  
**目标**: 理清 ACP 集成的设计思路，确保方案合理可行

---

## 1. 核心问题

### 1.1 我们要解决什么问题？

**现状**：
- nanobot-rs 目前只能通过 `exec` 工具调用外部命令
- 调用 coding agents（如 codex, claude）需要手动处理 stdio、解析输出
- 没有标准化的接口
- 每个 agent 需要单独适配

**问题**：
```rust
// 当前方式：通过 exec 调用
exec("codex exec 'Build a web server'")
// 问题：
// 1. 输出是纯文本，难以解析
// 2. 无法获取中间状态（thinking, tool calls）
// 3. 无法管理会话（持久化、恢复）
// 4. 无法控制权限和审批
```

**期望**：
```rust
// 理想方式：通过 ACP 协议
acp_execute({
    "agent_id": "codex",
    "task": "Build a web server"
})
// 优势：
// 1. 结构化输出（JSON events）
// 2. 实时获取中间状态
// 3. 会话管理
// 4. 权限控制
```

### 1.2 为什么选择 ACP？

**ACP 的优势**：
1. **标准化** - 一个协议，所有 agent
2. **生态** - Zed, OpenClaw 都在用
3. **完整** - 支持会话、权限、流式输出
4. **未来** - 更多 agent 会支持 ACP

**对比其他方案**：

| 方案 | 优势 | 劣势 |
|------|------|------|
| 直接 exec | 简单 | 无结构化输出、无会话管理 |
| 自定义协议 | 完全控制 | 需要每个 agent 单独适配 |
| ACP 协议 | 标准化、生态好 | 需要实现协议 |
| MCP 协议 | 工具标准化 | 不是为 coding agent 设计 |

**结论**：ACP 是最佳选择

---

## 2. 架构设计思路

### 2.1 分层设计

```
┌─────────────────────────────────────────┐
│  Application Layer (应用层)             │
│  - Agent 调用 acp_execute 工具          │
│  - 用户通过命令行/IM 交互                │
└─────────────────────────────────────────┘
              ↓
┌─────────────────────────────────────────┐
│  Tool Layer (工具层)                    │
│  - ACPTool: 封装 ACP 调用               │
│  - 参数验证、错误处理                    │
└─────────────────────────────────────────┘
              ↓
┌─────────────────────────────────────────┐
│  Session Layer (会话层)                 │
│  - ACPSessionManager: 管理多个会话      │
│  - 会话创建、查找、关闭                  │
│  - TTL 管理、并发控制                    │
└─────────────────────────────────────────┘
              ↓
┌─────────────────────────────────────────┐
│  Client Layer (客户端层)                │
│  - ACPClient: 单个 ACP 会话              │
│  - 进程管理、stdio 通信                  │
│  - 请求/响应处理                         │
└─────────────────────────────────────────┘
              ↓
┌─────────────────────────────────────────┐
│  Protocol Layer (协议层)                │
│  - ACPRequest/Response/Event 定义       │
│  - JSON 序列化/反序列化                  │
└─────────────────────────────────────────┘
              ↓
┌─────────────────────────────────────────┐
│  Transport Layer (传输层)               │
│  - stdio 读写                           │
│  - 进程管理                              │
└─────────────────────────────────────────┘
```

**设计原则**：
1. **单一职责** - 每层只做一件事
2. **依赖倒置** - 上层依赖接口，不依赖实现
3. **开闭原则** - 易于扩展新 agent，无需修改现有代码

### 2.2 核心抽象

#### 2.2.1 ACPClient - 单个会话

**职责**：
- 启动 agent 进程
- 管理 stdio 通信
- 发送请求、接收响应
- 处理进程生命周期

**接口**：
```rust
pub struct ACPClient {
    agent_id: String,
    process: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    session_id: String,
}

impl ACPClient {
    // 启动 agent
    pub async fn spawn(config: ACPConfig) -> Result<Self>;
    
    // 发送请求
    pub async fn send_request(&mut self, request: ACPRequest) -> Result<()>;
    
    // 接收响应（单个）
    pub async fn receive_response(&mut self) -> Result<ACPResponse>;
    
    // 执行任务（流式）
    pub async fn execute(&mut self, task: &str) -> impl Stream<Item = ACPEvent>;
    
    // 关闭会话
    pub async fn close(mut self) -> Result<()>;
}
```

**关键设计决策**：

**Q1: 为什么用 Child 而不是直接用 Command？**
- A: Child 可以持有进程句柄，管理生命周期
- 可以在 drop 时自动清理进程

**Q2: 为什么用 BufReader？**
- A: ACP 协议是行分隔的 JSON，BufReader 可以按行读取
- 提高性能，减少系统调用

**Q3: 为什么 execute 返回 Stream？**
- A: ACP 是流式协议，事件是逐个到达的
- Stream 可以实时处理，不需要等待全部完成

#### 2.2.2 ACPSessionManager - 会话管理

**职责**：
- 管理多个 ACP 会话
- 会话创建、查找、关闭
- TTL 管理、并发控制
- Agent 配置管理

**接口**：
```rust
pub struct ACPSessionManager {
    sessions: Arc<DashMap<String, ACPSession>>,
    config: ACPConfig,
    max_concurrent: usize,
}

impl ACPSessionManager {
    // 创建会话
    pub async fn create_session(
        &self,
        agent_id: &str,
        cwd: Option<PathBuf>,
    ) -> Result<String>;
    
    // 获取会话
    pub fn get_session(&self, session_id: &str) -> Option<&ACPSession>;
    
    // 执行任务
    pub async fn execute(
        &self,
        session_id: &str,
        task: &str,
    ) -> Result<impl Stream<Item = ACPEvent>>;
    
    // 关闭会话
    pub async fn close_session(&self, session_id: &str) -> Result<()>;
    
    // 清理过期会话
    pub async fn cleanup_expired(&self);
}
```

**关键设计决策**：

**Q1: 为什么用 DashMap 而不是 HashMap + RwLock？**
- A: DashMap 是并发安全的 HashMap，性能更好
- 避免锁竞争，支持高并发

**Q2: 为什么需要 max_concurrent？**
- A: 限制并发会话数，避免资源耗尽
- 每个会话都是一个进程，需要控制

**Q3: 如何处理 TTL？**
- A: 后台任务定期检查 last_active，清理过期会话
- 或者在每次访问时检查

#### 2.2.3 ACPTool - 工具封装

**职责**：
- 封装 ACP 调用为工具接口
- 参数验证
- 错误处理
- 与 ToolRegistry 集成

**接口**：
```rust
pub struct ACPTool {
    session_manager: Arc<ACPSessionManager>,
}

#[async_trait]
impl Tool for ACPTool {
    fn name(&self) -> &str {
        "acp_execute"
    }
    
    fn description(&self) -> &str {
        "Execute a task using an ACP agent"
    }
    
    fn parameters(&self) -> serde_json::Value {
        // JSON Schema
    }
    
    async fn execute(&self, args: &str, context: &ToolContext) -> Result<String> {
        // 1. 解析参数
        // 2. 创建或获取会话
        // 3. 执行任务
        // 4. 收集输出
        // 5. 返回结果
    }
}
```

**关键设计决策**：

**Q1: 一次性会话 vs 持久化会话？**
- A: 支持两种模式
  - 默认：一次性会话（每次调用创建新会话）
  - 可选：持久化会话（通过 session_id 复用）

**Q2: 如何处理流式输出？**
- A: 在工具层收集所有事件，最后返回完整输出
- 或者：支持回调，实时通知上层

**Q3: 如何处理错误？**
- A: 捕获所有错误，转为友好的错误消息
- 区分：协议错误、进程错误、超时错误

---

## 3. 协议设计思路

### 3.1 ACP 协议理解

**ACP 协议本质**：
- JSON-RPC 2.0 over stdio
- 请求/响应模式
- 事件流式传输

**通信流程**：
```
Client                          Agent
  |                               |
  |-- Request: Execute task ----->|
  |                               |
  |<-- Event: Thinking ----------|
  |<-- Event: ToolCall ----------|
  |<-- Event: ToolResult --------|
  |<-- Event: Output ------------|
  |<-- Response: Complete -------|
  |                               |
```

**关键点**：
1. **异步** - 请求和响应不是一一对应的
2. **流式** - 事件是逐个到达的
3. **双向** - Client 可以发送请求，Agent 可以请求审批

### 3.2 协议定义

**简化版协议**（足够用）：

```rust
// 请求
pub enum ACPRequest {
    Execute { task: String },
    Cancel,
    Close,
}

// 响应
pub enum ACPResponse {
    Event(ACPEvent),
    Complete,
    Error(String),
}

// 事件
pub enum ACPEvent {
    Thinking { text: String },
    ToolCall { name: String, args: String },
    ToolResult { name: String, result: String },
    Output { text: String },
    Error(String),
}
```

**关键设计决策**：

**Q1: 是否需要完整实现 JSON-RPC 2.0？**
- A: 不需要，简化版足够
- 只需要支持我们用到的功能

**Q2: 如何处理 agent 请求审批？**
- A: 第一版不支持，自动批准
- 未来可以添加 ApprovalNeeded 事件

**Q3: 如何处理协议版本？**
- A: 第一版不处理，假设兼容
- 未来可以添加版本协商

### 3.3 错误处理

**错误类型**：
1. **协议错误** - JSON 解析失败、格式错误
2. **进程错误** - 进程启动失败、崩溃
3. **超时错误** - 响应超时
4. **业务错误** - Agent 返回错误

**处理策略**：
```rust
pub enum ACPError {
    ProtocolError(String),
    ProcessError(String),
    TimeoutError,
    AgentError(String),
}

impl ACPClient {
    async fn execute(&mut self, task: &str) -> Result<impl Stream<Item = ACPEvent>> {
        // 1. 发送请求
        self.send_request(ACPRequest::Execute { task }).await
            .map_err(|e| ACPError::ProtocolError(e.to_string()))?;
        
        // 2. 接收事件流
        Ok(stream! {
            loop {
                // 设置超时
                let response = timeout(
                    Duration::from_secs(60),
                    self.receive_response()
                ).await;
                
                match response {
                    Ok(Ok(ACPResponse::Event(event))) => yield event,
                    Ok(Ok(ACPResponse::Complete)) => break,
                    Ok(Ok(ACPResponse::Error(err))) => {
                        yield ACPEvent::Error(err);
                        break;
                    }
                    Ok(Err(e)) => {
                        yield ACPEvent::Error(format!("Protocol error: {}", e));
                        break;
                    }
                    Err(_) => {
                        yield ACPEvent::Error("Timeout".to_string());
                        break;
                    }
                }
            }
        })
    }
}
```

---

## 4. 实施策略

### 4.1 MVP 范围

**第一版（MVP）目标**：
- ✅ 支持单个 agent (codex)
- ✅ 支持基本的 execute 操作
- ✅ 一次性会话（不持久化）
- ✅ 简化的协议实现
- ❌ 不支持审批
- ❌ 不支持会话恢复
- ❌ 不支持多 agent

**为什么这样划分？**
- 快速验证可行性
- 降低复杂度
- 尽早获得反馈

### 4.2 渐进式实现

**Phase 1: 基础实现（2 周）**
```
目标：能跑起来
- ACPClient 基础实现
- 简化的协议定义
- 支持 codex
- 基本的错误处理
```

**Phase 2: 会话管理（1 周）**
```
目标：支持多会话
- ACPSessionManager
- 会话创建、查找、关闭
- TTL 管理
```

**Phase 3: 工具集成（1 周）**
```
目标：集成到 nanobot-rs
- ACPTool 实现
- 注册到 ToolRegistry
- 配置系统
```

**Phase 4: 多 Agent（1 周）**
```
目标：支持所有 agent
- claude, pi, gemini, opencode
- Agent 配置管理
- 环境变量管理
```

**Phase 5: 高级特性（2 周）**
```
目标：完善功能
- 流式输出优化
- 权限控制
- 会话持久化
- 性能优化
```

### 4.3 风险和挑战

**技术风险**：

1. **协议兼容性**
   - 风险：不同 agent 的 ACP 实现可能有差异
   - 缓解：先支持一个 agent，验证后再扩展

2. **进程管理**
   - 风险：进程泄漏、僵尸进程
   - 缓解：确保 drop 时清理，添加超时机制

3. **并发控制**
   - 风险：多会话并发可能导致资源耗尽
   - 缓解：限制并发数，添加队列

4. **错误处理**
   - 风险：各种边界情况难以处理
   - 缓解：充分测试，添加日志

**非技术风险**：

1. **Agent 可用性**
   - 风险：用户可能没有安装 agent
   - 缓解：提供清晰的错误提示和安装指南

2. **API Key 管理**
   - 风险：用户可能没有配置 API key
   - 缓解：从环境变量或配置文件读取

---

## 5. 关键决策

### 5.1 为什么不用 acpx？

**acpx 的问题**：
- 依赖 Node.js
- 多一层抽象
- 性能开销

**直接实现的优势**：
- 纯 Rust，无外部依赖
- 更好的控制
- 更好的性能
- 更好的错误处理

### 5.2 为什么不用 FFI 调用 TypeScript SDK？

**FFI 的问题**：
- 复杂度高
- 跨语言调用开销
- 难以调试

**直接实现的优势**：
- 简单直接
- 性能更好
- 易于维护

### 5.3 为什么用 Stream 而不是 Vec？

**Stream 的优势**：
- 实时处理，不需要等待全部完成
- 内存占用小
- 可以提前取消

**Vec 的问题**：
- 需要等待全部完成
- 内存占用大
- 无法提前取消

### 5.4 为什么用 DashMap 而不是 HashMap + RwLock？

**DashMap 的优势**：
- 并发性能更好
- API 更简单
- 避免死锁

**HashMap + RwLock 的问题**：
- 锁竞争
- 容易死锁
- 性能较差

---

## 6. 测试策略

### 6.1 单元测试

**ACPClient 测试**：
```rust
#[tokio::test]
async fn test_spawn_client() {
    let config = ACPConfig {
        agent_id: "codex".to_string(),
        command: "echo".to_string(), // mock
        cwd: None,
        env: HashMap::new(),
    };
    
    let client = ACPClient::spawn(config).await;
    assert!(client.is_ok());
}

#[tokio::test]
async fn test_execute_task() {
    let mut client = create_mock_client().await;
    let mut events = client.execute("test task").await.unwrap();
    
    let first_event = events.next().await;
    assert!(matches!(first_event, Some(ACPEvent::Output { .. })));
}
```

**ACPSessionManager 测试**：
```rust
#[tokio::test]
async fn test_create_session() {
    let manager = ACPSessionManager::new(config);
    let session_id = manager.create_session("codex", None).await.unwrap();
    assert!(!session_id.is_empty());
}

#[tokio::test]
async fn test_concurrent_sessions() {
    let manager = Arc::new(ACPSessionManager::new(config));
    
    let handles: Vec<_> = (0..10).map(|_| {
        let m = manager.clone();
        tokio::spawn(async move {
            m.create_session("codex", None).await
        })
    }).collect();
    
    let results = futures::future::join_all(handles).await;
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 10);
}
```

### 6.2 集成测试

**端到端测试**：
```rust
#[tokio::test]
async fn test_acp_tool_execute() {
    let tool = ACPTool::new(session_manager);
    
    let result = tool.execute(r#"{
        "agent_id": "codex",
        "task": "echo hello"
    }"#, &context).await;
    
    assert!(result.is_ok());
    assert!(result.unwrap().contains("hello"));
}
```

### 6.3 Mock 策略

**Mock Agent**：
```rust
// 创建一个简单的 mock agent
// 用于测试，不需要真实的 codex
pub fn create_mock_agent() -> Command {
    // 返回预定义的 JSON 事件
    Command::new("sh")
        .arg("-c")
        .arg(r#"
            echo '{"type":"event","kind":"thinking","text":"Processing..."}'
            echo '{"type":"event","kind":"output","text":"Hello World"}'
            echo '{"type":"complete"}'
        "#)
}
```

---

## 7. 总结

### 7.1 核心思路

1. **分层设计** - 清晰的职责划分
2. **渐进实现** - MVP 先行，逐步完善
3. **风险控制** - 识别风险，提前缓解
4. **测试驱动** - 充分测试，确保质量

### 7.2 关键决策

1. **直接实现 ACP Client** - 不依赖 acpx
2. **简化协议** - 只实现需要的功能
3. **流式处理** - 使用 Stream 而不是 Vec
4. **并发安全** - 使用 DashMap

### 7.3 下一步

1. 开始实施 Phase 1 - 基础实现
2. 创建 `src/acp/` 模块
3. 实现 ACPClient
4. 编写测试

---

**状态**: ✅ 设计思路已完成
**下一步**: 开始编码实现
