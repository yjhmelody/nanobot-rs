# 配置参考

本文档基于当前源码实现，说明 `~/.nanobot/config.json` 的主要字段、默认值与行为边界。

## 1. 配置文件位置与加载规则

- 默认路径：`~/.nanobot/config.json`
- 加载函数：`nanobot_config::load_config`
- 若文件不存在或解析失败：回退到 `Config::default()`
- 支持环境变量替换：`{{ENV_VAR}}` 形式会在解析前替换
- 支持 JSONC 风格注释：`// ...` 与 `/* ... */`

示例：

```json
{
  "providers": {
    "anthropic": {
      "apiKey": "{{ANTHROPIC_API_KEY}}"
    }
  }
}
```

## 2. 顶层结构

```json
{
  "agents": {},
  "channels": {},
  "providers": {},
  "gateway": {},
  "tools": {},
  "acp": {}
}
```

其中 `acp` 为可选字段。

## 3. agents.defaults

关键字段（含默认值）：

- `workspace`: `~/.nanobot/workspace`
- `model`: `anthropic/claude-sonnet-4-5`
- `provider`: `auto`
- `fallbackProviders`: `null`
- `maxTokens`: `8192`
- `temperature`: `0.1`
- `maxToolIterations`: `40`
- `maxSubagentIterations`: `15` — subagent 单次任务的最大推理迭代次数
- `memoryWindow`: `100`
- `consolidationKeepRecent`: `10`
- `consolidationMinMessages`: `20`
- `consolidationSummaryMaxTokens`: `1000`
- `reasoningEffort`: `null` — 推理/思考配置。详见 [§4](#4-reasoningeffort-推理思考配置)
- `consolidationEnabled`: `true`

校验规则：

- `maxTokens > 0`
- `temperature` 在 `[0.0, 2.0]`
- `maxToolIterations > 0`
- `memoryWindow > 0`
- `consolidationKeepRecent > 0` 且 `consolidationKeepRecent <= memoryWindow`
- `consolidationMinMessages > 0`
- `consolidationSummaryMaxTokens > 0`
- `workspace` / `model` 非空

说明：

- `memoryWindow` 控制每次请求模型时带入的历史窗口大小。
- `consolidationEnabled` 控制是否在每次保存回合后自动执行 consolidation。
- `consolidationKeepRecent` 控制会话 consolidation 时保留为原始消息的最近条数。
- `consolidationMinMessages` 控制至少累积多少条“尚未 consolidation”的消息后才触发。
- `consolidationSummaryMaxTokens` 控制 consolidation 摘要请求可使用的最大 token。

### 3.1 长会话/复杂任务最佳实践

当你希望减少“对话进行到一半失忆”的情况，可以优先按下面思路调整：

- 增大 `memoryWindow`：每轮请求带更多历史。
- 增大 `consolidationKeepRecent`：压缩后保留更多原始最近消息。
- 增大 `consolidationMinMessages`：减少过早 consolidation。
- 增大 `consolidationSummaryMaxTokens`：让 consolidation 摘要更完整。
- 增大 `maxToolIterations`：允许复杂任务使用更多工具回合。

推荐起步参数（可按模型成本继续微调）：

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
  },
  "channels": {
    "instances": {
      "my_bot": {
        "channelType": "feishu",
        "appId": "cli_xxx",
        "appSecret": "yyy",
        "allowFrom": ["*"]
      }
    }
  }
}

## 4. reasoningEffort（推理/思考配置）

`reasoningEffort` 是 provider-agnostic 的推理/思考配置超集。

各 provider 按 `providerType` 只读自己关心的字段，无关字段静默忽略。

```jsonc
{
  "reasoningEffort": {
    // --- Anthropic 字段 ---
    "type": "adaptive",       // "adaptive" (Claude 4.6+) 或 "enabled" (4.0-4.5)
    "budgetTokens": 4096,     // type="enabled" 时的 token 预算

    // --- OpenAI / 兼容 provider 字段 ---
    "effort": "xhigh"         // "low" | "medium" | "high" | "xhigh"
  }
}
```

### 各 provider 映射关系

| Provider | 读取字段 | 序列化成 |
|----------|---------|----------|
| Anthropic | `type` + `budget_tokens` | `thinking: {type, budget_tokens?}` |
| OpenAI-Compatible | `effort` | `reasoning: {effort}` |

### 配置示例

```jsonc
// Anthropic Claude 4.6+ 自适应思考（模型自行决定是否思考）
{ "reasoningEffort": { "type": "adaptive" } }

// Anthropic Claude 3.7 / 4.0-4.5 固定思考预算
{ "reasoningEffort": { "type": "enabled", "budgetTokens": 4096 } }

// OpenAI Codex / o系列 深度思考
{ "reasoningEffort": { "effort": "xhigh" } }

// 不配置或为 null：不使用 reasoning/thinking
{ "reasoningEffort": null }
```

### 字段说明

