# ACP 在架构中的定位分析

**文档版本**: 1.0  
**创建日期**: 2026-03-07  
**问题**: ACP 可以做在 LLM Provider 同层补充吗？

---

## 1. 当前架构分析

### 1.1 LLM Provider 层

**定义**：
```rust
// src/provider/base.rs
#[async_trait]
pub trait LLMProvider: Send + Sync {
    fn default_model(&self) -> &str;
    async fn chat(&self, req: ChatRequest) -> LLMResponse;
}

pub struct ChatRequest {
    pub messages: Vec<ChatMessage>,
    pub tools: Option<Vec<ToolDefinition>>,
    pub model: Option<String>,
    pub max_tokens: i32,
    pub temperature: f32,
    pub reasoning_effort: Option<String>,
}
```

**职责**：
- 提供 LLM 推理能力
- 输入：消息 + 工具定义
- 输出：文本 + 工具调用

**现有实现**：
- OpenAI Compatible Provider
- 支持多种模型（OpenAI, Anthropic, Custom, etc）

### 1.2 Agent 层

**定义**：
```rust
// src/agent/loop_core.rs
pub struct AgentLoop {
    provider: Arc<dyn LLMProvider>,
    tools: Arc<ToolRegistry>,
    sessions: Arc<SessionManager>,
    // ...
}
```

**职责**：
- 编排 LLM 调用
- 执行工具调用
- 管理对话循环

**流程**：
```
User Input
  ↓
Agent Loop
  ↓
LLM Provider (推理)
  ↓
Tool Calls
  ↓
Tool Execution
  ↓
Loop...
```

---

## 2. ACP 的本质

### 2.1 ACP 是什么

**ACP 不是 LLM Provider**：
- ACP 是一个 **完整的 Coding Agent**
- 内部已经包含：
  - LLM Provider（自己的模型调用）
  - Tool System（文件操作、命令执行）
  - Agent Loop（自己的循环逻辑）

**ACP 的架构**：
```
ACP Agent (codex/claude/pi)
  ├── LLM Provider (内置)
  ├── Tool System (内置)
  ├── Agent Loop (内置)
  └── File System (内置)
```

### 2.2 ACP vs LLM Provider

| 方面 | LLM Provider | ACP Agent |
|------|-------------|-----------|
| 定位 | 推理引擎 | 完整 Agent |
| 输入 | 消息 + 工具定义 | 任务描述 |
| 输出 | 文本 + 工具调用 | 完成的任务 |
| 工具执行 | 外部执行 | 内部执行 |
| 循环逻辑 | 外部控制 | 内部控制 |
| 独立性 | 依赖外部 | 完全独立 |

**关键区别**：
- LLM Provider：只负责"思考"
- ACP Agent：负责"思考 + 行动 + 完成任务"

---

## 3. ACP 的正确定位

### 3.1 不应该放在 Provider 层

**原因 1：职责不匹配**
```rust
// LLM Provider 的职责
trait LLMProvider {
    async fn chat(&self, req: ChatRequest) -> LLMResponse;
    // 只负责推理，不执行工具
}

// ACP 的能力
struct ACPAgent {
    async fn execute(&self, task: &str) -> TaskResult;
    // 完整的任务执行，包括推理 + 工具执行
}
```

**原因 2：抽象层级不同**
- LLM Provider：底层能力（推理）
- ACP Agent：高层能力（任务完成）

**原因 3：控制流不同**
- LLM Provider：被动调用（Agent 控制循环）
- ACP Agent：主动执行（自己控制循环）

### 3.2 应该放在 Tool 层

**ACP 作为工具**：
```rust
// src/tools/acp.rs
pub struct ACPTool {
    session_manager: Arc<ACPSessionManager>,
}

impl Tool for ACPTool {
    fn name(&self) -> &str {
        "acp_execute"
    }
    
    async fn execute(&self, args: &str, context: &ToolContext) -> Result<String> {
        // 调用 ACP Agent 完成任务
        let result = self.session_manager
            .execute(agent_id, task)
            .await?;
        Ok(result)
    }
}
```

**为什么是工具**：
1. **委托模式** - nanobot-rs 的 Agent 委托任务给 ACP Agent
2. **工具语义** - "使用 Codex 完成某个任务"
3. **控制流** - nanobot-rs 的 Agent 控制何时调用

---

## 4. 架构对比

### 4.1 方案 A：ACP 作为 Provider（❌ 不推荐）

```
User Input
  ↓
nanobot-rs Agent
  ↓
ACP Provider (?)
  ↓
??? (ACP 自己会执行工具，无法控制)
```

**问题**：
- ❌ ACP 会自己执行工具，nanobot-rs 无法控制
- ❌ 两个 Agent 循环冲突
- ❌ 职责混乱

### 4.2 方案 B：ACP 作为 Tool（✅ 推荐）

```
User Input
  ↓
nanobot-rs Agent
  ├── LLM Provider (推理)
  └── Tools
      ├── read_file
      ├── write_file
      └── acp_execute (委托给 ACP Agent)
          ↓
          ACP Agent (完整执行)
```

**优势**：
- ✅ 职责清晰：nanobot-rs 决策，ACP 执行
- ✅ 控制流清晰：nanobot-rs 控制何时调用
- ✅ 可组合：可以和其他工具组合使用

---

## 5. 使用场景

### 5.1 场景 1：简单任务（不需要 ACP）

```
用户: "读取 README.md 的内容"

nanobot-rs Agent:
  ↓ LLM Provider 推理
  ↓ 决定使用 read_file 工具
  ↓ 执行 read_file
  ↓ 返回结果
```

### 5.2 场景 2：复杂编码任务（使用 ACP）

