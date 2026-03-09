# ACP Phase 2 改进计划

**文档版本**: 1.0  
**创建日期**: 2026-03-08  
**目标**: 在无法使用官方 SDK 的情况下，改进当前 MVP 实施

---

## 1. 当前 MVP 限制回顾

### 1.1 主要限制

1. ❌ **占位符实现**
   - `execute()` 只返回模拟文本
   - 不会真正启动 ACP agent 进程

2. ❌ **未集成到系统**
   - 未添加到 Config
   - 未注册到 ToolRegistry
   - 用户无法实际使用

3. ❌ **无会话管理**
   - 每次调用创建新进程
   - 无法复用会话

4. ❌ **无流式输出**
   - 只返回最终结果
   - 看不到中间过程

5. ❌ **无错误恢复**
   - 进程崩溃无法处理
   - 超时无法检测

---

## 2. Phase 2 改进目标

### 2.1 核心改进（不依赖官方 SDK）

**目标**: 在无法使用官方 SDK 的情况下，实现基本可用的 ACP 集成

**策略**: 
- 手动实现简化版 JSON-RPC 协议
- 使用 tokio::process 管理进程
- 实现基本的 stdio 通信

### 2.2 改进优先级

**P0 - 系统集成（必须）**
1. 添加到 Config
2. 注册到 ToolRegistry
3. 用户可以实际调用

**P1 - 基本功能（重要）**
1. 真正启动 ACP agent 进程
2. 基本的 stdio 通信
3. 简单的错误处理

**P2 - 增强功能（可选）**
1. 会话管理
2. 流式输出
3. 超时控制

---

## 3. 实施计划

### 3.1 P0: 系统集成（30 分钟）

#### 3.1.1 添加到 Config

```rust
// src/types/config.rs
use crate::acp::config::ACPConfig;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Config {
    pub agents: AgentsConfig,
    pub channels: ChannelsConfig,
    pub providers: ProvidersConfig,
    pub gateway: GatewayConfig,
    pub tools: ToolsConfig,
    
    // 新增
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acp: Option<ACPConfig>,
}
```

#### 3.1.2 注册到 ToolRegistry

```rust
// src/tools/registry_builder.rs
use crate::acp::config::ACPConfig;
use crate::tools::acp::ACPTool;

impl ToolRegistryBuilder {
    pub fn build(self, config: &Config) -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        
        // 注册现有工具...
        
        // 注册 ACP 工具
        if let Some(acp_config) = &config.acp {
            if acp_config.enabled {
                registry.register(Arc::new(ACPTool::new(acp_config.clone())));
            }
        }
        
        registry
    }
}
```

#### 3.1.3 配置示例

```toml
# config.toml

[acp]
enabled = true
defaultAgent = "codex"
allowedAgents = ["codex"]

[acp.agents.codex]
command = "codex"

[acp.agents.codex.env]
OPENAI_API_KEY = "${OPENAI_API_KEY}"
```

### 3.2 P1: 基本功能（1 小时）

#### 3.2.1 改进 ACPClient

**当前问题**: 只返回占位符文本

**改进方案**: 实现基本的进程启动和 stdio 通信

