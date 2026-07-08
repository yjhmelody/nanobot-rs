//! # nanobot-tools
//!
//! Tool execution framework for the nanobot AI agent.
//!
//! This crate provides the core tool abstraction and all built-in tools
//! that agents can invoke. It is the central dispatch layer between the
//! agent loop and various capabilities: filesystem access, shell commands,
//! web search/fetch, scheduling, subagent spawning, messaging, MCP-based
//! dynamic tools, and code search.
//!
//! ## Architecture
//!
//! ```text
//! ToolRegistry (central dispatcher)
//!   ├── base::Tool trait (contract for all tools)
//!   ├── filesystem (read_file, write_file, edit_file, list_dir)
//!   ├── shell      (exec)
//!   ├── web        (web_search, web_fetch)
//!   ├── search     (search_files, grep_code)
//!   ├── cron       (schedule reminders)
//!   ├── spawn      (subagent tasks)
//!   └── mcp        (dynamically registered MCP tools)
//! ```
//!
//! ## Key Design Decisions
//!
//! - **Trait-based polymorphism**: All tools implement the [`Tool`] trait,
//!   allowing the registry to dispatch uniformly regardless of tool origin.
//! - **Shared configuration**: [`SharedToolConfig`] is a thread-safe,
//!   cloneable handle to runtime configuration (workspace, exec/timeout
//!   settings, web API keys) that tools snapshot at execution time.
//! - **Dynamic registration**: MCP servers register tools at runtime via
//!   [`ToolRegistry::register_dynamic_tool`]. Builtin names are protected
//!   from accidental override.
//! - **Error recovery**: Tool errors are returned as structured [`ToolError`]
//!   types, which the agent loop surfaces as text to the LLM rather than
//!   aborting the turn.
//!
//! ## Dependencies
//!
//! - `nanobot-types` for shared type definitions (`ToolContext`, `ToolDefinition`,
//!   argument structs).
//! - `nanobot-config` for configuration deserialization.
//! - `nanobot-cron` (optional) for the scheduling backend.
//! - `rmcp` for MCP protocol client support.

pub mod base;
pub mod builtin;
pub mod config;
pub mod cron;
pub mod error;
pub mod filesystem;
pub mod mcp;
pub mod registry;
pub mod registry_builder;
pub mod search;
pub mod shell;
pub mod spawn;
pub mod web;

pub use base::{Tool, ToolContext};
pub use builtin::{BuiltinTool, UnknownToolError};
pub use config::SharedToolConfig;
pub use error::{ToolError, ToolResult};
pub use registry::ToolRegistry;
pub use registry_builder::ToolRegistryBuilder;
pub use spawn::SpawnService;
