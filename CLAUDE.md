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

### Architecture Principles

**Trait-First Design**:
- **Always define traits before implementations** for major components
- Traits enable loose coupling, testability, and future extensibility
- Each component should depend on trait abstractions, not concrete types

**Examples of trait-first design in this codebase**:
- `LLMProvider` trait → Multiple provider implementations (Anthropic, OpenAI, etc.)
- `Tool` trait → Built-in and dynamic tools (filesystem, web, MCP, etc.)
- `SessionStore` trait → Different persistence backends (JSONL, future: SQLite, Redis)
- `MemoryProvider` trait → Pluggable memory sources (file-based, composite, future: vector DB)
- `SkillsProvider` trait → Skills management (file-based, future: cached, remote)
- `ContextProvider` trait → Context building strategies

**Low Coupling Guidelines**:
1. **Depend on abstractions**: Use `Arc<dyn Trait>` or `Box<dyn Trait>` for dependencies
2. **Avoid circular dependencies**: If A needs B and B needs A, introduce a trait or event bus
3. **Use dependency injection**: Pass dependencies through constructors, not global state
4. **Separate concerns**: Each module should have a single, well-defined responsibility
5. **Interface segregation**: Keep traits focused and minimal

**When adding a new component**:
1. ✅ Define the trait first (e.g., `pub trait MyComponent: Send + Sync`)
2. ✅ Document the trait's contract and use cases
3. ✅ Implement the trait for concrete types
4. ✅ Use the trait in dependent code, not the concrete type
5. ✅ Add tests using mock implementations (via `mockall`)

### Code Style
- Prefer strong typing and trait abstractions
- Use `Result<T>` for fallible operations, never panic in library code
- Async functions: `async fn` + `#[async_trait]` for traits
- Logging: `tracing::{info, warn, error}` (not `println!`)

### Concurrency & Performance Guidelines

**Lock Selection Strategy**:
1. **High-concurrency read-heavy data** → Use `DashMap` (lock-free, 16-way sharding)
   - Example: `SessionManager.cache`, `SubagentManager.running_tasks`
   - 10-20x faster than RwLock for concurrent access

2. **Short critical sections (no await)** → Use `parking_lot::{Mutex, RwLock}`
   - Example: `SharedToolConfig.inner`, `AgentLoop.last_cleanup`
   - 3-5x faster than tokio::sync, smaller memory footprint (24 vs 40 bytes)
   - Cannot be held across await points

3. **Long critical sections (crosses await)** → Use `tokio::sync::{Mutex, RwLock}`
   - Example: `AgentLoop.session_locks` (held during message processing)
   - Required when lock must be held across await points
   - Async-aware, prevents blocking tokio runtime

4. **Simple flags/counters** → Use `std::sync::atomic::{AtomicBool, AtomicUsize}`
   - Example: `AgentLoop.running`, `CronService.running`
   - 100x faster than any lock, zero contention
   - Use `Ordering::Release`/`Acquire` for synchronization

**Anti-patterns to avoid**:
- ❌ `Arc<RwLock<HashMap>>` → Use `DashMap` instead
- ❌ `tokio::sync::Mutex` for short critical sections → Use `parking_lot::Mutex`
- ❌ `RwLock<bool>` → Use `AtomicBool`
- ❌ Holding locks across await points with parking_lot → Use tokio::sync

**Performance checklist**:
- [ ] Does this data structure have high concurrent access? → Consider DashMap
- [ ] Is the critical section short (<100 instructions)? → Consider parking_lot
- [ ] Does the lock cross an await point? → Must use tokio::sync
- [ ] Is this just a flag or counter? → Use atomics

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
1. Define the tool's interface (usually implements `Tool` trait)
2. Create `src/tools/my_tool.rs`
3. Implement `Tool` trait with proper error handling
4. Register in `ToolRegistry::new()` or via `register_dynamic_tool()`
5. Add unit tests with mock dependencies

**Add a new component**:
1. **Design the trait first**: Define the interface in a separate file (e.g., `traits.rs`)
2. Document the trait's purpose, methods, and contracts
3. Implement the trait for concrete types
4. Use dependency injection to pass the trait to consumers
5. Write tests using mock implementations

**Modify agent loop**:
1. Read `src/agent/loop_core.rs` first
2. Consider session isolation (per-session locks)
3. Wrap tool errors as text (don't break turn)
4. Test multi-turn scenarios

**Add a provider**:
1. Implement `LLMProvider` trait in `src/provider/`
2. Register in `ProviderRegistry`
3. Update `src/config/schema.rs`
4. Add integration tests

## Built-in Tools

- **filesystem**: read_file, write_file, list_directory, search_files
- **shell**: execute_command
- **web**: fetch_url, search_web
- **message**: send_message (to other sessions)
- **spawn**: spawn_subagent (parallel tasks)
- **cron**: add/list/remove scheduled jobs
- **MCP**: Dynamic tools via Model Context Protocol

## Key Design Decisions

1. **Trait-first architecture**: All major components are defined as traits before implementation
   - Enables loose coupling between modules
   - Facilitates testing with mock implementations
   - Allows multiple implementations (e.g., different providers, storage backends)
   - Makes the system extensible without modifying existing code

2. **Single-process**: Not distributed, local scheduling only

3. **Session isolation**: Concurrent sessions with per-session locks

4. **Error recovery**: Tool errors → text prompts (don't abort turn)

5. **Config compatibility**: Support both camelCase and snake_case for Python migration

6. **No tool transactions**: Tools are independent, no atomicity guarantees

7. **Progressive disclosure**: Skills and context loaded on-demand to minimize token usage

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