| 字段 | 类型 | 说明 |
|------|------|------|
| `type` | `string` | 仅 Anthropic 使用。`"adaptive"`（Claude 4.6+）或 `"enabled"`（Claude 3.7/4.0-4.5）|
| `budgetTokens` | `number` | 仅 Anthropic `type="enabled"` 时生效。思考 token 预算，须小于 `maxTokens` |
| `effort` | `string` | 仅 OpenAI-compatible 使用。可选值：`low`, `medium`, `high`, `xhigh` |

## 5. providers

### 4.1 当前支持的“模型提供商”范围

当前实现分两层：

1. **Provider 协议类型（真正决定调用逻辑）**
   - `anthropic`（Anthropic Messages API）
   - `open_ai_compatible`（OpenAI 兼容协议，支持 `responses` / `chat_completions`）
   - `oauth`（仅 OAuth 类型，不作为主 LLM provider 注入）

2. **内置 provider 键（默认配置里自带）**
   - `anthropic`
   - `openai`
   - `custom`
   - `github_copilot`

`provider` 名称支持别名归一化（如 `github-copilot`、`githubCopilot` 会归一为 `github_copilot`）。

`providers` 现在是 **HashMap 结构**，可添加任意自定义 provider 键。  
每个 provider 配置都使用同一个 `ProviderConfig` 结构。

`ProviderConfig` 关键字段：

- `providerType`（可选）：
  - `open_ai_compatible`：按 OpenAI-compatible 接口处理（默认，兼容别名 `openai`）
  - `anthropic`：按 Anthropic Messages 接口处理
  - `oauth`：OAuth 类型（不作为主 LLM provider 注入，兼容旧值 `o_auth`）
- `model`（可选）：该 provider 的默认模型。配置后会优先于 `agents.defaults.model`
- `wireApi`（可选，仅 OpenAI-compatible）：
  - `responses`：使用 `/responses`（默认）
  - `chat_completions`：使用 `/chat/completions`（兼容别名 `chat` / `completions`）
- `apiKey`
- `apiBase`
- `extraHeaders`

`provider=auto` 时，会按模型名和已配置鉴权信息推断 provider。  
`fallbackProviders` 配置后，会按顺序构建 fallback 链。

DeepSeek（OpenAI-compatible）示例：

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

DeepSeek（Anthropic-compatible）示例：

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

注意：

- `github_copilot` 当前不作为主 LLM provider 注入运行时（应通过 ACP 工具使用）。
- `custom` 默认走 OpenAI-compatible `responses` 路径，可通过 `wireApi` 切换到 `chat/completions`；默认 `apiBase` 为 `http://localhost:8000/v1`（未配置时）。

## 5. channels

支持通过 `channels.instances` 配置多个通道实例，每个实例由 `channelType` 字段标识类型。公共默认值在 `channels.defaults` 中配置，各实例可覆盖。

### 5.1 配置结构

```json
{
  "channels": {
    "defaults": {
      "sendProgress": true,
      "sendToolHints": false,
      "sendUsageSummary": false,
      "streamMode": "updateAll"
    },
    "instances": {
      "<instance_name>": {
        "channelType": "<type>",
        "enabled": true,
        "allowFrom": ["*"]
      }
    }
  }
}
```

### 5.2 当前支持的通道类型

| 类型 | `channelType` 值 | 当前状态 |
|---|---|---|
| CLI | 无（内置） | 可用（本地终端） |
| Telegram | `"telegram"` | 可用 |
| Feishu/Lark | `"feishu"` / `"lark"` | 可用 |

### 5.3 `defaults` 字段说明

- `sendProgress`（默认 `true`）— 是否发送进度事件
- `sendToolHints`（默认 `false`）— 是否发送工具调用提示
- `sendUsageSummary`（默认 `false`）— 是否在回复末尾附加 token 用量
- `streamMode`（默认 `updateAll`，可选：`updateAll` / `updateProgress` / `append`）— 流式消息行为

### 5.4 实例通用字段

每个实例配置中以下字段可选，覆盖 `defaults`：

- `sendProgress`（可选 `boolean`）
- `sendToolHints`（可选 `boolean`）
- `sendUsageSummary`（可选 `boolean`）
- `streamMode`（可选 `string`）

### 5.5 `allowFrom` 约束

- 不能为空
- 不允许空字符串或首尾空白
- `*` 不能和显式 id 混用

`sender_id` 的取值优先级：
1. `user_id`（最稳定，租户内员工 ID，永久不变）
2. `union_id`（跨应用统一，同一开发商下不变）
3. `open_id`（按应用隔离，最后兜底）

Feishu 事件中不一定包含所有 ID 类型，按优先级取第一个存在的值。

### 5.6 Telegram 实例专有字段

| 字段 | 类型 | 必填 | 说明 |
|---|---|---|---|
| `token` | `string` | 是 | Bot token |
| `apiBase` | `string` | 否 | API 地址，默认 `https://api.telegram.org` |

