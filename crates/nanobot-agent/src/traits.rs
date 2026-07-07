//! Core trait definitions for the agent crate.
//!
//! Defines the three main abstractions in the agent system:
//!
//! * [`ContextProvider`] — Builds system prompts and message histories
//!   for the LLM. The default implementation is [`ContextBuilder`].
//! * [`Agent`] — The top-level agent interface: run, stop, process
//!   messages directly, and manage the lifecycle. The default
//!   implementation is [`AgentLoop`].
//! * [`SkillsProvider`] — Manages skills with progressive disclosure.
//!   The default implementation is [`SkillsLoader`].
//!
//! # Design
//!
//! Following the crate's trait-first architecture, these traits enable
//! loose coupling and testability. Any component can be replaced with a
//! mock (using `mockall`) for unit testing.

use async_trait::async_trait;

use crate::error::AgentResult;
use crate::skills::SkillInfo;
use nanobot_session::SessionManager;
use nanobot_types::SessionKey;
use nanobot_types::provider::ChatMessage;

/// Builds context and messages for the agent's LLM interactions.
///
/// Implementors are responsible for constructing the complete message
/// array that the agent sends to the LLM, including the system prompt,
/// historical messages, the current user message, and any media
/// attachments.
///
/// # Contract
///
/// - Must prepend a system message as the first element.
/// - Must append the current user message as the last element.
/// - Must merge runtime context (timestamp, channel, chat ID) into the
///   user message (not the system prompt) for prompt caching efficiency.
///
/// # Default Implementation
///
/// See [`ContextBuilder`](crate::context::ContextBuilder).
#[async_trait]
#[allow(clippy::too_many_arguments)]
pub trait ContextProvider: Send + Sync {
    /// Builds the complete message history for the next LLM request.
    ///
    /// # Arguments
    ///
    /// * `session_manager` — Session manager for retrieving memory context.
    /// * `session_key` — Current session key for memory lookup.
    /// * `history` — Previous conversation messages from the session.
    /// * `current_message` — The new user message text.
    /// * `media` — Optional media attachments (URLs or file paths).
    /// * `channel` — Optional channel name for runtime-context injection.
    /// * `chat_id` — Optional chat ID for runtime-context injection.
    ///
    /// # Returns
    ///
    /// A vector of [`ChatMessage`]s ready for the LLM API.
    async fn build_messages(
        &self,
        session_manager: &SessionManager,
        session_key: &str,
        history: Vec<ChatMessage>,
        current_message: &str,
        media: Option<&[String]>,
        channel: Option<&str>,
        chat_id: Option<&str>,
    ) -> Vec<ChatMessage>;
}

/// The top-level agent interface.
///
/// An agent listens for inbound messages, processes them through an LLM +
/// tools loop, and publishes outbound responses. Implementors must provide
/// [`run`], [`stop`], [`process_direct`], and lifecycle methods.
///
/// # Default Implementation
///
/// See [`AgentLoop`](crate::loop_core::AgentLoop).
///
/// # Lifecycle
///
/// 1. `Arc<Self>::run()` — Starts the event loop.
/// 2. `stop()` — Signals the loop to stop accepting new messages.
/// 3. `shutdown()` — Gracefully stops the loop, then closes MCP and
///    provider connections.
#[async_trait]
pub trait Agent: Send + Sync {
    /// Starts the inbound-message event loop, blocking until stopped.
    async fn run(self: std::sync::Arc<Self>);

    /// Signals the agent to stop processing new messages.
    async fn stop(&self);

    /// Stops the agent and gracefully shuts down MCP and provider connections.
    ///
    /// Default implementation calls [`stop`], then [`close_mcp`], then
    /// [`close_provider`].
    async fn shutdown(&self) {
        self.stop().await;
        self.close_mcp().await;
        self.close_provider().await;
    }

    /// Processes a single message directly, bypassing the message bus.
    ///
    /// Useful for CLI commands or programmatic invocation.
    ///
    /// # Returns
    ///
    /// The agent's response text.
    ///
    /// # Errors
    ///
    /// Returns [`AgentError`] if message processing fails.
    async fn process_direct(
        &self,
        content: &str,
        session_key: &SessionKey,
        channel: &str,
        chat_id: &str,
    ) -> AgentResult<String>;

    /// Returns `true` if there are in-flight tasks for the given session.
    fn has_active_tasks(&self, session_key: &SessionKey) -> bool;

    /// Closes all MCP server connections.
    async fn close_mcp(&self);

    /// Closes the underlying LLM provider connection (if supported).
    async fn close_provider(&self);
}

/// Manages skills with progressive disclosure.
///
/// Implementors discover available skills, load their content, check
/// requirements, and build summaries for injection into the system prompt.
///
/// # Default Implementation
///
/// See [`SkillsLoader`](crate::skills::SkillsLoader).
#[async_trait]
pub trait SkillsProvider: Send + Sync + std::fmt::Debug {
    /// Lists available skills, optionally excluding those whose
    /// requirements are not met.
    async fn list_skills(&self, filter_unavailable: bool) -> Vec<SkillInfo>;

    /// Loads the full content of a skill by name.
    ///
    /// Returns `None` if the skill does not exist.
    async fn load_skill(&self, name: &str) -> Option<String>;

    /// Returns the names of all skills marked as `always: true`.
    ///
    /// These skills have their full content injected into the system
    /// prompt rather than being listed as a summary.
    async fn get_always_skills(&self) -> Vec<String>;

    /// Loads and concatenates the full content of the named skills for
    /// injection into the system prompt.
    async fn load_skills_for_context(&self, skill_names: &[String]) -> String;

    /// Builds a condensed summary of all available skills.
    ///
    /// The summary is injected into the system prompt to inform the LLM
    /// about available capabilities without consuming excessive tokens.
    async fn build_skills_summary(&self) -> String;
}
