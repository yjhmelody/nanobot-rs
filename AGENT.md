# Agent Behavior Guide

You are an AI assistant working on the nanobot-rs project. This guide defines how you should approach tasks.

## Core Principles

1. **Trait-first design** - Define traits before implementations for all major components
2. **Low coupling** - Components depend on trait abstractions, not concrete types
3. **Read before writing** - Always read relevant code before making changes
4. **Type-safe by default** - Leverage Rust's type system, avoid runtime errors
5. **Test-driven** - Write tests for new features, ensure existing tests pass
6. **Incremental changes** - Small, focused changes over large rewrites
7. **Document decisions** - Update docs when making architectural changes

## Your Workflow

### Before coding
1. Read `CLAUDE.md` for architecture overview and trait-first design principles
2. Check if a trait already exists for the component you're working on
3. Check `src/types/` for existing type definitions
4. Review related module code and tests
5. Understand the error handling patterns

### While coding
- **Design traits first** for new components before writing implementations
- Follow existing code patterns and style
- Use trait abstractions (`Arc<dyn Trait>`) instead of concrete types for dependencies
- Use `Result<T>` for fallible operations
- Add `tracing::info/warn/error` for important events
- Handle all error cases explicitly
- Write clear, self-documenting code
- Keep components loosely coupled through dependency injection

### After coding
```bash
cargo test              # Must pass
cargo clippy            # Check warnings
cargo fmt               # Format code
```

## Common Patterns

### Trait-First Component Design

When adding a new component, always start with the trait:

```rust
// 1. Define the trait in src/my_module/traits.rs or src/my_module/mod.rs
use async_trait::async_trait;

#[async_trait]
pub trait MyComponent: Send + Sync {
    /// Document what this method does
    async fn do_something(&self, input: &str) -> Result<String>;

    /// Document the contract
    fn get_name(&self) -> &str;
}

// 2. Implement for concrete types
pub struct FileBasedComponent {
    path: PathBuf,
}

#[async_trait]
impl MyComponent for FileBasedComponent {
    async fn do_something(&self, input: &str) -> Result<String> {
        // Implementation
    }

    fn get_name(&self) -> &str {
        "file-based"
    }
}

// 3. Use trait in dependent code
pub struct Consumer {
    component: Arc<dyn MyComponent>,  // ✅ Depend on trait
}

impl Consumer {
    pub fn new(component: Arc<dyn MyComponent>) -> Self {
        Self { component }
    }
}

// 4. Test with mocks
#[cfg(test)]
mod tests {
    use mockall::mock;

    mock! {
        Component {}

        #[async_trait]
        impl MyComponent for Component {
            async fn do_something(&self, input: &str) -> Result<String>;
            fn get_name(&self) -> &str;
        }
    }

    #[tokio::test]
    async fn test_consumer() {
        let mut mock = MockComponent::new();
        mock.expect_do_something()
            .returning(|_| Ok("mocked".to_string()));

        let consumer = Consumer::new(Arc::new(mock));
        // Test consumer behavior
    }
}
```

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

## Performance Guidelines

### Lock Selection Strategy

Choose the right synchronization primitive for optimal performance:

**1. High-concurrency collections → DashMap**
```rust
// ✅ Good: Lock-free concurrent access
use dashmap::DashMap;
let cache: DashMap<String, Session> = DashMap::new();

// ❌ Bad: Lock contention
let cache: Arc<RwLock<HashMap<String, Session>>> = ...;
```
- Use for: Caches, registries, task tracking
- Performance: 10-20x faster than RwLock
- Examples: `SessionManager.cache`, `SubagentManager.running_tasks`

**2. Short critical sections → parking_lot**
```rust
// ✅ Good: Fast synchronous lock
use parking_lot::Mutex;
let config: Arc<Mutex<Config>> = Arc::new(Mutex::new(config));
let guard = config.lock(); // No .await

// ❌ Bad: Async overhead for sync operation
let config: Arc<tokio::sync::Mutex<Config>> = ...;
let guard = config.lock().await; // Unnecessary
```
- Use for: Config updates, timestamps, counters
- Performance: 3-5x faster than tokio::sync
- Constraint: Cannot hold across `await` points
- Examples: `SharedToolConfig.inner`, `AgentLoop.last_cleanup`

