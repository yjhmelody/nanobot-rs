# ACP Tool 集成实施计划

**文档版本**: 1.0  
**创建日期**: 2026-03-07  
**目标**: 将 ACP 作为 Tool 集成到 nanobot-rs

---

## 1. 实施概览

### 1.1 目标

将 ACP Agent（codex, claude, pi, gemini, opencode）作为工具集成到 nanobot-rs，让 Agent 可以委托复杂编码任务给专业的 coding agent。

### 1.2 架构定位

```
nanobot-rs Agent (决策层)
  ├── LLM Provider (推理)
  └── Tools (能力)
      ├── read_file
      ├── write_file
      └── acp_execute ⭐ (新增)
          ↓
      ACP Agent (执行层)
```

### 1.3 实施范围

**Phase 1: MVP（本次实施）**
- ✅ 添加依赖
- ✅ 实现 ACP Client（基于官方 SDK）
- ✅ 实现 ACPTool
- ✅ 支持单个 agent (codex)
- ✅ 基本配置

**Phase 2-4: 后续**
- 会话管理
- 多 Agent 支持
- 高级特性

---

## 2. 依赖添加

### 2.1 Cargo.toml

```toml
[dependencies]
# 现有依赖...

# ACP 集成
agent-client-protocol = "0.1"  # ACP 官方 SDK
dashmap = "6.0"                # 会话管理（后续使用）
```

### 2.2 验证依赖

```bash
cd ~/code/yjhmelody/nanobot-rs
cargo add agent-client-protocol
cargo add dashmap
cargo check
```

---

## 3. 模块结构

### 3.1 目录结构

```
src/
├── acp/
│   ├── mod.rs          # 模块导出
│   ├── client.rs       # ACP Client 实现
│   ├── config.rs       # 配置定义
│   └── error.rs        # 错误类型
├── tools/
│   ├── acp.rs          # ACPTool 实现 ⭐
│   └── ...
└── lib.rs              # 添加 acp 模块
```

### 3.2 创建模块

```bash
mkdir -p src/acp
touch src/acp/mod.rs
touch src/acp/client.rs
touch src/acp/config.rs
touch src/acp/error.rs
touch src/tools/acp.rs
```

---

## 4. 核心实现

### 4.1 ACP 配置

```rust
// src/acp/config.rs
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ACPConfig {
    pub enabled: bool,
    pub default_agent: String,
    pub allowed_agents: Vec<String>,
    pub agents: HashMap<String, AgentConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub command: String,
    pub env: HashMap<String, String>,
}

impl Default for ACPConfig {
    fn default() -> Self {
        let mut agents = HashMap::new();
        
        agents.insert("codex".to_string(), AgentConfig {
            command: "codex".to_string(),
            env: HashMap::new(),
        });
        
        Self {
            enabled: true,
            default_agent: "codex".to_string(),
            allowed_agents: vec!["codex".to_string()],
            agents,
        }
    }
}
```

### 4.2 ACP Client

```rust
// src/acp/client.rs
use anyhow::{Result, Context};
use tokio::process::{Command, Child};
use std::process::Stdio;
use std::path::PathBuf;

pub struct ACPClient {
    agent_id: String,
    process: Child,
    // 暂时简化，后续使用官方 SDK
}

impl ACPClient {
    pub async fn spawn(
        agent_id: String,
        command: String,
        cwd: Option<PathBuf>,
        env: std::collections::HashMap<String, String>,
    ) -> Result<Self> {
        let mut cmd = Command::new(&command);
        
        if let Some(cwd) = cwd {
            cmd.current_dir(cwd);
        }
        
        for (key, value) in env {
            cmd.env(key, value);
        }
        
        cmd.stdin(Stdio::piped())
           .stdout(Stdio::piped())
           .stderr(Stdio::piped());
        
        let process = cmd.spawn()
            .context(format!("Failed to spawn ACP agent: {}", agent_id))?;
        
        Ok(Self {
            agent_id,
            process,
        })
    }
    
    pub async fn execute(&mut self, task: &str) -> Result<String> {
        // MVP: 简化实现，直接返回
        // TODO: 使用官方 SDK 实现完整的 ACP 协议
        Ok(format!("ACP agent {} would execute: {}", self.agent_id, task))
    }
    
    pub async fn close(mut self) -> Result<()> {
        self.process.kill().await?;
        Ok(())
    }
}
```

### 4.3 ACP Tool

```rust
// src/tools/acp.rs
use async_trait::async_trait;
use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::acp::client::ACPClient;
use crate::acp::config::ACPConfig;
use crate::tools::base::{Tool, ToolContext};

#[derive(Debug, Deserialize)]
struct ACPExecuteRequest {
    agent_id: String,
    task: String,
    cwd: Option<String>,
}

pub struct ACPTool {
    config: ACPConfig,
}

impl ACPTool {
    pub fn new(config: ACPConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Tool for ACPTool {
    fn name(&self) -> &str {
        "acp_execute"
    }
    
    fn description(&self) -> &str {
        "Execute a coding task using an ACP agent (codex, claude, pi, gemini, opencode). \
         Use this for complex coding tasks that require multiple file operations, \
         code generation, or refactoring."
    }
    
    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "agent_id": {
                    "type": "string",
                    "enum": ["codex", "claude", "pi", "gemini", "opencode"],
                    "description": "The ACP agent to use for the task"
                },
                "task": {
                    "type": "string",
                    "description": "The coding task to execute. Be specific and clear."
                },
                "cwd": {
                    "type": "string",
                    "description": "Working directory for the task (optional)"
                }
            },
            "required": ["agent_id", "task"]
        })
    }
    
    async fn execute(&self, args: &str, _context: &ToolContext) -> Result<String> {
        let req: ACPExecuteRequest = serde_json::from_str(args)
            .context("Failed to parse acp_execute arguments")?;
        
        // 验证 agent_id
        if !self.config.allowed_agents.contains(&req.agent_id) {
            return Err(anyhow!(
                "Agent '{}' is not allowed. Allowed agents: {:?}",
                req.agent_id,
                self.config.allowed_agents
            ));
        }
        
        // 获取 agent 配置
        let agent_config = self.config.agents.get(&req.agent_id)
            .ok_or_else(|| anyhow!("Agent '{}' not configured", req.agent_id))?;
        
        // 创建 ACP Client
        let mut client = ACPClient::spawn(
            req.agent_id.clone(),
            agent_config.command.clone(),
            req.cwd.map(|s| s.into()),
            agent_config.env.clone(),
        ).await?;
        
        // 执行任务
        let result = client.execute(&req.task).await?;
        
        // 关闭 client
        client.close().await?;
        
        Ok(result)
    }
}
```

