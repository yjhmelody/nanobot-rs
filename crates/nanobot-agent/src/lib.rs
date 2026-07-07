//! Crate-level agent logic for the nanobot framework.
//!
//! This crate provides the core agent loop, context assembly, retrieval,
//! skills loading, subagent management, and the ReAct (Reason-Act-Observe)
//! execution engine. It depends on `nanobot-bus` for message passing,
//! `nanobot-provider` for LLM calls, `nanobot-tools` for tool execution,
//! `nanobot-session` for conversation persistence, and `nanobot-config`
//! for runtime configuration.
//!
//! # Architecture
//!
//! The entry point is [`AgentLoop`], which listens for inbound messages on a
//! [`MessageBus`](nanobot_bus::MessageBus), dispatches them to a per-session
//! handler, runs the ReAct loop, and publishes outbound responses. Context
//! assembly is handled by [`ContextBuilder`], retrieval by
//! [`RetrievalService`], skills by [`SkillsLoader`], and subagents by
//! [`SubagentManager`].
//!
//! # Key Design Decisions
//!
//! - **Trait-first**: Major components implement traits defined in
//!   [`traits`], making them swappable for testing or alternate backends.
//! - **Session isolation**: Concurrent messages for the same session are
//!   serialized via per-session locks.
//! - **Progressive disclosure**: Skills are loaded on-demand and summarized
//!   to minimise context-window usage.
//! - **ReAct pattern**: The core loop cycles through plan-act-observe until
//!   the LLM produces a final answer or the iteration limit is reached.

pub mod builder;
pub mod context;
pub mod error;
pub mod loop_core;
pub mod react;
pub mod retrieval;
pub mod skills;
pub mod subagent;
pub mod traits;
pub mod utils;

pub use self::builder::{AgentConfig, AgentLoopBuilder};
pub use self::context::ContextBuilder;
pub use self::error::{AgentError, AgentResult};
pub use self::loop_core::AgentLoop;
pub use self::react::{ExecutionContext, LoopExitReason, LoopOutcome, ReActExecutor};
pub use self::retrieval::{RetrievalService, RetrievedContext};
pub use self::skills::SkillsLoader;
pub use self::subagent::SubagentManager;
pub use self::traits::{Agent, ContextProvider, SkillsProvider};
