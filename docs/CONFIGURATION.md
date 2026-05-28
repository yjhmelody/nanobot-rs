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
- `memoryWindow`: `100`
- `consolidationKeepRecent`: `10`
- `consolidationMinMessages`: `20`
- `consolidationSummaryMaxTokens`: `1000`
- `reasoningEffort`: `null`
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

## 4. providers

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

### 5.1 当前支持的 channel（按源码状态）

| Channel | 配置键 | 当前状态 |
|---|---|---|
| CLI | 无（内置） | 可用（本地终端） |
| Telegram | `channels.telegram` | 可用（完整适配器） |
| Discord | `channels.discord` | 占位实现（Placeholder） |
| Feishu/Lark | `channels.feishu`（支持别名 `channels.lark`） | 可用（应用 API 对话 + 事件订阅入站；兼容 webhook 出站） |

说明：

- `channels` 配置结构当前只定义了 `telegram/discord/feishu` 三个外部通道键；其中 `feishu` 支持配置别名 `lark`。
- 其他未在结构中定义的 channel 键不会被当前运行时接入。

公共字段：

- `sendProgress`（默认 `true`）
- `sendToolHints`（默认 `false`）
- `sendUsageSummary`（默认 `false`）
- `streamMode`（默认 `updateAll`，可选：`updateAll` / `updateProgress` / `append`）

通道字段结构（`telegram` / `discord` / `feishu`）：

- `enabled: bool`
- `allowFrom: string[]`
- `agentOverrides?: object`
- 其余字段通过 `extra` 承载（配置文件中直接写扁平字段）

`agentOverrides` 当前支持：

- `memoryWindow`
- `consolidationEnabled`
- `consolidationKeepRecent`
- `consolidationMinMessages`
- `consolidationSummaryMaxTokens`

说明：

- 这些覆盖值只对对应 channel 生效。
- 典型用法是给 `feishu` 这种长任务入口设置更大的 `memoryWindow` 和更保守的 consolidation 参数。

`allowFrom` 约束（启用通道时）：

- 不能为空
- 不允许空字符串或首尾空白
- `*` 不能和显式 id 混用

Telegram 额外字段：

- `token`（必填）
- `apiBase`（可选，默认 `https://api.telegram.org`）
- `receiveAck`（可选，默认 `false`）

Feishu/Lark 额外字段：

- `webhook`（可选；也可用 `webhookUrl` / `url` / `botKey`，用于 webhook 出站）
- `appId` + `appSecret`（可选；两者需同时配置，用于应用 API 按 `chat_id` 回消息）
- `secret`（可选；也可用 `signSecret`，用于签名）
- `apiBase`（可选；仅在使用 `botKey` 时拼接 webhook，默认 `https://open.feishu.cn`）
- `verifyToken`（可选；事件订阅 token 校验）
- `callbackListen`（可选；默认 `0.0.0.0:19820`）
- `callbackPath`（可选；默认 `/feishu/events`）
- `eventEnabled`（可选；默认在 `appId/appSecret` 模式下为 `true`，关闭后仅出站）

说明：`feishu` 启用时，至少需要配置以下其一：

- `webhook` / `webhookUrl` / `url` / `botKey`
- `appId` + `appSecret`

## 6. tools

- `web.proxy`: 可选 HTTP 代理
- `web.search.apiKey`: `web_search` 所需 API key
- `web.search.maxResults`: 默认 `5`，需 `> 0`
- `exec.timeout`: 默认 `60`（秒），需 `> 0`
- `exec.pathAppend`: 追加到 PATH
- `exec.disableSafetyGuard`: 是否关闭危险命令模式防护，默认 `false`
- `exec.disableAllGuards`: 是否关闭 exec 的全部防护（包括 workspace/path 检查），默认 `false`
- `restrictToWorkspace`: 是否限制文件工具在 workspace 内
- `mcpServers`: MCP server 定义（每个 server 需至少提供 `command` 或 `url`）

## 7. gateway

- `host`: 默认 `0.0.0.0`
- `port`: 默认 `18790`
- `heartbeat.enabled`: 默认 `true`
- `heartbeat.intervalS`: 默认 `1800`

说明：当前二进制的 `gateway` 子命令会启动 channels + cron + heartbeat + agent loop。`host/port` 字段仅为保留配置，当前不生效，也不代表已暴露独立 HTTP API 服务。

## 8. acp（可选）

`acp` 配置用于注入 `acp_execute` 工具，典型字段：

- `enabled`
- `defaultAgent`
- `allowedAgents`
- `agents.<name>.command/args/env`

默认内置了 `claude` / `codex` / `copilot` 三个 agent 配置模板。