```
用户: "用 Rust 实现一个 HTTP 服务器"

nanobot-rs Agent:
  ↓ LLM Provider 推理
  ↓ 决定这是复杂编码任务
  ↓ 调用 acp_execute 工具
      ↓
      ACP Agent (codex):
        ↓ 自己的 LLM 推理
        ↓ 自己的工具执行
        ↓ 完成任务
      ↓
  ↓ 返回结果给用户
```

### 5.3 场景 3：混合任务

```
用户: "分析项目结构，然后用 Codex 重构认证模块"

nanobot-rs Agent:
  ↓ LLM Provider 推理
  ↓ 步骤 1: 使用 read_file 分析项目
  ↓ 步骤 2: 调用 acp_execute 重构
      ↓
      ACP Agent (codex):
        ↓ 重构认证模块
      ↓
  ↓ 步骤 3: 验证结果
  ↓ 返回给用户
```

---

## 6. 实现建议

### 6.1 核心设计

**ACP 作为工具**：
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
        "Execute a coding task using an ACP agent (codex, claude, pi, gemini, opencode)"
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
                    "description": "The coding task to execute"
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
        
        // 创建或获取会话
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
                ACPEvent::Thinking { text } => {
                    info!(target: TARGET_AGENT, "💭 ", text);
                }
                ACPEvent::ToolCall { name, args } => {
                    info!(target: TARGET_AGENT, "🔧 {}({})", name, args);
                }
                ACPEvent::Output { text } => {
                    output.push_str(&text);
                }
                ACPEvent::Error { message } => {
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

### 6.2 注册工具

```rust
// src/tools/registry.rs
pub fn register_builtin_tools(registry: &mut ToolRegistry) {
    // 现有工具
    registry.register(Box::new(ReadFileTool::new()));
    registry.register(Box::new(WriteFileTool::new()));
    // ...
    
    // ACP 工具
    if config.acp.enabled {
        let acp_tool = ACPTool::new(acp_session_manager);
        registry.register(Box::new(acp_tool));
    }
}
```

### 6.3 Agent 使用

```rust
// Agent 自动决策何时使用 ACP
let response = provider.chat(ChatRequest {
    messages: vec![
        ChatMessage::user("用 Rust 实现一个 HTTP 服务器")
    ],
    tools: Some(vec![
        // 所有可用工具，包括 acp_execute
        tool_registry.get_definitions()
    ]),
    // ...
}).await?;

// LLM 可能返回：
// ToolCall {
//     name: "acp_execute",
//     args: {
//         "agent_id": "codex",
//         "task": "Build a HTTP server with Rust and Axum"
//     }
// }
```

---

## 7. 对比总结

### 7.1 架构层级

```
┌─────────────────────────────────────┐
│  User Interface (IM/CLI/Desktop)    │
└─────────────────────────────────────┘
              ↓
┌─────────────────────────────────────┐
│  Agent Layer (nanobot-rs)           │
│  - AgentLoop                        │
│  - Session Management               │
└─────────────────────────────────────┘
              ↓
┌──────────────────┬──────────────────┐
│  LLM Provider    │  Tool System     │
│  - OpenAI        │  - read_file     │
│  - Anthropic     │  - write_file    │
│  - Custom        │  - acp_execute ⭐│
└──────────────────┴──────────────────┘
                         ↓
              ┌──────────────────────┐
              │  ACP Agent           │
              │  - codex/claude/pi   │
              └──────────────────────┘
```

### 7.2 定位对比

| 层级 | 组件 | 职责 | ACP 是否属于 |
|------|------|------|-------------|
| UI 层 | IM/CLI/Desktop | 用户交互 | ❌ |
| Agent 层 | AgentLoop | 任务编排 | ❌ |
| Provider 层 | LLM Provider | 推理能力 | ❌ |
| Tool 层 | Tool System | 能力扩展 | ✅ **是** |
| External | ACP Agent | 独立 Agent | - |

### 7.3 为什么是 Tool 层

**1. 语义正确**
- "使用 Codex 完成某个任务" = 工具调用
- 不是 "使用 Codex 作为推理引擎"

**2. 控制流正确**
- nanobot-rs Agent 决定何时调用
- ACP Agent 被动执行

**3. 职责清晰**
- nanobot-rs：高层决策
- ACP：具体执行

**4. 可组合**
- 可以和其他工具组合
- 可以在复杂流程中使用

---

## 8. 结论

### 8.1 回答原问题

**问题**: ACP 可以做在 LLM Provider 同层补充吗？

**答案**: ❌ **不应该**

**原因**：
1. **职责不匹配** - ACP 是完整 Agent，不是推理引擎
2. **抽象层级不同** - Provider 是底层，ACP 是高层
3. **控制流冲突** - Provider 被动，ACP 主动

### 8.2 正确定位

**ACP 应该作为 Tool**：
- ✅ 语义正确：工具调用
- ✅ 控制流正确：nanobot-rs 控制
- ✅ 职责清晰：委托执行
- ✅ 可组合：和其他工具配合

### 8.3 架构优势

**清晰的分层**：
```
User → nanobot-rs Agent → LLM Provider (推理)
                        → Tools (能力)
                            → acp_execute (委托)
                                → ACP Agent (执行)
```

**灵活的组合**：
- 简单任务：直接用内置工具
- 复杂任务：委托给 ACP
- 混合任务：组合使用

**可控的执行**：
- nanobot-rs 决定何时使用 ACP
- nanobot-rs 可以验证 ACP 的结果
- nanobot-rs 可以在 ACP 前后做其他操作

---

**状态**: ✅ 完成
**结论**: ACP 应该作为 Tool，不是 Provider
**理由**: 职责、抽象层级、控制流都不匹配
