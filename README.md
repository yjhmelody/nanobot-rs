# nanobot

[![CI](https://github.com/yjhmelody/nanobot-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/yjhmelody/nanobot-rs/actions/workflows/ci.yml)
[![Release](https://github.com/yjhmelody/nanobot-rs/actions/workflows/release.yml/badge.svg)](https://github.com/yjhmelody/nanobot-rs/actions/workflows/release.yml)

中文 | [English](./README_EN.md)

`nanobot` 是一个 Rust 实现的本地 Agent 运行时，提供：

- CLI 单轮与交互式对话
- Gateway 模式与渠道分发
- 原生 `Anthropic Messages` / `OpenAI-compatible` provider（`responses` 与 `chat_completions`）
- 文件、Shell、Web、消息、子代理、Cron、MCP、ACP 等工具能力
- 本地会话持久化、记忆、心跳与定时任务

> ⚠️ 项目定位：`nanobot` 当前主要用于学习与研究，不建议直接用于生产场景。

## 致谢与参考

本项目的早期实现参考了以下开源项目：

- OpenClaw：<https://github.com/openclaw/openclaw>
- Python 版 Nanobot：<https://github.com/HKUDS/nanobot>

## 适用场景

- 在本地快速搭建可扩展的 AI Agent
- 通过 Telegram/Feishu/CLI 接入多轮对话与工具调用
- 用统一配置切换 OpenAI-compatible 与 Anthropic-compatible 网关

## 当前能力状态

- **Provider**：`anthropic`、`openai`、`custom`、任意自定义键（HashMap）
- **Channel**：`CLI`（可用）、`Telegram`（可用）、`Feishu/Lark`（可用）、`Discord`（占位实现）
- **运行模式**：`agent`（交互/单轮）与 `gateway`（渠道 + 调度 + 心跳）

## 文档

- `docs/README.md`：文档索引与维护原则
- `docs/QUICK_START.md`：安装、配置与日常使用
- `docs/ARCHITECTURE.md`：当前系统架构与设计边界
- `docs/DEVELOPMENT.md`：开发、测试与调试流程
- `docs/OPEN_SOURCE_LEARNING_PATH.md`：面向新手贡献者的阅读与上手路径
- `docs/CONFIGURATION.md`：配置字段与默认行为参考
- `docs/CLI_REFERENCE.md`：命令行与聊天内建命令参考
- `docs/DEPLOYMENT.md`：gateway 部署与运行说明

## 快速开始

### 1. 环境要求

- Rust 1.95
- `cargo`, `just`（可选）

### 2. 启动

```bash
cargo run -- onboard
$EDITOR ~/.nanobot/config.json
cargo run -- agent -m "Hello!"
```

### 3. 常见 provider 示例

DeepSeek（OpenAI-compatible）：

```json
{
  "agents": {
    "defaults": {
      "provider": "deepseek"
    }
  },
  "providers": {
    "deepseek": {
      "providerType": "open_ai_compatible",
      "wireApi": "chat_completions",
      "model": "deepseek-v4-flash",
      "apiBase": "https://api.deepseek.com",
      "apiKey": "{{DEEPSEEK_API_KEY}}"
    }
  }
}
```

DeepSeek（Anthropic-compatible）：

```json
{
  "agents": {
    "defaults": {
      "provider": "deepseekAnthropic"
    }
  },
  "providers": {
    "deepseekAnthropic": {
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

## CI / Release

仓库已配置 GitHub Actions：

- `CI`：在 Linux / macOS / Windows 上执行格式、lint、测试
- `Release`：在 Linux / macOS / Windows 上构建发行包；推送 `v*` tag 时自动创建 GitHub Release

具体发布说明见 `docs/DEPLOYMENT.md`。
发布前可用 `just changelog vX.Y.Z` 自动生成 `CHANGELOG.md` 条目。仓库 `pre-push` 已内置 tag-changelog 校验：推送 `v*` tag 时若缺失对应 changelog 条目会被拦截。

## 开发命令

仓库包含 `justfile`：

```bash
just                 # 查看可用命令
just ci              # 本地 CI 校验
just e2e             # 离线端到端验证
just agent -m "hi"   # 启动 agent
```

## 贡献与反馈

- Issue：提交 bug / 功能请求
- PR：欢迎改进代码与文档
- 详细开发说明见 `docs/DEVELOPMENT.md`
