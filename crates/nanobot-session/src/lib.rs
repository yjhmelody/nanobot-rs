//! Session management crate for the nanobot AI agent framework.
//!
//! This crate provides the core session lifecycle, persistence, and memory infrastructure.
//! It enables nanobot agents to maintain conversational context across multiple turns,
//! persist session state to disk, consolidate (compress) long conversations, retrieve
//! long-term memory, and transform or filter message history before it reaches the LLM.
//!
//! # Architecture
//!
//! The crate is organised around several trait abstractions, each with one or more
//! concrete implementations:
//!
//! | Trait | Role | Built-in implementations |
//! |-------|------|--------------------------|
//! | [`SessionStore`] | Persistence backend for sessions | [`JsonlSessionStore`] (file-based), [`InMemorySessionStore`] (testing) |
//! | [`ConsolidationStrategy`] | Session compression / summarisation | [`LlmConsolidationStrategy`] (LLM-based) |
//! | [`MemoryProvider`] | Long-term memory retrieval and storage | [`FileMemoryProvider`], [`CompositeMemoryProvider`] |
//! | [`HistoryTransformer`] | Pre-LLM message pipeline transforms | [`SensitiveDataFilter`], [`MetadataAnnotator`] |
//! | [`SessionHook`] | Session lifecycle event hooks | [`LoggingHook`] |
//!
//! The [`SessionManager`] is the top-level orchestrator, composing all the above traits
//! into a single callable interface that the agent loop interacts with.
//!
//! # Key Design Decisions
//!
//! - **Trait-first**: All major components are defined as traits before implementations,
//!   allowing alternate backends (SQLite, Redis, etc.) to be plugged in without modifying
//!   existing code.
//! - **JSONL persistence**: Sessions are stored as newline-delimited JSON, which is
//!   human-readable, append-friendly, and easy to debug with standard Unix tools.
//! - **DashMap caching**: The file store uses a [`DashMap`](dashmap::DashMap) for concurrent
//!   read-heavy cache access without blocking on a single lock.
//! - **Sliding window history**: [`Session::get_history`] returns only the most recent
//!   `max_messages` unconsolidated entries, back-tracking to include the first user
//!   message in the window to avoid cutting an assistant reply mid-stream.
//! - **Dual-layer memory**: Long-term memory (MEMORY.md) stores cross-session knowledge,
//!   while the history log (HISTORY.md) is an append-only event log for debugging and
//!   reference.
//!
//! # Session Lifecycle
//!
//! 1. `get_or_create` -- load from store or create new
//! 2. Messages are added by the agent loop (out-of-crate)
//! 3. `save` -- persist with optional consolidation and hook callbacks
//! 4. `delete` -- remove session and notify hooks
//!
//! # Module Overview
//!
//! - [`traits`] -- All trait definitions
//! - [`types`] -- Core data types: `Session`, `SessionEntry`, `SessionSummary`, etc.
//! - [`session_manager`] -- Orchestrator combining store, consolidation, memory, transformers, hooks
//! - [`session_store`] -- Persistence backends (JSONL + in-memory)
//! - [`consolidation_strategy`] -- LLM-based session compression
//! - [`memory_provider`] -- Long-term memory providers (file, composite)
//! - [`memory_store`] -- File-backed memory store (MEMORY.md / HISTORY.md)
//! - [`transformer`] -- Pre-LLM message transformers
//! - [`session_hook`] -- Lifecycle hook implementations (logging)
//! - [`helpers`] -- Utility functions (directory creation, filename sanitisation)
//! - [`error`] -- Error types
//!
//! [`SessionStore`]: traits::SessionStore
//! [`ConsolidationStrategy`]: traits::ConsolidationStrategy
//! [`MemoryProvider`]: traits::MemoryProvider
//! [`HistoryTransformer`]: traits::HistoryTransformer
//! [`SessionHook`]: traits::SessionHook
//! [`SessionManager`]: session_manager::SessionManager
//! [`JsonlSessionStore`]: session_store::JsonlSessionStore
//! [`InMemorySessionStore`]: session_store::InMemorySessionStore
//! [`LlmConsolidationStrategy`]: consolidation_strategy::LlmConsolidationStrategy
//! [`FileMemoryProvider`]: memory_provider::FileMemoryProvider
//! [`CompositeMemoryProvider`]: memory_provider::CompositeMemoryProvider
//! [`SensitiveDataFilter`]: transformer::SensitiveDataFilter
//! [`MetadataAnnotator`]: transformer::MetadataAnnotator
//! [`LoggingHook`]: session_hook::LoggingHook
pub mod consolidation_strategy;
pub mod error;
pub mod helpers;
pub mod memory_provider;
pub mod memory_store;
pub mod session_hook;
pub mod session_manager;
pub mod session_store;
pub mod traits;
pub mod transformer;
pub mod types;

pub use self::consolidation_strategy::*;
pub use self::error::{SessionError, SessionResult};
pub use self::memory_provider::*;
pub use self::memory_store::*;
pub use self::session_hook::*;
pub use self::session_manager::*;
pub use self::session_store::*;
pub use self::traits::*;
pub use self::transformer::*;
pub use self::types::*;
