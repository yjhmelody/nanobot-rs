# 快速开始

本文面向“想把 `nanobot-rs` 跑起来并开始使用”的用户。

## 前置条件

- Rust 工具链
- 一个可用的 LLM 凭证：
  - Anthropic API Key
  - OpenAI API Key
  - 或兼容 `OpenAI Responses API` 的自定义服务

## 安装与构建

```bash
git clone <your-repo-url>
cd nanobot-rs
cargo build --release
```

二进制位于 `target/release/nanobot-rs`。

开发阶段也可以直接使用：

```bash
cargo run -- <subcommand>
```

## 初始化

首次运行建议先执行：

```bash
nanobot-rs onboard
```

它会完成两件事：

- 创建或刷新 `~/.nanobot/config.json`
- 初始化工作区 `~/.nanobot/workspace` 及模板文件

## 最小配置

默认配置已经包含推荐参数，你通常只需要补上 provider 凭证。

### Anthropic

```json
{
  "agents": {
    "defaults": {
      "workspace": "~/.nanobot/workspace",
      "model": "anthropic/claude-sonnet-4-5",
      "provider": "auto",
      "maxTokens": 8192,
      "temperature": 0.1,
      "maxToolIterations": 40,
      "memoryWindow": 100,
      "reasoningEffort": null
    }
  },
  "providers": {
    "anthropic": {
      "apiKey": "sk-ant-..."
    }
  }
}
```

### OpenAI

```json
{
  "agents": {
    "defaults": {
      "model": "openai/gpt-4.1"
    }
  },
  "providers": {
    "openai": {
      "apiKey": "sk-..."
    }
  }
}
```

### Custom（Responses-compatible）

```json
{
  "agents": {
    "defaults": {
      "provider": "custom",
      "model": "your-model"
    }
  },
  "providers": {
    "custom": {
      "apiBase": "https://your-endpoint.example.com/v1",
      "apiKey": "token"
    }
  }
}
```

`custom` provider 现在要求目标服务兼容 **OpenAI Responses API**，不再兼容旧的 `chat/completions` 接口。

## Provider 支持说明

| Provider | 用途 | 说明 |
|---|---|---|
| `anthropic` | 主 LLM provider | 使用原生 `Messages API` |
| `openai` | 主 LLM provider | 使用原生 `Responses API` |
| `custom` | 主 LLM provider | 要求兼容 `Responses API` |
| `github_copilot` | 辅助命令 / ACP | 不作为主 LLM provider 注入 AgentLoop |

GitHub Copilot 当前支持：

```bash
nanobot-rs provider login github_copilot
nanobot-rs provider status github_copilot
```

如果要把 Copilot / Claude / Codex 当作外部 coding agent 使用，请通过 ACP 配置接入，而不是把它们设置成主 provider。

## 常用命令

### 单轮执行

```bash
nanobot-rs agent -m "总结一下当前目录结构"
```

### 交互模式

```bash
nanobot-rs agent
```

### 指定会话

```bash
nanobot-rs agent -s cli:project-a -m "继续上次任务"
```

### 查看状态

```bash
nanobot-rs status
```

### 启动 Gateway

```bash
nanobot-rs gateway
```

## 内建工具

当前内建工具包括：

- 文件系统：`read_file` `write_file` `edit_file` `list_dir`
- Shell：`exec`
- Web：`web_search` `web_fetch`
- 消息：`message`
- 可选：`spawn` `cron`

此外还支持：

- `MCP`：通过 `tools.mcpServers` 接入
- `ACP`：通过外部 coding agent 执行复杂任务

## 渠道支持

当前渠道状态：

- `cli`：可用
- `telegram`：可用
- `discord`：占位实现
- `feishu`：占位实现

启用 Telegram 时至少需要：

```json
{
  "channels": {
    "telegram": {
      "enabled": true,
      "allowFrom": ["*"],
      "token": "<telegram-bot-token>"
    }
  }
}
```

Telegram 额外字段（可选）：

- `apiBase`：自定义 Telegram API 地址（默认 `https://api.telegram.org`）。
- `receiveAck`：是否在收到消息后发送 `sendChatAction` 的 `typing` 作为“已收到”提示。`typing` 状态会持续约 5 秒或更短，并会在机器人发送消息后被清除；且在频道聊天与频道私聊中不支持，仅建议在响应需要较长时间时开启。

## 工作区与数据

默认工作区：

```text
~/.nanobot/workspace
```

常见内容：

- `sessions/`：会话历史
- `memory/`：长期记忆
- `AGENTS.md` `TOOLS.md` `USER.md` 等模板文件

## 常见问题

### 1. 为什么自定义 provider 返回 404？

请确认你的服务支持 **`/responses`**，而不是只支持旧的 `chat/completions`。

### 2. 为什么设置了 `github_copilot` 作为主 provider 后无法启动？

`github_copilot` 当前不作为主 LLM provider 使用，只支持登录/状态检查与 ACP 场景。

### 3. 为什么启用渠道后没有消息？

请检查：

- 渠道是否 `enabled`
- `allowFrom` 是否已配置
- Telegram token / 相关额外字段是否正确

## 下一步

- 了解系统结构：`docs/ARCHITECTURE.md`
- 查看开发与测试流程：`docs/DEVELOPMENT.md`
