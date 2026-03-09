# Quick Start Guide

Get started with nanobot-rs in minutes.

## Prerequisites

- Rust 1.75+ (2024 edition)
- An LLM API key (Anthropic, OpenAI, or compatible provider)

## Installation

### From Source

```bash
git clone https://github.com/yourusername/nanobot-rs.git
cd nanobot-rs
cargo build --release
```

The binary will be at `target/release/nanobot-rs`.

## Initial Setup

### 1. Onboard

Initialize configuration and workspace:

```bash
nanobot-rs onboard
```

This creates:
- `~/.nanobot/config.json` - Configuration file
- `~/.nanobot/workspace/` - Working directory for sessions, skills, and memory

### 2. Configure API Key

Edit `~/.nanobot/config.json` and add your API key:

```json
{
  "agents": {
    "defaults": {
      "workspace": "~/.nanobot/workspace",
      "model": "anthropic/claude-opus-4-5",
      "provider": "auto",
      "maxTokens": 8192,
      "temperature": 0.1,
      "maxToolIterations": 40,
      "memoryWindow": 100
    }
  },
  "providers": {
    "anthropic": {
      "apiKey": "sk-ant-xxx"
    },
    "openai": {
      "apiKey": "sk-xxx"
    },
    "githubCopilot": {
      "apiKey": ""
    }
  }
}
```

**Note:** GitHub Copilot uses OAuth authentication and doesn't require an API key. Authenticate first with `copilot login` or `nanobot-rs provider login github_copilot`, then use models with the `github-copilot/` or `github_copilot/` prefix.

**Supported Providers:**
- `anthropic` - Claude models
- `openai` - GPT models
- `github_copilot` - GitHub Copilot (OAuth, no API key needed)
- `openai_codex` - OpenAI Codex (OAuth, not yet implemented)
- `openrouter` - Multiple providers
- `deepseek`, `groq`, `gemini`, `moonshot`, `minimax`
- `zhipu`, `dashscope`, `siliconflow`, `volcengine`
- Custom providers via `custom` config

## Usage

### Agent Mode (CLI Chat)

#### Single Message

```bash
nanobot-rs agent -m "Hello! What can you do?"
```

#### Using GitHub Copilot

```bash
# Authenticate once
nanobot-rs provider login github_copilot

# Set model with github-copilot prefix
nanobot-rs agent -m "Explain this code" -s "copilot:session1"
```

Or configure in `config.json`:

```json
{
  "agents": {
    "defaults": {
      "model": "github-copilot/gpt-4o",
      "provider": "github_copilot"
    }
  }
}
```

#### Interactive Session

```bash
nanobot-rs agent
```

Then type messages interactively. The agent will:
- Execute tools (read/write files, run commands, search web)
- Maintain conversation history
- Use skills from `workspace/skills/`

#### Custom Session

```bash
nanobot-rs agent -s "my-project:task-1" -m "Analyze the codebase"
```

Sessions are stored in `workspace/sessions/<session-key>.jsonl`.

### Gateway Mode (Multi-Channel)

Start the HTTP gateway for channel integrations:

```bash
nanobot-rs gateway
```

Default: `http://0.0.0.0:18790`

Configure channels in `config.json`:

```json
{
  "channels": {
    "telegram": {
      "enabled": true,
      "allowFrom": ["user123"],
      "botToken": "xxx"
    }
  }
}
```

### Status Check

```bash
nanobot-rs status
```

Shows configuration and workspace status.

## Core Features

### Built-in Tools

The agent has access to these tools:

| Tool | Description |
|------|-------------|
| `read_file` | Read file contents |
| `write_file` | Write/create files |
| `edit_file` | Edit existing files |
| `list_dir` | List directory contents |
| `exec` | Execute shell commands |
| `web_search` | Search the web (requires API key) |
| `web_fetch` | Fetch web content |
| `message` | Send messages to other sessions |
| `spawn` | Spawn parallel subagent tasks |
| `cron` | Schedule recurring jobs |

### Skills System

Add custom skills to extend agent capabilities:

```bash
mkdir -p ~/.nanobot/workspace/skills/my-skill
cat > ~/.nanobot/workspace/skills/my-skill/SKILL.md << 'EOF'
---
description: My custom skill
always: false
---

# My Skill

This skill does X, Y, Z.

## Usage

When the user asks for X, do Y.
EOF
```

Skills are automatically loaded and available to the agent.

### Memory System

#### Session Memory
- Stored in `workspace/sessions/*.jsonl`
- First line: metadata, subsequent lines: messages
- Controlled by `memoryWindow` config (default: 100 messages)

#### Long-term Memory
- `workspace/memory/MEMORY.md` - Cross-session knowledge
- `workspace/memory/YYYY-MM-DD.md` - Daily logs
- Agent can read/write to persist learnings