### 4.4 模块导出

```rust
// src/acp/mod.rs
pub mod client;
pub mod config;
pub mod error;

pub use client::ACPClient;
pub use config::{ACPConfig, AgentConfig};
```

```rust
// src/lib.rs
// 添加到现有模块列表
pub mod acp;
```

---

## 5. 工具注册

### 5.1 修改 ToolRegistry

```rust
// src/tools/registry.rs
use crate::acp::config::ACPConfig;
use crate::tools::acp::ACPTool;

impl ToolRegistry {
    pub fn new_with_config(config: &Config) -> Self {
        let mut registry = Self::new();
        
        // 注册现有工具
        registry.register(Box::new(ReadFileTool::new()));
        registry.register(Box::new(WriteFileTool::new()));
        // ...
        
        // 注册 ACP 工具
        if let Some(acp_config) = &config.acp {
            if acp_config.enabled {
                registry.register(Box::new(ACPTool::new(acp_config.clone())));
            }
        }
        
        registry
    }
}
```

---

## 6. 配置集成

### 6.1 添加到 Config

```rust
// src/config/schema.rs
use crate::acp::config::ACPConfig;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    // 现有字段...
    
    #[serde(default)]
    pub acp: Option<ACPConfig>,
}
```

### 6.2 配置示例

```toml
# config.toml

[acp]
enabled = true
default_agent = "codex"
allowed_agents = ["codex"]

[acp.agents.codex]
command = "codex"

[acp.agents.codex.env]
OPENAI_API_KEY = "${OPENAI_API_KEY}"
```

---

## 7. 测试

### 7.1 单元测试

```rust
// src/acp/client.rs
#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_acp_client_spawn() {
        // Mock test
        // TODO: 实现完整测试
    }
}
```

```rust
// src/tools/acp.rs
#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_acp_tool_parameters() {
        let config = ACPConfig::default();
        let tool = ACPTool::new(config);
        
        assert_eq!(tool.name(), "acp_execute");
        
        let params = tool.parameters();
        assert!(params["properties"]["agent_id"].is_object());
        assert!(params["properties"]["task"].is_object());
    }
}
```

### 7.2 集成测试

```bash
# 手动测试
nanobot-rs agent -m "用 acp_execute 工具让 codex 创建一个 hello world 程序"
```

---

## 8. 实施步骤

### Step 1: 添加依赖（5 分钟）
```bash
cd ~/code/yjhmelody/nanobot-rs
cargo add agent-client-protocol
cargo add dashmap
cargo check
```

### Step 2: 创建模块结构（5 分钟）
```bash
mkdir -p src/acp
touch src/acp/{mod.rs,client.rs,config.rs,error.rs}
touch src/tools/acp.rs
```

### Step 3: 实现核心代码（30 分钟）
- 实现 ACPConfig
- 实现 ACPClient（简化版）
- 实现 ACPTool

### Step 4: 集成到系统（15 分钟）
- 修改 Config
- 修改 ToolRegistry
- 添加配置示例

### Step 5: 测试（10 分钟）
- 编译检查
- 单元测试
- 手动测试

**总计**: 约 1 小时

---

## 9. MVP 限制

**当前 MVP 的限制**：
1. ❌ 未使用官方 SDK（简化实现）
2. ❌ 未实现完整的 ACP 协议
3. ❌ 未实现会话管理
4. ❌ 只支持单个 agent (codex)
5. ❌ 未实现流式输出

**后续改进**：
- Phase 2: 使用官方 SDK 实现完整协议
- Phase 3: 实现会话管理
- Phase 4: 支持多 Agent

---

## 10. 使用示例

### 10.1 从 Agent 调用

```
用户: "用 Codex 创建一个 Rust HTTP 服务器"

nanobot-rs Agent:
  ↓ LLM Provider 推理
  ↓ 决定使用 acp_execute 工具
  ↓ 调用: acp_execute({
      "agent_id": "codex",
      "task": "Create a Rust HTTP server using Axum"
    })
  ↓ ACP Client 执行
  ↓ 返回结果
```

### 10.2 配置

```toml
[acp]
enabled = true
default_agent = "codex"
allowed_agents = ["codex"]

[acp.agents.codex]
command = "codex"

[acp.agents.codex.env]
OPENAI_API_KEY = "${OPENAI_API_KEY}"
```

---

## 11. 下一步

**完成 MVP 后**：
1. 使用官方 SDK 替换简化实现
2. 实现完整的 ACP 协议通信
3. 添加会话管理
4. 支持更多 Agent
5. 添加流式输出
6. 完善错误处理

---

**状态**: ✅ 计划完成
**预计时间**: 1 小时
**难度**: 低（MVP 简化实现）
