# nanobot

[![CI](https://github.com/yjhmelody/nanobot-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/yjhmelody/nanobot-rs/actions/workflows/ci.yml)
[![Release](https://github.com/yjhmelody/nanobot-rs/actions/workflows/release.yml/badge.svg)](https://github.com/yjhmelody/nanobot-rs/actions/workflows/release.yml)

[中文](./README.md) | English

`nanobot` is a local AI agent runtime implemented in Rust, featuring:

- Single-shot and interactive CLI chat
- Gateway mode with channel dispatch
- Native `Anthropic Messages` and `OpenAI-compatible` providers (`responses` and `chat_completions`)
- Tooling: filesystem, shell, web, messaging, subagent, cron, MCP, ACP
- Local session persistence, memory, heartbeat, and scheduling

> ⚠️ Project positioning: `nanobot` is currently intended for learning and research, and is not recommended for production use.

## Acknowledgements

The early implementation of this project referenced:

- OpenClaw: <https://github.com/openclaw/openclaw>
- Python nanobot: <https://github.com/HKUDS/nanobot>

## Use Cases

- Build an extensible local AI agent quickly
- Connect multi-turn conversations and tool calls via Telegram/CLI
- Switch between OpenAI-compatible and Anthropic-compatible gateways with one config format

## Current Capability Status

- **Providers**: `anthropic`, `openai`, `custom`, plus arbitrary custom provider keys (HashMap-based)
- **Channels**: `CLI` (available), `Telegram` (available), `Discord/Feishu` (placeholder)
- **Run modes**: `agent` (interactive/single-shot) and `gateway` (channels + scheduler + heartbeat)

## Documentation

- `docs/README.md`: docs index and maintenance principles
- `docs/QUICK_START.md`: installation, configuration, and daily usage
- `docs/ARCHITECTURE.md`: architecture and design boundaries
- `docs/DEVELOPMENT.md`: development, testing, and debugging
- `docs/OPEN_SOURCE_LEARNING_PATH.md`: onboarding path for new contributors
- `docs/CONFIGURATION.md`: config fields and behavior reference
- `docs/CLI_REFERENCE.md`: CLI and built-in chat command reference
- `docs/DEPLOYMENT.md`: gateway deployment and operations

## Quick Start

### 1. Requirements

- Rust 1.95
- `cargo`, `just` (optional)

### 2. Bootstrapping

```bash
cargo run -- onboard
$EDITOR ~/.nanobot/config.json
cargo run -- agent -m "Hello!"
```

### 3. Common Provider Examples

DeepSeek (OpenAI-compatible):

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

DeepSeek (Anthropic-compatible):

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

GitHub Actions are configured for:

- `CI`: format, lint, and tests on Linux/macOS/Windows
- `Release`: cross-platform packaging and automatic GitHub Release on `v*` tags

See `docs/DEPLOYMENT.md` for release details.
Before tagging, you can run `just changelog vX.Y.Z` to auto-generate a `CHANGELOG.md` entry.

## Development Commands

This repository includes a `justfile`:

```bash
just                 # list available commands
just ci              # local CI checks
just e2e             # offline end-to-end verification
just agent -m "hi"   # start agent
```

## Contributing & Feedback

- Issues: bug reports and feature requests
- PRs: code and documentation improvements are welcome
- See `docs/DEVELOPMENT.md` for contributor workflow
