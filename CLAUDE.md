# Project Context

nanobot-rs is a Rust-based AI agent framework with multi-channel messaging, tool execution, session management, and scheduling capabilities.

## Tech Stack

- **Language**: Rust 2024 edition
- **Async Runtime**: tokio
- **Error Handling**: anyhow + thiserror
- **Testing**: mockall, tempfile
- **Key Dependencies**: serde, tracing, reqwest, rmcp (MCP client)

## Architecture Overview

```
MessageBus (src/bus/)
    ↓ inbound messages
AgentLoop (src/agent/loop_core.rs)
    ↓ calls
LLMProvider (src/provider/) + ToolRegistry (src/tools/)
    ↓ results
SessionManager (src/session/) → JSONL persistence
    ↓ outbound
MessageBus → Channels
```

**Key Flow**: User message → Bus → Agent → LLM + Tools → Session save → Response

## Core Components

| Component | Location | Purpose |
|-----------|----------|---------|
| AgentLoop | `src/agent/loop_core.rs` | Main reasoning loop with tool calling |
| ToolRegistry | `src/tools/registry.rs` | Built-in + dynamic tool dispatcher |
| SessionManager | `src/session/manager.rs` | JSONL-based conversation persistence |
| MessageBus | `src/bus/` | Central pub/sub for inbound/outbound messages |
| CronService | `src/cron/service.rs` | Scheduler (every/cron/at) |
| SubagentManager | `src/agent/subagent.rs` | Spawn independent agent tasks |

## Type System Migration (In Progress)

Types are being consolidated into `src/types/`:
- `bus.rs` - Message types
- `cron.rs` - Scheduling types
- `provider.rs` - LLM provider types
- `session.rs` - Session types
- `tools.rs` - Tool types

**When adding types**: Check `src/types/` first, add there if domain-agnostic.

## Development Conventions

### Code Style
- Prefer strong typing and trait abstractions
- Use `Result<T>` for fallible operations, never panic in library code
- Async functions: `async fn` + `#[async_trait]` for traits
- Logging: `tracing::{info, warn, error}` (not `println!`)

### Testing
```bash
cargo test                    # Run all tests
cargo test --test integration # Integration tests only
RUST_LOG=debug cargo test     # With logs
```

- Unit tests: `#[cfg(test)]` in same file
- Mocks: Use `mockall` for trait dependencies
- Temp files: Use `tempfile::tempdir()`

### Common Tasks

**Add a new tool**:
1. Create `src/tools/my_tool.rs`
2. Implement `Tool` trait
3. Register in `ToolRegistry::new()` or via `register_dynamic_tool()`
4. Add tests

**Modify agent loop**:
1. Read `src/agent/loop_core.rs` first
2. Consider session isolation (per-session locks)
3. Wrap tool errors as text (don't break turn)
4. Test multi-turn scenarios

**Add a provider**:
1. Implement `LLMProvider` trait in `src/provider/`
2. Register in `ProviderRegistry`
3. Update `src/config/schema.rs`

## Built-in Tools

- **filesystem**: read_file, write_file, list_directory, search_files
- **shell**: execute_command
- **web**: fetch_url, search_web
- **message**: send_message (to other sessions)
- **spawn**: spawn_subagent (parallel tasks)
- **cron**: add/list/remove scheduled jobs
- **MCP**: Dynamic tools via Model Context Protocol

## Key Design Decisions

1. **Single-process**: Not distributed, local scheduling only
2. **Session isolation**: Concurrent sessions with per-session locks
3. **Error recovery**: Tool errors → text prompts (don't abort turn)
4. **Config compatibility**: Support both camelCase and snake_case for Python migration
5. **No tool transactions**: Tools are independent, no atomicity guarantees

## Commands

```bash
# Development
cargo build
cargo run -- agent          # Start agent mode
cargo run -- gateway        # Start gateway mode

# Testing
cargo test
cargo clippy
cargo fmt

# Debugging
RUST_LOG=debug cargo run
# Sessions stored in: workspace/sessions/*.jsonl
```

## Important Files

- `RUST_MVP_DESIGN.md` - Detailed design doc
- `templates/` - Agent context templates (AGENTS.md, TOOLS.md, etc.)
- `workspace/sessions/` - Conversation history (JSONL)
- `workspace/skills/` - Custom skills (each with SKILL.md)

## Current Status

✅ Agent loop, tools, sessions, cron, heartbeat, subagents
⚠️ Channel adapters (partial), Skills system (basic framework)

## References

- Design: `RUST_MVP_DESIGN.md`
- Refactoring: `REFACTORING_LOG.md`, `CIRCULAR_DEPENDENCY_SOLUTION_SUMMARY.md`
- Components: `docs/` directory
