# ACP 工具集成 - 简化方案

**文档版本**: 1.0  
**创建日期**: 2026-03-08  
**目标**: 通过配置文件手动启用 ACP 工具

---

## 1. 问题分析

### 1.1 当前架构

ToolRegistry 在多个地方创建：
- `src/agent/builder.rs` - Agent 构建器
- `src/agent/subagent.rs` - Subagent 管理器

每个地方都直接调用 `ToolRegistry::new()`，不使用 ToolRegistryBuilder。

### 1.2 集成难点

要集成 ACP 工具，需要：
1. 修改 ToolRegistry::new 签名（添加 acp_config 参数）
2. 修改所有调用点
3. 传递 Config 到所有地方

**影响范围大，风险高**

---

## 2. 简化方案

### 2.1 核心思路

**不修改 ToolRegistry::new**，而是：
1. 在 Config 中添加 acp 字段 ✅（已完成）
2. 在 Agent 启动后，动态注册 ACP 工具
3. 通过 `register_dynamic_tool()` 添加

### 2.2 实施位置

在 `src/agent/builder.rs` 的 `build()` 方法中：

```rust
// 创建 ToolRegistry
let tools = Arc::new(ToolRegistry::new(...));

// 动态注册 ACP 工具
if let Some(acp_config) = &config.acp {
    if acp_config.enabled {
        let acp_tool = Arc::new(ACPTool::new(acp_config.clone()));
        tools.register_dynamic_tool(acp_tool)?;
    }
}
```

---

## 3. 实施步骤

### Step 1: 修改 agent/builder.rs

```rust
// src/agent/builder.rs
use crate::tools::acp::ACPTool;

impl AgentBuilder {
    pub fn build(self) -> Result<Agent> {
        // ... 现有代码 ...
        
        let tools = Arc::new(ToolRegistry::new(
            workspace.clone(),
            config.tools.restrict_to_workspace,
            config.tools.exec.clone(),
            config.tools.web.clone(),
            Some(bus.clone()),
            None, // spawn_service 稍后设置
            Some(cron_service.clone()),
        ));
        
        // 动态注册 ACP 工具
        if let Some(acp_config) = &config.acp {
            if acp_config.enabled {
                let acp_tool = Arc::new(ACPTool::new(acp_config.clone()));
                tools.register_dynamic_tool(acp_tool)
                    .context("Failed to register ACP tool")?;
            }
        }
        
        // ... 其余代码 ...
    }
}
```

### Step 2: 测试

```bash
cargo check
cargo test --lib acp
```

### Step 3: 配置示例

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

---

## 4. 优势

### 4.1 最小侵入

- ✅ 不修改 ToolRegistry::new 签名
- ✅ 不修改现有调用点
- ✅ 只在一个地方添加代码

### 4.2 向后兼容

- ✅ 如果没有配置 acp，不影响现有功能
- ✅ 如果 acp.enabled = false，不注册工具

### 4.3 易于测试

- ✅ 可以单独测试 ACP 工具
- ✅ 不影响其他工具的测试

---

## 5. 完整实施

### 5.1 查找注册点

```bash
grep -r "ToolRegistry::new" src/agent/
```

结果：
- `src/agent/builder.rs` - 主要 Agent
- `src/agent/subagent.rs` - Subagent（2 处）

### 5.2 修改所有注册点

**位置 1: agent/builder.rs**
```rust
// 在 ToolRegistry::new 之后添加
if let Some(acp_config) = &config.acp {
    if acp_config.enabled {
        tools.register_dynamic_tool(Arc::new(ACPTool::new(acp_config.clone())))?;
    }
}
```

**位置 2: agent/subagent.rs (第一处)**
```rust
// 同样的代码
```

**位置 3: agent/subagent.rs (第二处)**
```rust
// 同样的代码
```

---

## 6. 测试计划

### 6.1 单元测试

```rust
#[test]
fn test_acp_tool_registration() {
    let mut config = Config::default();
    config.acp = Some(ACPConfig::default());
    
    // 构建 Agent
    let agent = AgentBuilder::new(config).build().unwrap();
    
    // 验证 ACP 工具已注册
    let defs = agent.tools.definitions();
    assert!(defs.iter().any(|d| d.function.name == "acp_execute"));
}
```

### 6.2 集成测试

```bash
# 1. 配置 ACP
cat > config.toml << EOF
[acp]
enabled = true
defaultAgent = "codex"
allowedAgents = ["codex"]

[acp.agents.codex]
command = "echo"
EOF

# 2. 启动 Agent
nanobot-rs agent -m "list available tools"

# 3. 验证 acp_execute 在工具列表中
```

---

## 7. 时间估算

| 任务 | 时间 |
|------|------|
| 修改 agent/builder.rs | 10 分钟 |
| 修改 agent/subagent.rs | 10 分钟 |
| 测试编译 | 5 分钟 |
| 单元测试 | 10 分钟 |
| 集成测试 | 10 分钟 |
| **总计** | **45 分钟** |

---

## 8. 总结

### 8.1 方案对比

| 方案 | 优势 | 劣势 |
|------|------|------|
| 修改 ToolRegistry::new | 统一管理 | 影响范围大 |
| 动态注册 | 最小侵入 | 需要在多处添加 |

**选择**: 动态注册（最小侵入）

### 8.2 下一步

1. ✅ Config 已添加 acp 字段
2. ⏳ 在 agent/builder.rs 中动态注册
3. ⏳ 在 agent/subagent.rs 中动态注册
4. ⏳ 测试验证

---

**状态**: ✅ 方案确定
**预计时间**: 45 分钟
**风险**: 低
