# 通用 Agent 架构设计 - 从 Codex/Claude Code 学习

**文档版本**: 1.0  
**创建日期**: 2026-03-07  
**目标**: 让 nanobot-rs 适配各种场景（IM、CLI、Desktop、Web）

---

## 目录

1. [Codex/Claude Code 架构分析](#1-codexclaude-code-架构分析)
2. [通用化设计原则](#2-通用化设计原则)
3. [核心抽象层](#3-核心抽象层)
4. [场景适配器](#4-场景适配器)
5. [实施方案](#5-实施方案)

---

## 1. Codex/Claude Code 架构分析

### 1.1 核心特性

从配置和文档中提取的关键设计：

#### 1.1.1 多模式运行

```toml
# ~/.codex/config.toml

# 交互模式（默认）
codex

# 非交互模式
codex exec "prompt"
codex --print "prompt"  # 纯输出，无文件操作

# 后台模式
codex exec --full-auto "prompt"  # 自动批准
codex --dangerously-bypass-approvals-and-sandbox  # 完全自动
```

**设计亮点**：
- ✅ 同一个 agent，多种运行模式
- ✅ 通过参数控制行为
- ✅ 适配不同场景（交互/自动化/批处理）

#### 1.1.2 权限和沙箱系统

```toml
[profiles.auto-max]
approval_policy = "never"
sandbox_mode = "workspace-write"

[profiles.review]
approval_policy = "on-request"
sandbox_mode = "workspace-write"
```

**沙箱级别**：
- `read-only` - 只读
- `workspace-write` - 工作区可写
- `danger-full-access` - 完全访问

**审批策略**：
- `never` - 自动执行
- `on-request` - 按需确认
- `always` - 每次都问

**设计亮点**：
- ✅ 安全性分级
- ✅ 场景化配置（review vs build）
- ✅ 用户可控

#### 1.1.3 工具系统

```bash
# 工具白名单
claude --allowedTools "Edit Bash Read"

# 工具黑名单
claude --disallowedTools "Bash(git:*)"

# 完全禁用
claude --tools ""

# 默认全部
claude --tools "default"
```

**设计亮点**：
- ✅ 细粒度控制
- ✅ 支持模式匹配（如 `Bash(git:*)`）
- ✅ 白名单 + 黑名单

#### 1.1.4 上下文管理

```toml
model_context_window = 1000000
model_auto_compact_token_limit = 900000

[projects."/Users/yjhmelody/code/github"]
trust_level = "trusted"
```

**设计亮点**：
- ✅ 自动压缩上下文
- ✅ 项目级信任管理
- ✅ 大上下文窗口支持

#### 1.1.5 会话管理

```bash
# 继续最近会话
codex --continue

# 恢复指定会话
codex --resume <session-id>

# Fork 会话
codex --fork-session

# 临时会话（不保存）
codex --ephemeral
```

**设计亮点**：
- ✅ 会话持久化
- ✅ 会话恢复
- ✅ 会话分支
- ✅ 临时模式

#### 1.1.6 输出格式

```bash
# 文本输出（默认）
codex --print "prompt"

# JSON 输出
codex --print --output-format json "prompt"

# 流式 JSON
codex --print --output-format stream-json "prompt"

# 结构化输出
codex --json-schema '{"type":"object",...}' "prompt"
```

**设计亮点**：
- ✅ 多种输出格式
- ✅ 流式输出
- ✅ 结构化输出（JSON Schema）

#### 1.1.7 扩展系统

```bash
# MCP 服务器
codex --mcp-config servers.json

# 插件
claude --plugin-dir ./plugins

# 自定义 Agent
claude --agents '{"reviewer": {...}}'
```

**设计亮点**：
- ✅ MCP 协议支持
- ✅ 插件系统
- ✅ 自定义 Agent

### 1.2 架构总结

```
┌─────────────────────────────────────────────┐
│           CLI Interface                     │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  │
│  │Interactive│  │  Exec    │  │  Print   │  │
│  │   Mode   │  │  Mode    │  │  Mode    │  │
│  └──────────┘  └──────────┘  └──────────┘  │
└─────────────────────────────────────────────┘
              ↓
┌─────────────────────────────────────────────┐
│         Core Agent Engine                   │
│  ┌──────────────────────────────────────┐   │
│  │  Session Manager                     │   │
│  │  - Persistence                       │   │
│  │  - Resume/Fork                       │   │
│  └──────────────────────────────────────┘   │
│  ┌──────────────────────────────────────┐   │
│  │  Permission System                   │   │
│  │  - Approval Policy                   │   │
│  │  - Sandbox Mode                      │   │
│  └──────────────────────────────────────┘   │
│  ┌──────────────────────────────────────┐   │
│  │  Tool Registry                       │   │
│  │  - Built-in Tools                    │   │
│  │  - MCP Servers                       │   │
│  │  - Plugins                           │   │
│  └──────────────────────────────────────┘   │
│  ┌──────────────────────────────────────┐   │
│  │  Context Manager                     │   │
│  │  - Auto Compaction                   │   │
│  │  - Project Trust                     │   │
│  └──────────────────────────────────────┘   │
└─────────────────────────────────────────────┘
              ↓
┌─────────────────────────────────────────────┐
│         LLM Provider                        │
│  - OpenAI / Anthropic / Custom              │
└─────────────────────────────────────────────┘
```

---

## 2. 通用化设计原则

### 2.1 核心原则

#### 原则 1: 分离关注点

**问题**：当前 nanobot-rs 混合了 IM、Agent、工具逻辑

**方案**：
```
Core Agent (纯逻辑)
    ↓
Adapters (场景适配)
    ↓
Interfaces (IM/CLI/Desktop/Web)
```

#### 原则 2: 配置驱动

**问题**：行为硬编码，难以适配不同场景

**方案**：
```toml
[profiles.im-bot]
approval_policy = "never"
sandbox_mode = "workspace-write"
output_format = "text"
streaming = false

[profiles.desktop-ide]
approval_policy = "on-request"
sandbox_mode = "read-only"
output_format = "json"
streaming = true
diff_preview = true

[profiles.cli-tool]
approval_policy = "always"
sandbox_mode = "danger-full-access"
output_format = "text"
streaming = false
```

#### 原则 3: 接口抽象

**问题**：直接依赖具体实现（Telegram、Discord）

**方案**：
```rust
trait AgentInterface {
    async fn receive_input(&mut self) -> Result<Input>;
    async fn send_output(&mut self, output: Output) -> Result<()>;
    async fn request_approval(&mut self, action: Action) -> Result<bool>;
}
```

#### 原则 4: 可组合性

**问题**：功能耦合，难以复用

**方案**：
```rust
// 组合不同能力
let agent = AgentBuilder::new()
    .with_executor(ReActExecutor::new())
    .with_tools(vec![bash_tool, edit_tool])
    .with_approval(ApprovalSystem::new())
    .with_sandbox(Sandbox::workspace_write())
    .build();
```

---

## 3. 核心抽象层

### 3.1 AgentCore - 纯逻辑层

```rust
// src/core/agent.rs

/// 核心 Agent - 不依赖任何 UI/IO
pub struct AgentCore {
    executor: Arc<dyn AgentExecutor>,
    tools: Arc<ToolRegistry>,
    sessions: Arc<SessionManager>,
    context: Arc<ContextManager>,
    permissions: Arc<PermissionSystem>,
}

impl AgentCore {
    /// 执行任务 - 返回事件流
    pub async fn execute(
        &self,
        input: AgentInput,
        config: ExecutionConfig,
    ) -> impl Stream<Item = AgentEvent> {
        // 纯逻辑，不涉及 IO
    }
    
    /// 请求审批 - 返回 Future
    pub async fn request_approval(
        &self,
        action: Action,
    ) -> ApprovalRequest {
        // 返回请求，由外部处理
    }
}

/// Agent 输入 - 场景无关
pub struct AgentInput {
    pub prompt: String,
    pub images: Vec<Image>,
    pub context: Vec<ContextItem>,
    pub session_id: Option<String>,
}

/// Agent 事件 - 场景无关
pub enum AgentEvent {
    Thinking(String),
    ToolCall { name: String, args: String },
    ToolResult { name: String, result: String },
    ApprovalNeeded(ApprovalRequest),
    Progress { current: usize, total: usize },
    Output(String),
    Complete(ExecutionResult),
    Error(String),
}

/// 执行配置 - 场景特定
pub struct ExecutionConfig {
    pub approval_policy: ApprovalPolicy,
    pub sandbox_mode: SandboxMode,
    pub output_format: OutputFormat,
    pub streaming: bool,
    pub max_iterations: usize,
}
```

### 3.2 AgentInterface - 场景适配

```rust
// src/adapters/interface.rs

/// Agent 接口 - 场景适配层
#[async_trait]
pub trait AgentInterface: Send + Sync {
    /// 接收输入
    async fn receive_input(&mut self) -> Result<AgentInput>;
    
    /// 发送事件
    async fn send_event(&mut self, event: AgentEvent) -> Result<()>;
    
    /// 请求审批
    async fn request_approval(&mut self, request: ApprovalRequest) -> Result<bool>;
    
    /// 获取配置
    fn get_config(&self) -> ExecutionConfig;
}

/// 审批请求
pub struct ApprovalRequest {
    pub action: Action,
    pub risk_level: RiskLevel,
    pub preview: ActionPreview,
}

pub enum Action {
    ExecuteCommand { command: String },
    WriteFile { path: PathBuf, content: String },
    DeleteFile { path: PathBuf },
    NetworkRequest { url: String },
}

pub enum RiskLevel {
    Safe,      // 自动批准
    Low,       // 通知
    Medium,    // 需要确认
    High,      // 需要审查
    Critical,  // 需要明确批准
}
```

### 3.3 权限系统

```rust
// src/core/permissions.rs

pub struct PermissionSystem {
    policy: ApprovalPolicy,
    sandbox: SandboxMode,
    trust_manager: Arc<TrustManager>,
}

pub enum ApprovalPolicy {
    Never,      // 自动批准所有
    OnRequest,  // Agent 请求时才问
    Always,     // 每次都问
    Smart,      // 基于风险级别
}

pub enum SandboxMode {
    ReadOnly,
    WorkspaceWrite,
    FullAccess,
}

impl PermissionSystem {
    pub async fn check_permission(
        &self,
        action: &Action,
    ) -> Result<PermissionDecision> {
        // 1. 检查沙箱限制
        if !self.sandbox.allows(action) {
            return Ok(PermissionDecision::Denied);
        }
        
        // 2. 检查信任级别
        let risk = self.assess_risk(action);
        
        // 3. 根据策略决定
        match self.policy {
            ApprovalPolicy::Never => Ok(PermissionDecision::Approved),
            ApprovalPolicy::Always => Ok(PermissionDecision::NeedsApproval),
            ApprovalPolicy::Smart => {
                if risk <= RiskLevel::Low {
                    Ok(PermissionDecision::Approved)
                } else {
                    Ok(PermissionDecision::NeedsApproval)
                }
            }
            _ => Ok(PermissionDecision::NeedsApproval),
        }
    }
    
    fn assess_risk(&self, action: &Action) -> RiskLevel {
        match action {
            Action::ExecuteCommand { command } => {
                if command.contains("rm -rf") || command.contains("sudo") {
                    RiskLevel::Critical
                } else if command.starts_with("git") {
                    RiskLevel::Low
                } else {
                    RiskLevel::Medium
                }
            }
            Action::WriteFile { path, .. } => {
                if self.trust_manager.is_trusted(path) {
                    RiskLevel::Low
                } else {
                    RiskLevel::Medium
                }
            }
            Action::DeleteFile { .. } => RiskLevel::High,
            Action::NetworkRequest { .. } => RiskLevel::Medium,
        }
    }
}

pub enum PermissionDecision {
    Approved,
    Denied,
    NeedsApproval,
}
```

### 3.4 输出格式化

```rust
// src/core/formatter.rs

pub trait OutputFormatter: Send + Sync {
    fn format_event(&self, event: &AgentEvent) -> String;
    fn format_result(&self, result: &ExecutionResult) -> String;
}

pub struct TextFormatter;
impl OutputFormatter for TextFormatter {
    fn format_event(&self, event: &AgentEvent) -> String {
        match event {
            AgentEvent::Thinking(text) => format!("💭 {}", text),
            AgentEvent::ToolCall { name, .. } => format!("🔧 {}", name),
            AgentEvent::Output(text) => text.clone(),
            _ => String::new(),
        }
    }
}

pub struct JsonFormatter;
impl OutputFormatter for JsonFormatter {
    fn format_event(&self, event: &AgentEvent) -> String {
        serde_json::to_string(event).unwrap()
    }
}

pub struct MarkdownFormatter;
impl OutputFormatter for MarkdownFormatter {
    fn format_event(&self, event: &AgentEvent) -> String {
        match event {
            AgentEvent::ToolCall { name, args } => {
                format!("```bash\n{} {}\n```", name, args)
            }
            AgentEvent::Output(text) => text.clone(),
            _ => String::new(),
        }
    }
}
```

---

## 4. 场景适配器

### 4.1 IM Bot 适配器

```rust
// src/adapters/im.rs

pub struct IMBotAdapter {
    core: Arc<AgentCore>,
    bus: Arc<MessageBus>,
    formatter: Arc<dyn OutputFormatter>,
    config: ExecutionConfig,
}

#[async_trait]
impl AgentInterface for IMBotAdapter {
    async fn receive_input(&mut self) -> Result<AgentInput> {
        let msg = self.bus.receive_inbound().await?;
        Ok(AgentInput {
            prompt: msg.content,
            images: msg.media,
            context: vec![],
            session_id: Some(msg.session_key()),
        })
    }
    
    async fn send_event(&mut self, event: AgentEvent) -> Result<()> {
        match event {
            AgentEvent::Output(text) => {
                self.bus.publish_outbound(OutboundMessage {
                    content: self.formatter.format_event(&event),
                    ..Default::default()
                }).await?;
            }
            AgentEvent::ApprovalNeeded(req) => {
                // IM 场景：自动批准（用户已信任）
                // 或发送确认消息
            }
            _ => {
                // 其他事件：静默或记录日志
            }
        }
        Ok(())
    }
    
    async fn request_approval(&mut self, request: ApprovalRequest) -> Result<bool> {
        // IM 场景：根据风险级别决定
        match request.risk_level {
            RiskLevel::Safe | RiskLevel::Low => Ok(true),
            _ => {
                // 发送确认消息，等待回复
                self.send_approval_request(&request).await
            }
        }
    }
    
    fn get_config(&self) -> ExecutionConfig {
        ExecutionConfig {
            approval_policy: ApprovalPolicy::Smart,
            sandbox_mode: SandboxMode::WorkspaceWrite,
            output_format: OutputFormat::Text,
            streaming: false,
            max_iterations: 40,
        }
    }
}
```

### 4.2 CLI 适配器

```rust
// src/adapters/cli.rs

pub struct CLIAdapter {
    core: Arc<AgentCore>,
    formatter: Arc<dyn OutputFormatter>,
    interactive: bool,
}

#[async_trait]
impl AgentInterface for CLIAdapter {
    async fn receive_input(&mut self) -> Result<AgentInput> {
        if self.interactive {
            // 交互模式：从 stdin 读取
            let mut input = String::new();
            std::io::stdin().read_line(&mut input)?;
            Ok(AgentInput {
                prompt: input.trim().to_string(),
                images: vec![],
                context: vec![],
                session_id: None,
            })
        } else {
            // 非交互模式：从参数读取
            Ok(AgentInput {
                prompt: std::env::args().nth(1).unwrap_or_default(),
                images: vec![],
                context: vec![],
                session_id: None,
            })
        }
    }
    
    async fn send_event(&mut self, event: AgentEvent) -> Result<()> {
        match event {
            AgentEvent::Thinking(text) => {
                if self.interactive {
                    eprintln!("💭 {}", text);
                }
            }
            AgentEvent::ToolCall { name, args } => {
                if self.interactive {
                    eprintln!("🔧 {} {}", name, args);
                }
            }
            AgentEvent::Output(text) => {
                println!("{}", text);
            }
            AgentEvent::Complete(result) => {
                if let Some(output) = result.response {
                    println!("{}", output);
                }
            }
            _ => {}
        }
        Ok(())
    }
    
    async fn request_approval(&mut self, request: ApprovalRequest) -> Result<bool> {
        if !self.interactive {
            // 非交互模式：根据风险级别自动决定
            return Ok(request.risk_level <= RiskLevel::Medium);
        }
        
        // 交互模式：询问用户
        eprintln!("\n⚠️  Approval needed:");
        eprintln!("Action: {:?}", request.action);
        eprintln!("Risk: {:?}", request.risk_level);
        eprintln!("\nApprove? [y/N]: ");
        
        let mut response = String::new();
        std::io::stdin().read_line(&mut response)?;
        
        Ok(response.trim().to_lowercase() == "y")
    }
    
    fn get_config(&self) -> ExecutionConfig {
        ExecutionConfig {
            approval_policy: if self.interactive {
                ApprovalPolicy::OnRequest
            } else {
                ApprovalPolicy::Smart
            },
            sandbox_mode: SandboxMode::WorkspaceWrite,
            output_format: OutputFormat::Text,
            streaming: self.interactive,
            max_iterations: 40,
        }
    }
}
```

### 4.3 Desktop IDE 适配器

```rust
// src/adapters/desktop.rs

pub struct DesktopIDEAdapter {
    core: Arc<AgentCore>,
    tauri_handle: tauri::AppHandle,
    formatter: Arc<dyn OutputFormatter>,
}

#[async_trait]
impl AgentInterface for DesktopIDEAdapter {
    async fn receive_input(&mut self) -> Result<AgentInput> {
        // 从 Tauri 事件接收
        // 实现略
        todo!()
    }
    
    async fn send_event(&mut self, event: AgentEvent) -> Result<()> {
        // 发送到前端
        self.tauri_handle.emit_all("agent-event", &event)?;
        Ok(())
    }
    
    async fn request_approval(&mut self, request: ApprovalRequest) -> Result<bool> {
        // 显示 Diff 预览，等待用户确认
        let (tx, rx) = oneshot::channel();
        
        self.tauri_handle.emit_all("approval-request", ApprovalRequestPayload {
            request,
            response_channel: tx,
        })?;
        
        rx.await.map_err(|_| anyhow!("Approval timeout"))
    }
    
    fn get_config(&self) -> ExecutionConfig {
        ExecutionConfig {
            approval_policy: ApprovalPolicy::OnRequest,
            sandbox_mode: SandboxMode::ReadOnly,
            output_format: OutputFormat::Json,
            streaming: true,
            max_iterations: 40,
        }
    }
}
```

### 4.4 Web API 适配器

```rust
// src/adapters/web.rs

pub struct WebAPIAdapter {
    core: Arc<AgentCore>,
    formatter: Arc<dyn OutputFormatter>,
}

#[async_trait]
impl AgentInterface for WebAPIAdapter {
    async fn receive_input(&mut self) -> Result<AgentInput> {
        // 从 HTTP 请求接收
        todo!()
    }
    
    async fn send_event(&mut self, event: AgentEvent) -> Result<()> {
        // SSE 流式输出
        todo!()
    }
    
    async fn request_approval(&mut self, request: ApprovalRequest) -> Result<bool> {
        // Web API：不支持交互，自动拒绝高风险操作
        Ok(request.risk_level <= RiskLevel::Low)
    }
    
    fn get_config(&self) -> ExecutionConfig {
        ExecutionConfig {
            approval_policy: ApprovalPolicy::Smart,
            sandbox_mode: SandboxMode::ReadOnly,
            output_format: OutputFormat::Json,
            streaming: true,
            max_iterations: 20,
        }
    }
}
```

---

## 5. 实施方案

### 5.1 Phase 1: 核心抽象（2 周）

**目标**: 建立核心抽象层

**任务**:
1. ✅ 定义 `AgentCore`
2. ✅ 定义 `AgentInterface` trait
3. ✅ 实现 `PermissionSystem`
4. ✅ 实现 `OutputFormatter`
5. ✅ 重构现有代码

**交付物**:
- `src/core/agent.rs`
- `src/core/permissions.rs`
- `src/core/formatter.rs`
- `src/adapters/interface.rs`

### 5.2 Phase 2: 适配器实现（3 周）

**目标**: 实现各场景适配器

**任务**:
1. ✅ 实现 `IMBotAdapter`
2. ✅ 实现 `CLIAdapter`
3. ✅ 实现 `DesktopIDEAdapter`
4. ✅ 实现 `WebAPIAdapter`
5. ✅ 编写测试

**交付物**:
- `src/adapters/im.rs`
- `src/adapters/cli.rs`
- `src/adapters/desktop.rs`
- `src/adapters/web.rs`

### 5.3 Phase 3: 配置系统（1 周）

**目标**: 配置驱动的行为

**任务**:
1. ✅ 设计配置 schema
2. ✅ 实现 Profile 系统
3. ✅ 实现配置加载
4. ✅ 更新文档

**交付物**:
- `src/config/profiles.rs`
- `config.example.toml`

### 5.4 Phase 4: 集成测试（1 周）

**目标**: 验证各场景

**任务**:
1. ✅ IM Bot 场景测试
2. ✅ CLI 场景测试
3. ✅ Desktop 场景测试
4. ✅ Web API 场景测试

---

## 6. 配置示例

### 6.1 通用配置

```toml
# config.toml

[agent]
default_profile = "im-bot"

[profiles.im-bot]
approval_policy = "smart"
sandbox_mode = "workspace-write"
output_format = "text"
streaming = false
max_iterations = 40

[profiles.cli-interactive]
approval_policy = "on-request"
sandbox_mode = "workspace-write"
output_format = "text"
streaming = true
max_iterations = 40

[profiles.cli-auto]
approval_policy = "smart"
sandbox_mode = "workspace-write"
output_format = "text"
streaming = false
max_iterations = 40

[profiles.desktop-ide]
approval_policy = "on-request"
sandbox_mode = "read-only"
output_format = "json"
streaming = true
max_iterations = 40
diff_preview = true

[profiles.web-api]
approval_policy = "smart"
sandbox_mode = "read-only"
output_format = "json"
streaming = true
max_iterations = 20

[permissions]
trusted_directories = [
    "~/Projects",
    "~/code"
]

[permissions.risk_assessment]
# 命令风险评估规则
high_risk_commands = [
    "rm -rf",
    "sudo",
    "chmod 777"
]
```

### 6.2 使用示例

```bash
# IM Bot 模式（默认）
nanobot-rs gateway

# CLI 交互模式
nanobot-rs agent --profile cli-interactive

# CLI 自动模式
nanobot-rs agent --profile cli-auto -m "Build a web server"

# Desktop IDE 模式
nanobot-rs desktop --profile desktop-ide

# Web API 模式
nanobot-rs serve --profile web-api
```

---

## 7. 总结

### 7.1 从 Codex/Claude Code 学到的

1. **多模式运行** - 同一个 agent，不同运行方式
2. **权限分级** - 沙箱 + 审批策略
3. **工具控制** - 白名单 + 黑名单
4. **会话管理** - 持久化 + 恢复 + 分支
5. **输出格式** - 文本 + JSON + 流式
6. **扩展系统** - MCP + 插件

### 7.2 通用化的关键

1. **核心抽象** - `AgentCore` 纯逻辑，不依赖 UI
2. **场景适配** - `AgentInterface` 适配不同场景
3. **配置驱动** - Profile 系统控制行为
4. **权限系统** - 安全性分级
5. **可组合性** - 功能模块化

### 7.3 适配场景

| 场景 | 审批策略 | 沙箱模式 | 输出格式 | 流式 |
|------|---------|---------|---------|------|
| IM Bot | Smart | WorkspaceWrite | Text | ❌ |
| CLI Interactive | OnRequest | WorkspaceWrite | Text | ✅ |
| CLI Auto | Smart | WorkspaceWrite | Text | ❌ |
| Desktop IDE | OnRequest | ReadOnly | JSON | ✅ |
| Web API | Smart | ReadOnly | JSON | ✅ |

---

**下一步**: 开始实施 Phase 1 - 核心抽象层