```rust
// src/acp/client.rs
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Command, Child, ChildStdin, ChildStdout};
use serde_json::json;

pub struct ACPClient {
    agent_id: String,
    process: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl ACPClient {
    pub async fn spawn(
        agent_id: String,
        command: String,
        cwd: Option<PathBuf>,
        env: HashMap<String, String>,
    ) -> Result<Self> {
        let mut cmd = Command::new(&command);
        
        // 添加 exec 参数（非交互模式）
        cmd.arg("exec");
        
        if let Some(cwd) = cwd {
            cmd.current_dir(cwd);
        }
        
        for (key, value) in env {
            cmd.env(key, value);
        }
        
        cmd.stdin(Stdio::piped())
           .stdout(Stdio::piped())
           .stderr(Stdio::piped());
        
        let mut process = cmd.spawn()
            .context(format!("Failed to spawn ACP agent: {}", agent_id))?;
        
        let stdin = process.stdin.take()
            .ok_or_else(|| anyhow!("Failed to get stdin"))?;
        let stdout = BufReader::new(
            process.stdout.take()
                .ok_or_else(|| anyhow!("Failed to get stdout"))?
        );
        
        Ok(Self {
            agent_id,
            process,
            stdin,
            stdout,
        })
    }
    
    pub async fn execute(&mut self, task: &str) -> Result<String> {
        // 发送任务（简化版，直接作为命令行参数）
        // 注意：这不是标准的 ACP 协议，只是一个可用的实现
        
        // 读取输出
        let mut output = String::new();
        let mut line = String::new();
        
        // 设置超时
        let timeout = tokio::time::timeout(
            Duration::from_secs(300), // 5 分钟
            async {
                while self.stdout.read_line(&mut line).await? > 0 {
                    output.push_str(&line);
                    line.clear();
                }
                Ok::<_, anyhow::Error>(())
            }
        );
        
        match timeout.await {
            Ok(Ok(_)) => Ok(output),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(anyhow!("Execution timeout")),
        }
    }
    
    pub async fn close(mut self) -> Result<()> {
        self.process.kill().await?;
        Ok(())
    }
}
```

#### 3.2.2 改进调用方式

**问题**: ACP agents 通常不支持 stdin 输入

**解决**: 使用命令行参数传递任务

```rust
// src/acp/client.rs
impl ACPClient {
    pub async fn spawn_with_task(
        agent_id: String,
        command: String,
        task: String,
        cwd: Option<PathBuf>,
        env: HashMap<String, String>,
    ) -> Result<Self> {
        let mut cmd = Command::new(&command);
        
        // 使用 exec 模式 + 任务参数
        cmd.arg("exec").arg(task);
        
        // ... 其他配置 ...
        
        let mut process = cmd.spawn()?;
        
        // 不需要 stdin，只读取 stdout
        let stdout = BufReader::new(
            process.stdout.take()
                .ok_or_else(|| anyhow!("Failed to get stdout"))?
        );
        
        Ok(Self {
            agent_id,
            process,
            stdin: None, // 不使用 stdin
            stdout,
        })
    }
}
```

#### 3.2.3 改进 ACPTool

```rust
// src/tools/acp.rs
impl Tool for ACPTool {
    async fn execute(&self, args: &str, _context: &ToolContext) -> Result<String> {
        let req: ACPExecuteRequest = serde_json::from_str(args)
            .map_err(|e| NanobotError::invalid_tool_args(self.name(), format!("Failed to parse arguments: {}", e)))?;
        
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
        
        // 创建 ACP Client（带任务）
        let mut client = ACPClient::spawn_with_task(
            req.agent_id.clone(),
            agent_config.command.clone(),
            req.task.clone(),
            req.cwd.map(|s| s.into()),
            agent_config.env.clone(),
        ).await
        .map_err(|e| NanobotError::tool_execution(self.name(), e))?;
        
        // 读取输出
        let result = client.read_output().await
            .map_err(|e| NanobotError::tool_execution(self.name(), e))?;
        
        // 等待进程结束
        client.wait().await
            .map_err(|e| NanobotError::tool_execution(self.name(), e))?;
        
        Ok(result)
    }
}
```

### 3.3 P2: 增强功能（可选）

#### 3.3.1 会话管理

```rust
// src/acp/session.rs
use dashmap::DashMap;

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
    pub async fn get_or_create_session(
        &self,
        agent_id: &str,
        cwd: Option<PathBuf>,
    ) -> Result<String> {
        // 查找现有会话
        for entry in self.sessions.iter() {
            if entry.value().agent_id == agent_id {
                return Ok(entry.key().clone());
            }
        }
        
        // 创建新会话
        self.create_session(agent_id, cwd).await
    }
}
```

#### 3.3.2 流式输出

