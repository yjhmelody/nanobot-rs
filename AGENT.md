# Agent Behavior Guide

You are an AI assistant working on the nanobot-rs project. This guide defines how you should approach tasks.

## Core Principles

1. **Read before writing** - Always read relevant code before making changes
2. **Type-safe by default** - Leverage Rust's type system, avoid runtime errors
3. **Test-driven** - Write tests for new features, ensure existing tests pass
4. **Incremental changes** - Small, focused changes over large rewrites
5. **Document decisions** - Update docs when making architectural changes

## Your Workflow

### Before coding
1. Read `CLAUDE.md` for architecture overview
2. Check `src/types/` for existing type definitions
3. Review related module code and tests
4. Understand the error handling patterns

### While coding
- Follow existing code patterns and style
- Use `Result<T>` for fallible operations
- Add `tracing::info/warn/error` for important events
- Handle all error cases explicitly
- Write clear, self-documenting code

### After coding
```bash
cargo test              # Must pass
cargo clippy            # Check warnings
cargo fmt               # Format code
```

## Common Patterns

### Adding a Tool
```rust
// 1. Define in src/tools/my_tool.rs
pub struct MyTool { /* ... */ }

#[async_trait]
impl Tool for MyTool {
    fn name(&self) -> &str { "my_tool" }
    fn definition(&self) -> ToolDefinition { /* ... */ }
    async fn execute(&self, args: &str, ctx: &ToolContext) -> Result<String> {
        let params: MyParams = parse_args(args)?;
        // Implementation
    }
}

// 2. Register in ToolRegistry::new()
let tool = Arc::new(MyTool::new());
tools.insert(tool.name().to_string(), tool);

// 3. Add tests
#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn test_my_tool() { /* ... */ }
}
```

### Error Handling
```rust
// Good: Return Result, let caller decide
pub fn process_data(input: &str) -> Result<Data> {
    let parsed = serde_json::from_str(input)
        .context("Failed to parse input")?;
    Ok(parsed)
}

// Bad: Panic or unwrap
pub fn process_data(input: &str) -> Data {
    serde_json::from_str(input).unwrap() // ❌ Never do this
}
```

### Async Patterns
```rust
// Use tokio::spawn for concurrent tasks
let handle = tokio::spawn(async move {
    // Task implementation
});

// Use Arc for shared state
let shared = Arc::new(MyState::new());
let cloned = shared.clone();
tokio::spawn(async move {
    cloned.do_something().await;
});
```

## Available Tools

When working as an agent, you have access to:

- **read_file** - Read file contents
- **write_file** - Write/create files
- **list_directory** - List directory contents
- **search_files** - Search for files by pattern
- **execute_command** - Run shell commands (avoid long-running processes)
- **send_message** - Send messages to other sessions
- **spawn_subagent** - Create parallel agent tasks
- **add_cron_job** - Schedule recurring tasks

## Memory System

### Session Memory
- Conversation history stored in `workspace/sessions/*.jsonl`
- First line: metadata, subsequent lines: messages
- Controlled by `memory_window` config

### Long-term Memory
- `memory/MEMORY.md` - Cross-session knowledge
- `memory/YYYY-MM-DD.md` - Daily logs
- Update when you learn something important

### Context Templates
Located in `templates/`:
- `AGENTS.md` - Agent behavior guidelines
- `TOOLS.md` - Available tools reference
- `USER.md` - User preferences
- `SOUL.md` - Agent personality
- `HEARTBEAT.md` - Periodic tasks

## Skills System

Skills are in `workspace/skills/`:
```
skills/
  my-skill/
    SKILL.md       # Skill description
    skill.toml     # Optional: dependencies, requirements
```

Skills are dynamically loaded and can extend agent capabilities.

## Debugging

### View logs
```bash
RUST_LOG=debug cargo run
RUST_LOG=nanobot_rs::agent=trace cargo run  # Specific module
```

### Run tests with output
```bash
cargo test -- --nocapture
cargo test --test integration_test
```

### Inspect sessions
```bash
cat workspace/sessions/cli_direct.jsonl | jq
```

## Critical Constraints

1. **Concurrency**: Agent loop handles multiple sessions concurrently
   - Each session has its own lock
   - Don't block on shared state

2. **Error recovery**: Tool errors become text prompts
   - Don't abort the turn on tool failure
   - Wrap errors in helpful context

3. **Resource cleanup**: Always clean up async resources
   - Close MCP connections
   - Cancel spawned tasks when session ends

4. **Type migration**: Check `src/types/` first
   - New domain types go in `src/types/`
   - Update imports when types move

## When Stuck

1. Read the relevant module in `src/`
2. Check tests for usage examples
3. Review `RUST_MVP_DESIGN.md` for design rationale
4. Look at `REFACTORING_LOG.md` for historical context
5. Check `docs/` for component documentation

## Anti-patterns to Avoid

❌ Using `unwrap()` or `expect()` in library code
❌ Ignoring errors with `let _ = ...`
❌ Long-running synchronous operations in async context
❌ Modifying code without reading it first
❌ Adding features not requested
❌ Breaking existing tests without fixing them
