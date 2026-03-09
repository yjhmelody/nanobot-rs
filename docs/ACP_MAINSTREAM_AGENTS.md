# ACP 主流 Coding Agents 集成

**文档版本**: 1.0  
**创建日期**: 2026-03-08  
**目标**: 集成主流的 ACP coding agents

---

## 1. 主流 Coding Agents

### 1.1 支持 ACP 协议的 Agents

| Agent | 提供商 | 特点 | 命令 |
|-------|--------|------|------|
| **Codex** | OpenAI | 代码生成专家 | `codex` |
| **Claude Code** | Anthropic | 长上下文，推理能力强 | `claude` |
| **Cursor** | Cursor | IDE 集成，快速迭代 | `cursor` |
| **Windsurf** | Codeium | 多文件编辑 | `windsurf` |
| **Cline** | Cline | 开源，可定制 | `cline` |

### 1.2 选择标准

**必须支持**:
- ✅ ACP (Agent Client Protocol)
- ✅ stdio 通信
- ✅ 命令行调用

**优先考虑**:
- 活跃维护
- 社区支持
- 文档完善

---

## 2. 配置方案

### 2.1 默认配置

```rust
// src/acp/config.rs
impl Default for ACPConfig {
    fn default() -> Self {
        let mut agents = HashMap::new();
        
        // Codex (OpenAI)
        agents.insert("codex".to_string(), AgentConfig {
            command: "codex".to_string(),
            env: HashMap::new(),
        });
        
        // Claude Code (Anthropic)
        agents.insert("claude".to_string(), AgentConfig {
            command: "claude".to_string(),
            env: HashMap::new(),
        });
        
        // Cursor
        agents.insert("cursor".to_string(), AgentConfig {
            command: "cursor".to_string(),
            env: HashMap::new(),
        });
        
        // Windsurf (Codeium)
        agents.insert("windsurf".to_string(), AgentConfig {
            command: "windsurf".to_string(),
            env: HashMap::new(),
        });
        
        // Cline
        agents.insert("cline".to_string(), AgentConfig {
            command: "cline".to_string(),
            env: HashMap::new(),
        });
        
        Self {
            enabled: true,
            default_agent: "claude".to_string(), // Claude 作为默认
            allowed_agents: vec![
                "codex".to_string(),
                "claude".to_string(),
                "cursor".to_string(),
                "windsurf".to_string(),
                "cline".to_string(),
            ],
            agents,
        }
    }
}
```

### 2.2 用户配置示例

```toml
# config.toml

[acp]
enabled = true
defaultAgent = "claude"  # 默认使用 Claude
allowedAgents = ["codex", "claude", "cursor", "windsurf", "cline"]

# Codex (OpenAI)
[acp.agents.codex]
command = "codex"

[acp.agents.codex.env]
OPENAI_API_KEY = "${OPENAI_API_KEY}"

# Claude Code (Anthropic)
[acp.agents.claude]
command = "claude"

[acp.agents.claude.env]
ANTHROPIC_API_KEY = "${ANTHROPIC_API_KEY}"

# Cursor
[acp.agents.cursor]
command = "cursor"

[acp.agents.cursor.env]
CURSOR_API_KEY = "${CURSOR_API_KEY}"

# Windsurf (Codeium)
[acp.agents.windsurf]
command = "windsurf"

[acp.agents.windsurf.env]
CODEIUM_API_KEY = "${CODEIUM_API_KEY}"

# Cline (开源)
[acp.agents.cline]
command = "cline"
# Cline 可能不需要 API key
```

---

## 3. Agent 特性对比

### 3.1 功能对比

| 特性 | Codex | Claude | Cursor | Windsurf | Cline |
|------|-------|--------|--------|----------|-------|
| 代码生成 | ⭐⭐⭐⭐⭐ | ⭐⭐⭐⭐⭐ | ⭐⭐⭐⭐ | ⭐⭐⭐⭐ | ⭐⭐⭐ |
| 长上下文 | ⭐⭐⭐ | ⭐⭐⭐⭐⭐ | ⭐⭐⭐ | ⭐⭐⭐⭐ | ⭐⭐⭐ |
| 推理能力 | ⭐⭐⭐⭐ | ⭐⭐⭐⭐⭐ | ⭐⭐⭐ | ⭐⭐⭐ | ⭐⭐⭐ |
| 多文件编辑 | ⭐⭐⭐ | ⭐⭐⭐⭐ | ⭐⭐⭐⭐⭐ | ⭐⭐⭐⭐⭐ | ⭐⭐⭐ |
| 速度 | ⭐⭐⭐⭐ | ⭐⭐⭐ | ⭐⭐⭐⭐⭐ | ⭐⭐⭐⭐ | ⭐⭐⭐ |
| 开源 | ❌ | ❌ | ❌ | ❌ | ✅ |