### Scheduling (Cron)

The agent can schedule tasks:

```bash
# In agent chat:
"Schedule a daily backup at 2am"
```

The agent will use the `cron` tool to create recurring jobs.

### Subagents

Spawn parallel agent tasks:

```bash
# In agent chat:
"Spawn a subagent to analyze the logs while you work on the code"
```

The agent will use the `spawn` tool to create independent tasks.

## Configuration Reference

### Agent Defaults

```json
{
  "agents": {
    "defaults": {
      "workspace": "~/.nanobot/workspace",
      "model": "anthropic/claude-opus-4-5",
      "provider": "auto",
      "maxTokens": 8192,
      "temperature": 0.1,
      "maxToolIterations": 40,
      "memoryWindow": 100,
      "reasoningEffort": null
    }
  }
}
```

- `workspace` - Base directory for all operations
- `model` - Model name (with optional provider prefix)
- `provider` - Force specific provider or "auto"
- `maxTokens` - Max tokens per response
- `temperature` - Sampling temperature (0.0-2.0)
- `maxToolIterations` - Max tool calls per turn
- `memoryWindow` - Number of messages to keep in context
- `reasoningEffort` - Extended thinking mode (provider-specific)

### Tools Configuration

```json
{
  "tools": {
    "restrictToWorkspace": false,
    "web": {
      "proxy": null,
      "search": {
        "apiKey": "",
        "maxResults": 5
      }
    },
    "exec": {
      "timeout": 60,
      "pathAppend": ""
    },
    "mcpServers": {}
  }
}
```

- `restrictToWorkspace` - Limit file operations to workspace
- `web.search.apiKey` - API key for web search
- `exec.timeout` - Command timeout in seconds
- `mcpServers` - Model Context Protocol server configs

### MCP Servers

Integrate external tools via MCP:

```json
{
  "tools": {
    "mcpServers": {
      "filesystem": {
        "command": "npx",
        "args": ["-y", "@modelcontextprotocol/server-filesystem", "/path/to/dir"],
        "env": {},
        "toolTimeout": 30
      }
    }
  }
}
```

### Gateway Configuration

```json
{
  "gateway": {
    "host": "0.0.0.0",
    "port": 18790,
    "heartbeat": {
      "enabled": true,
      "intervalS": 1800
    }
  }
}
```

- `heartbeat.enabled` - Enable periodic agent check-ins
- `heartbeat.intervalS` - Heartbeat interval in seconds

## Examples

### Example 1: Code Analysis

```bash
nanobot-rs agent -m "Analyze the Rust code in src/ and suggest improvements"
```

The agent will:
1. List files in `src/`
2. Read relevant files
3. Analyze code patterns
4. Provide suggestions

### Example 2: Automated Task

```bash
nanobot-rs agent -m "Create a daily report script that summarizes git commits"
```

The agent will:
1. Write a shell script
2. Test it
3. Optionally schedule it with cron

### Example 3: Research Task

```bash
nanobot-rs agent -m "Research the latest Rust async patterns and summarize"
```

The agent will:
1. Search the web
2. Fetch relevant articles
3. Synthesize findings
4. Save to memory

## Troubleshooting

### Enable Debug Logs

```bash
RUST_LOG=debug nanobot-rs agent -m "test"
```

For specific modules:

```bash
RUST_LOG=nanobot_rs::agent=trace nanobot-rs agent
```

### Check Configuration

```bash
cat ~/.nanobot/config.json | jq
```

### Inspect Sessions

```bash
cat ~/.nanobot/workspace/sessions/cli_direct.jsonl | jq
```

### Reset Configuration

```bash
nanobot-rs onboard --overwrite
```

### Common Issues

**"No provider configured"**
- Add API key to `~/.nanobot/config.json`
- For GitHub Copilot, no API key needed - just use `github-copilot/` model prefix

**"openai_codex OAuth provider is not implemented yet"**
- OpenAI Codex OAuth is not yet supported in the current version
- Use GitHub Copilot instead for OAuth-based access

**"Workspace not found"**
- Run `nanobot-rs onboard` first

**"Tool execution timeout"**
- Increase `tools.exec.timeout` in config

**"Memory window too small"**
- Increase `agents.defaults.memoryWindow` in config

## Next Steps

- Read [CLAUDE.md](../CLAUDE.md) for architecture overview
- Read [AGENT.md](../AGENT.md) for development guide
- Explore `workspace/skills/` for custom skills
- Check `templates/` for agent context templates
- Review `RUST_MVP_DESIGN.md` for detailed design

## Resources

- GitHub: https://github.com/yourusername/nanobot-rs
- Issues: https://github.com/yourusername/nanobot-rs/issues
- Docs: `docs/` directory