配置示例：

```json
{
  "channels": {
    "instances": {
      "public_bot": {
        "channelType": "telegram",
        "token": "bot123:abc",
        "allowFrom": ["*"]
      },
      "admin_bot": {
        "channelType": "telegram",
        "token": "bot456:def",
        "allowFrom": ["admin_chat_id"],
        "sendProgress": false
      }
    }
  }
}
```

### 5.7 Feishu/Lark 实例专有字段

| 字段 | 类型 | 必填 | 说明 |
|---|---|---|---|
| `appId` | `string` | 是（应用 API 模式） | 飞书应用 ID |
| `appSecret` | `string` | 是（应用 API 模式） | 飞书应用 Secret |
| `webhookUrl` | `string` | 否 | webhook 地址或 bot key。如果以 `http` 开头则直接使用，否则拼接为 `{apiBase}/open-apis/bot/v2/hook/{key}` |
| `apiBase` | `string` | 否 | API 地址，默认 `https://open.feishu.cn` |
| `verifyToken` | `string` | 否 | 事件订阅 token |
| `secret` | `string` | 否 | Webhook 签名密钥 |
| `eventEnabled` | `boolean` | 否 | 是否启动事件监听。默认：有 `appId+appSecret` 且非 WS 模式时为 `true` |
| `wsEnabled` | `boolean` | 否 | 是否使用 WebSocket 替代回调服务器。默认：有 `appId+appSecret` 且 `eventEnabled` 未显式设为 `true` 时启用 |
| `callbackListen` | `string` | 否 | 回调监听地址，默认 `0.0.0.0:19820` |
| `callbackPath` | `string` | 否 | 回调路径，默认 `/feishu/events` |
| `streamPlaceholderEnabled` | `boolean` | 否 | 流式响应时发送占位提示 |
| `streamPlaceholderText` | `string` | 否 | 占位提示文本，默认 `"thinking..."` |
| `renderMode` | `string` | 否 | 消息渲染模式：`"raw"`（纯文本 + ASCII 表格）、`"card"`（交互卡片）、`"auto"`（自动嗅探，含格式/emoji 标记走卡片，否则纯文本）。默认 `"raw"` |

配置示例：

```json
{
  "channels": {
    "instances": {
      "my_feishu_bot": {
        "channelType": "feishu",
        "appId": "cli_xxx",
        "appSecret": "yyy",
        "allowFrom": ["*"]
      }
    }
  }
}
```

说明：Feishu 启用时至少需要配置 webhook URL 或 `appId+appSecret`。

#### 会话与标识符说明

```text
飞书事件                    InboundMessage              SessionKey
──────────────────────────────────────────────────────────────────
union_id = "on_xxx" ───→  sender_id: "on_xxx"  ──→  allow_from 过滤
chat_id = "oc_xxx"   ───→  chat_id:   "oc_xxx"   ──→  SessionKey("实例名", "oc_xxx")
```

- **Session 绑定**：Session key = `实例名:chat_id`。同一飞书群/会话内的所有用户共享一个 Agent session，上下文混合在同一轮对话中。Bot 不区分消息来源用户。
- **Sender ID 角色**：`sender_id` 使用 **union_id**（跨应用稳定的用户标识），仅用于 `allowFrom` 访问控制，不参与 session 隔离。如需按用户隔离上下文，需额外配置。
- **Event 接收方式**：WebSocket（默认）和 HTTP Callback 仅影响事件接收，不影响消息发送能力。只要配置了 `appId+appSecret`，发消息走 IM API。

## 7. tools

- `web.proxy`: 可选 HTTP 代理
- `web.search.apiKey`: `web_search` 所需 API key
- `web.search.maxResults`: 默认 `5`，需 `> 0`
- `exec.timeout`: 默认 `60`（秒），需 `> 0`
- `exec.pathAppend`: 追加到 PATH
- `exec.disableSafetyGuard`: 是否关闭危险命令模式防护，默认 `false`
- `exec.disableAllGuards`: 是否关闭 exec 的全部防护（包括 workspace/path 检查），默认 `false`
- `restrictToWorkspace`: 是否限制文件工具在 workspace 内
- `mcpServers`: MCP server 定义（每个 server 需至少提供 `command` 或 `url`）

## 8. gateway

- `host`: 默认 `0.0.0.0`
- `port`: 默认 `18790`
- `heartbeat.enabled`: 默认 `true`
- `heartbeat.intervalS`: 默认 `1800`

说明：当前二进制的 `gateway` 子命令会启动 channels + cron + heartbeat + agent loop。`host/port` 字段仅为保留配置，当前不生效，也不代表已暴露独立 HTTP API 服务。

## 9. acp（可选）

`acp` 配置用于注入 `acp_execute` 工具，典型字段：

- `enabled`
- `defaultAgent`
- `allowedAgents`
- `agents.<name>.command/args/env`

默认内置了 `claude` / `codex` / `copilot` 三个 agent 配置模板。