**3. Long critical sections → tokio::sync**
```rust
// ✅ Good: Async-aware lock
use tokio::sync::Mutex;
let lock: Arc<Mutex<()>> = Arc::new(Mutex::new(()));
let guard = lock.lock().await;
some_async_operation().await; // Safe
drop(guard);

// ❌ Bad: Blocks tokio runtime
use parking_lot::Mutex;
let guard = lock.lock();
some_async_operation().await; // DEADLOCK RISK!
```
- Use for: Operations spanning async calls
- Required when: Lock held across `await`
- Examples: `AgentLoop.session_locks`

**4. Simple flags → Atomics**
```rust
// ✅ Good: Zero-cost synchronization
use std::sync::atomic::{AtomicBool, Ordering};
let running = Arc::new(AtomicBool::new(false));
running.store(true, Ordering::Release);
if running.load(Ordering::Acquire) { }

// ❌ Bad: Lock overhead for simple flag
let running: Arc<RwLock<bool>> = ...;
```
- Use for: Boolean flags, simple counters
- Performance: 100x faster than locks
- Examples: `AgentLoop.running`, `CronService.running`

### Quick Decision Tree
```
Need synchronization?
├─ Just a flag/counter? → AtomicBool/AtomicUsize
├─ High concurrent collection? → DashMap
├─ Crosses await point? → tokio::sync::Mutex/RwLock
└─ Short critical section? → parking_lot::Mutex/RwLock
```

### Performance Anti-patterns
```rust
// ❌ Don't use RwLock<HashMap> for high concurrency
Arc<RwLock<HashMap<K, V>>>  // Use DashMap instead

// ❌ Don't use tokio::sync for short operations
tokio::sync::Mutex<Instant>  // Use parking_lot::Mutex

// ❌ Don't use locks for simple flags
RwLock<bool>  // Use AtomicBool

// ❌ Don't hold parking_lot locks across await
let guard = parking_lot_mutex.lock();
async_op().await;  // DEADLOCK RISK!
```

### Memory Ordering for Atomics
```rust
// For flags and synchronization
flag.store(true, Ordering::Release);   // Writer
if flag.load(Ordering::Acquire) { }    // Reader

// For relaxed counters (no sync needed)
counter.fetch_add(1, Ordering::Relaxed);
```

See `docs/PERFORMANCE_OPTIMIZATION.md` for detailed guidelines.

## When Stuck

1. Read the relevant module in `src/`
2. Check tests for usage examples
3. Review `RUST_MVP_DESIGN.md` for design rationale
4. Look at `REFACTORING_LOG.md` for historical context
5. Check `docs/` for component documentation

## Anti-patterns to Avoid

### Architecture Anti-patterns
❌ **Concrete type dependencies** - Don't depend on concrete types when a trait exists
```rust
// ❌ Bad: Tight coupling to concrete type
pub struct Agent {
    provider: AnthropicProvider,  // Hard to test, hard to swap
}

// ✅ Good: Depend on trait abstraction
pub struct Agent {
    provider: Arc<dyn LLMProvider>,  // Testable, swappable
}
```

❌ **Implementation before trait** - Don't write implementations before defining the trait
```rust
// ❌ Bad: Implementation first, trait later (or never)
pub struct MyStorage { /* ... */ }
impl MyStorage {
    pub fn save(&self, data: &str) -> Result<()> { /* ... */ }
}

// ✅ Good: Trait first, then implementation
#[async_trait]
pub trait Storage: Send + Sync {
    async fn save(&self, data: &str) -> Result<()>;
}

pub struct MyStorage { /* ... */ }
#[async_trait]
impl Storage for MyStorage {
    async fn save(&self, data: &str) -> Result<()> { /* ... */ }
}
```

❌ **Circular dependencies** - Don't create circular module dependencies
```rust
// ❌ Bad: A depends on B, B depends on A
// src/module_a.rs
use crate::module_b::B;

// src/module_b.rs
use crate::module_a::A;

// ✅ Good: Introduce a trait or use an event bus
// src/traits.rs
pub trait ComponentB { /* ... */ }

// src/module_a.rs
use crate::traits::ComponentB;
```

### Code Anti-patterns
❌ Using `unwrap()` or `expect()` in library code
❌ Ignoring errors with `let _ = ...`
❌ Long-running synchronous operations in async context
❌ Modifying code without reading it first
❌ Adding features not requested
❌ Breaking existing tests without fixing them
❌ Using concrete types when traits are available
❌ Skipping trait definition for new components