### 3.2 使用场景

**Codex**:
- 快速代码生成
- 算法实现
- 代码补全

**Claude Code**:
- 复杂项目重构
- 架构设计
- 长文档处理

**Cursor**:
- 快速迭代开发
- IDE 内编辑
- 实时协作

**Windsurf**:
- 大规模代码修改
- 多文件重构
- 批量处理

**Cline**:
- 自定义工作流
- 本地部署
- 隐私敏感项目

---

## 4. 工具定义更新

### 4.1 更新 Tool Definition

```rust
// src/tools/acp.rs
impl Tool for ACPTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::function(
            "acp_execute",
            "Delegate complex coding tasks to specialized ACP agents (Codex, Claude, Cursor, Windsurf, Cline)",
            JsonSchema::object(
                BTreeMap::from([
                    (
                        "agent_id".to_string(),
                        JsonSchema::string_enum(vec![
                            "codex",
                            "claude",
                            "cursor",
                            "windsurf",
                            "cline",
                        ])
                        .with_description("The ACP agent to use"),
                    ),
                    (
                        "task".to_string(),
                        JsonSchema::string()
                            .with_description("The coding task to execute"),
                    ),
                    (
                        "cwd".to_string(),
                        JsonSchema::string()
                            .with_description("Working directory (optional)"),
                    ),
                ]),
                vec!["agent_id".to_string(), "task".to_string()],
            ),
        )
    }
}
```

---

## 5. 使用示例

### 5.1 使用不同的 Agents

```bash
# 使用 Claude（默认）
nanobot-rs agent -m "重构这个项目的架构"

# 使用 Codex（快速生成）
nanobot-rs agent -m "用 Codex 生成一个快速排序算法"

# 使用 Cursor（快速迭代）
nanobot-rs agent -m "用 Cursor 修复这个 bug"

# 使用 Windsurf（多文件）
nanobot-rs agent -m "用 Windsurf 重构所有 API 接口"

# 使用 Cline（开源）
nanobot-rs agent -m "用 Cline 实现这个功能"
```

### 5.2 Agent 自动选择

nanobot-rs 的 LLM Provider 会根据任务特点自动选择合适的 agent：

```
用户: "重构整个项目的错误处理"

nanobot-rs Agent:
  ↓ LLM 分析任务
  ↓ 判断：需要长上下文 + 复杂推理
  ↓ 选择：Claude Code
  ↓ 调用：acp_execute({
      "agent_id": "claude",
      "task": "Refactor error handling across the project"
    })
```

---

## 6. 安装指南

### 6.1 安装 Agents

```bash
# Codex (需要 OpenAI API key)
npm install -g @openai/codex-cli

# Claude Code (需要 Anthropic API key)
npm install -g @anthropic/claude-cli

# Cursor (需要 Cursor 账号)
# 从 https://cursor.sh 下载安装

# Windsurf (需要 Codeium 账号)
# 从 https://codeium.com/windsurf 下载安装

# Cline (开源)
npm install -g cline
```

### 6.2 配置 API Keys

```bash
# 设置环境变量
export OPENAI_API_KEY="sk-..."
export ANTHROPIC_API_KEY="sk-ant-..."
export CURSOR_API_KEY="..."
export CODEIUM_API_KEY="..."

# 或者在 config.toml 中配置
```

---

## 7. 实施步骤

### Step 1: 更新 ACPConfig Default

```bash
# 修改 src/acp/config.rs
# 添加所有主流 agents
```

### Step 2: 更新 Tool Definition

```bash
# 修改 src/tools/acp.rs
# 更新 agent_id enum
```

### Step 3: 更新文档

```bash
# 更新 README.md
# 添加 agents 安装指南
```

### Step 4: 测试

```bash
# 测试每个 agent
cargo test --lib acp
```

---

## 8. 总结

### 8.1 集成的 Agents

**主流 Agents**（5 个）:
1. ✅ Codex (OpenAI) - 代码生成专家
2. ✅ Claude Code (Anthropic) - 长上下文推理
3. ✅ Cursor - IDE 集成
4. ✅ Windsurf (Codeium) - 多文件编辑
5. ✅ Cline - 开源可定制

### 8.2 默认选择

**推荐默认**: Claude Code
- 长上下文（200K tokens）
- 强大的推理能力
- 适合复杂任务

### 8.3 下一步

1. 更新 src/acp/config.rs
2. 更新 src/tools/acp.rs
3. 添加安装文档
4. 测试验证

---

**状态**: ✅ 方案完成  
**预计时间**: 30 分钟  
**难度**: 简单
