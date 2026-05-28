# 快速开始

本文面向“想把 `nanobot` 跑起来并开始使用”的用户。

## 前置条件

- Rust 工具链
- 一个可用的 LLM 凭证：
  - Anthropic API Key
  - OpenAI API Key
  - 或兼容 `OpenAI Responses API` 的自定义服务

## 安装与构建

```bash
git clone <your-repo-url>
cd nanobot
cargo build --release
```

二进制位于 `target/release/nanobot`。

如果你不想从源码构建，也可以直接从 GitHub Releases 下载对应平台的发行包：

- Linux：`x86_64-unknown-linux-gnu`
- macOS：`x86_64-apple-darwin`
- Windows：`x86_64-pc-windows-msvc`

开发阶段也可以直接使用：

```bash
cargo run -- <subcommand>
```

## 初始化

首次运行建议先执行：

```bash
nanobot onboard
```

它会完成两件事：

- 创建或刷新 `~/.nanobot/config.json`
- 初始化工作区 `~/.nanobot/workspace` 及模板文件

## 最小配置

默认配置已经包含推荐参数，你通常只需要补上 provider 凭证。

### 长会话 / 复杂任务推荐配置

如果你希望多轮复杂任务时更不容易“失忆”，建议从默认值提升到下面这一档：

```json
{
  "agents": {
    "defaults": {
      "maxTokens": 16384,
      "maxToolIterations": 80,
      "memoryWindow": 400,
      "consolidationEnabled": true,
      "consolidationKeepRecent": 80,
      "consolidationMinMessages": 120,
      "consolidationSummaryMaxTokens": 4000
    }
  }
}
```

如果你主要在某个通道（如 `lark`）处理长任务，优先使用 `channels.<channel>.agentOverrides` 做局部放大。

### Anthropic

```json
{
  "agents": {
    "defaults": {
      "workspace": "~/.nanobot/workspace",
      "model": "anthropic/claude-sonnet-4-5",
      "provider": "anthropic",
      "maxTokens": 8192,
      "temperature": 0.1,
      "maxToolIterations": 40,
      "memoryWindow": 100,
      "consolidationEnabled": true,
      "consolidationKeepRecent": 10,
      "consolidationMinMessages": 20,
      "consolidationSummaryMaxTokens": 1000,
      "reasoningEffort": null
    }
  },
  "providers": {
    "anthropic": {
      "providerType": "anthropic",
      "model": "claude-sonnet-4-5",
      "apiKey": "...",
      "apiBase": "...",
      "extraHeaders": null
    },
    "openai": {
      "providerType": "openai",
      "model": "gpt-5.4",
      "apiKey": "..",
      "apiBase": "..",
      "extraHeaders": null
    },
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

### Custom（OpenAI-compatible）

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
      "wireApi": "responses",
      "apiBase": "https://your-endpoint.example.com/v1",
      "apiKey": "token"
    }
  }
}
```

如果你的网关只支持旧接口，可改成：

```json
{
  "providers": {
    "custom": {
      "wireApi": "chat_completions"
    }
  }
}
```

### DeepSeek（OpenAI-compatible）

```json
{
  "agents": {
    "defaults": {
      "provider": "deepseek"
    }
  },
  "providers": {
    "deepseek": {
      "providerType": "openai",
      "wireApi": "chat_completions",
      "model": "deepseek-chat",
      "apiBase": "https://api.deepseek.com/v1",
      "apiKey": "{{DEEPSEEK_API_KEY}}"
    }
  }
}
```

### DeepSeek（Anthropic-compatible）

```json
{
  "agents": {
    "defaults": {
      "provider": "deepseek_anthropic"
    }
  },
  "providers": {
    "deepseek_anthropic": {
      "providerType": "anthropic",
      "model": "deepseek-v4-flash",
      "apiBase": "https://api.deepseek.com/anthropic",
      "apiKey": "{{ANTHROPIC_AUTH_TOKEN}}",
      "extraHeaders": {
        "anthropic-version": "2023-06-01"
      }
    }
  }
}
```

## Provider 支持说明

| Provider | 用途 | 说明 |
|---|---|---|
| `anthropic` | 主 LLM provider | 使用原生 `Messages API` |
| `openai` | 主 LLM provider | 默认使用 `Responses API`，可用 `wireApi` 切到 `chat/completions` |
| `custom` | 主 LLM provider | 默认使用 `Responses API`，可用 `wireApi` 切到 `chat/completions` |
| `github_copilot` | 辅助命令 / ACP | 不作为主 LLM provider 注入 AgentLoop |

GitHub Copilot 当前支持：

```bash
nanobot provider login github_copilot
nanobot provider status github_copilot
```

如果要把 Copilot / Claude / Codex 当作外部 coding agent 使用，请通过 ACP 配置接入，而不是把它们设置成主 provider。

## 常用命令

### 单轮执行

```bash
nanobot agent -m "总结一下当前目录结构"
```

### 交互模式

```bash
nanobot agent
```

### 指定会话

```bash
nanobot agent -s cli:project-a -m "继续上次任务"
```

### 查看状态

```bash
nanobot status
```

### 启动 Gateway

```bash
nanobot gateway
```

## 内建工具

当前运行时默认可用工具包括：

- 文件系统：`read_file` `write_file` `edit_file` `list_dir`
- Shell：`exec`
- Web：`web_search` `web_fetch`
- 代码检索：`search_files` `grep_code`
- 消息：`message`
- 任务：`spawn` `cron`

此外还支持：

- `MCP`：通过 `tools.mcpServers` 接入
- `ACP`：配置 `acp` 后会注入 `acp_execute` 工具，用于外部 coding agent 执行复杂任务

## 渠道支持

当前渠道状态：

- `cli`：可用
- `telegram`：可用
- `discord`：占位实现
- `feishu/lark`：可用（应用 API 对话 + 事件订阅入站；兼容 webhook 出站）

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

启用 Feishu/Lark（应用模式，建议用于对话）至少需要：

```json
{
  "channels": {
    "lark": {
      "enabled": true,
      "allowFrom": ["*"],
      "appId": "<feishu-app-id>",
      "appSecret": "<feishu-app-secret>",
      "verifyToken": "<event-subscription-token>"
    }
  }
}
```

Feishu/Lark 额外字段（可选）：

- `webhook` / `webhookUrl` / `url` / `botKey`：Webhook 出站通道（不按 `chat_id`）
- `secret` / `signSecret`：签名密钥
- `apiBase`：仅在使用 `botKey` 时生效，默认 `https://open.feishu.cn`
- `verifyToken`：事件订阅 token
- `callbackListen`：本地监听地址（默认 `0.0.0.0:19820`）
- `callbackPath`：回调路径（默认 `/feishu/events`）

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

请检查 `providers.<name>.wireApi` 是否与网关匹配：

- `responses` -> `.../responses`
- `chat_completions` -> `.../chat/completions`

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