```rust
// src/acp/client.rs
use tokio_stream::{Stream, StreamExt};

impl ACPClient {
    pub async fn execute_stream(&mut self, task: &str) -> Result<impl Stream<Item = String>> {
        let (tx, rx) = tokio::sync::mpsc::channel(100);
        
        // 后台任务读取输出
        tokio::spawn(async move {
            let mut line = String::new();
            while self.stdout.read_line(&mut line).await.unwrap_or(0) > 0 {
                if tx.send(line.clone()).await.is_err() {
                    break;
                }
                line.clear();
            }
        });
        
        Ok(tokio_stream::wrappers::ReceiverStream::new(rx))
    }
}
```

---

## 4. 实施步骤

### Step 1: 系统集成（30 分钟）

```bash
# 1. 修改 Config
# 2. 修改 ToolRegistryBuilder
# 3. 添加配置示例
# 4. 测试编译
cargo check
```

### Step 2: 改进 ACPClient（1 小时）

```bash
# 1. 实现 spawn_with_task
# 2. 实现 read_output
# 3. 添加超时控制
# 4. 测试
cargo test --lib acp
```

### Step 3: 集成测试（30 分钟）

```bash
# 1. 配置 codex
# 2. 手动测试
nanobot-rs agent -m "用 acp_execute 工具让 codex 创建一个 hello world"

# 3. 验证输出
```

---

## 5. 测试计划

### 5.1 单元测试

```rust
#[tokio::test]
async fn test_acp_client_spawn_with_task() {
    // Mock test
}

#[tokio::test]
async fn test_acp_tool_integration() {
    let config = ACPConfig::default();
    let tool = ACPTool::new(config);
    
    // 验证工具已注册
    assert_eq!(tool.name(), "acp_execute");
}
```

### 5.2 集成测试

```bash
# 前提：已安装 codex
which codex

# 测试 1: 简单任务
nanobot-rs agent -m "用 codex 创建一个 hello.txt 文件，内容是 'Hello World'"

# 测试 2: 复杂任务
nanobot-rs agent -m "用 codex 创建一个 Rust 程序，打印 Fibonacci 数列"
```

---

## 6. 风险和限制

### 6.1 技术限制

1. **不是标准 ACP 协议**
   - 使用命令行参数而不是 stdio
   - 可能与某些 agent 不兼容

2. **无法获取中间状态**
   - 只能读取最终输出
   - 看不到 thinking、tool calls

3. **会话管理简单**
   - 无法真正复用会话
   - 每次都是新进程

### 6.2 兼容性

**支持的 agents**:
- ✅ codex (支持 `codex exec "task"`)
- ✅ claude (支持 `claude exec "task"`)
- ⚠️ pi (需要验证)
- ⚠️ gemini (需要验证)
- ⚠️ opencode (需要验证)

---

## 7. 未来改进

### 7.1 等待官方 SDK

当网络恢复后：
1. 添加 `agent-client-protocol` 依赖
2. 使用官方 SDK 替换手动实现
3. 实现完整的 ACP 协议
4. 支持所有标准特性

### 7.2 完整协议实现

- JSON-RPC 2.0 over stdio
- 流式事件传输
- 审批请求处理
- 会话持久化

---

## 8. 总结

### 8.1 改进重点

**P0 - 系统集成**：
- ✅ 添加到 Config
- ✅ 注册到 ToolRegistry
- ✅ 用户可以使用

**P1 - 基本功能**：
- ✅ 真正启动进程
- ✅ 读取输出
- ✅ 错误处理

**P2 - 增强功能**：
- ⏳ 会话管理（可选）
- ⏳ 流式输出（可选）

### 8.2 预期效果

**改进前（MVP）**：
- 只返回占位符文本
- 无法实际使用

**改进后（Phase 2）**：
- 真正调用 ACP agent
- 返回实际结果
- 用户可以使用

---

**状态**: ✅ 计划完成
**预计时间**: 2 小时
**难度**: 中等
