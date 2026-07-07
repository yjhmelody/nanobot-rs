//! ACP (Agent Client Protocol) integration for delegating coding tasks to
//! external agent processes.
//!
//! ## Overview
//!
//! The ACP module allows nanobot to delegate complex, multi-step tasks (e.g.,
//! refactoring, feature implementation) to external ACP-compatible agent
//! processes such as `codex-acp`. Communication follows the
//! [Agent Client Protocol](https://github.com/anthropics/agent-client-protocol)
//! specification.
//!
//! ## Sub-modules
//!
//! | Module | Purpose |
//! |--------|---------|
//! | `client` | High-level client that spawns an ACP subprocess and manages its lifecycle |
//! | `config` | Re-export of ACP configuration types from `nanobot-config` |
//! | `simple_client` | `Client` trait implementation for the official ACP Rust SDK |
//! | `tool` | `Tool` trait implementation exposing ACP execution as a tool callable by the LLM |
//!
//! ## Data Flow
//!
//! 1. `ACPTool::execute` receives arguments from the LLM (agent_id, task, cwd).
//! 2. `ACPClient::spawn` launches the ACP subprocess and runs an actor thread.
//! 3. The actor thread performs the ACP handshake (initialize, new_session).
//! 4. `ACPClient::execute` sends a prompt request and streams back the result.
//! 5. `ACPClient::close` shuts down the subprocess cleanly.
//!
//! ## Threading Model
//!
//! A dedicated OS thread runs the ACP actor (with its own tokio `LocalSet`)
//! so that the CPU-heavy protocol message loop does not block the main
//! async runtime. Inter-thread communication uses `mpsc`/`oneshot` channels.

pub mod client;
pub mod config;
pub mod simple_client;
pub mod tool;

pub use client::build_acp_command;
pub use nanobot_config::acp::ACPConfig;
pub use tool::ACPTool;
