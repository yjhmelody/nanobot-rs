use async_trait::async_trait;

use crate::agent::skills::SkillInfo;
use crate::error::Result;
use crate::session::SessionManager;
use crate::types::SessionKey;
use crate::types::provider::ChatMessage;

/// Trait for building context and messages for the agent.
#[async_trait]
pub trait ContextProvider: Send + Sync {
    /// Builds the message history for the LLM request.
    ///
    /// This method constructs the complete message array including:
    /// - System prompt
    /// - Historical messages
    /// - Current user message with runtime context
    ///
    /// # Arguments
    ///
    /// * `session_manager` - Session manager for retrieving memory context
    /// * `session_key` - Current session key for memory lookup
    /// * `history` - Previous conversation messages
    /// * `current_message` - The new user message text
    /// * `media` - Optional media attachments (URLs or paths)
    /// * `channel` - Optional channel name for runtime context
    /// * `chat_id` - Optional chat ID for runtime context
    ///
    /// # Returns
    ///
    /// Returns a vector of chat messages ready for the LLM API.
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

/// Trait for the main agent reasoning loop.
///
/// The agent loop is responsible for:
/// - Receiving messages from the message bus
/// - Processing messages through the ReAct (Reasoning + Acting) cycle
/// - Managing concurrent sessions with per-session locks
/// - Coordinating with LLM providers and tool execution
/// - Persisting conversation history
///
/// # Design Principles
///
/// - **Session Isolation**: Each session has its own lock to prevent race conditions
/// - **Concurrent Processing**: Multiple sessions can be processed in parallel
/// - **Error Recovery**: Tool errors are wrapped as text prompts, not fatal
/// - **Resource Management**: Proper cleanup of MCP connections and active tasks
#[async_trait]
pub trait Agent: Send + Sync {
    /// Starts the agent loop.
    ///
    /// This method runs indefinitely, processing messages from the message bus
    /// until `stop()` is called. It should be spawned in a separate task.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let agent = Arc::new(agent_loop);
    /// tokio::spawn(async move {
    ///     agent.run().await;
    /// });
    /// ```
    async fn run(self: std::sync::Arc<Self>);

    /// Stops the agent loop gracefully.
    ///
    /// This method:
    /// - Sets the running flag to false
    /// - Aborts all active tasks
    /// - Clears the task registry
    ///
    /// The `run()` method will exit after processing the current message.
    async fn stop(&self);

    /// Gracefully shuts down the agent.
    ///
    /// This is a convenience method that combines:
    /// 1. `stop()` - Stop the message loop
    /// 2. `close_mcp()` - Close MCP connections
    /// 3. `close_provider()` - Close LLM provider
    ///
    /// Use this for complete shutdown. For more fine-grained control,
    /// call the individual methods separately.
    async fn shutdown(&self) {
        self.stop().await;
        self.close_mcp().await;
        self.close_provider().await;
    }

    /// Processes a message directly without going through the message bus.
    ///
    /// This is useful for:
    /// - Direct API calls (e.g., HTTP gateway)
    /// - Testing
    /// - Synchronous request-response patterns
    ///
    /// # Arguments
    ///
    /// * `content` - The message content
    /// * `session_key` - Session identifier
    /// * `channel` - Channel name (e.g., "cli", "telegram")
    /// * `chat_id` - Chat identifier
    ///
    /// # Returns
    ///
    /// Returns the agent's response text.
    async fn process_direct(
        &self,
        content: &str,
        session_key: &str,
        channel: &str,
        chat_id: &str,
    ) -> Result<String>;

    /// Checks if a session has active tasks.
    ///
    /// # Arguments
    ///
    /// * `session_key` - The session to check
    ///
    /// # Returns
    ///
    /// Returns true if the session has any active tasks.
    fn has_active_tasks(&self, session_key: &SessionKey) -> bool;

    /// Closes MCP (Model Context Protocol) connections.
    ///
    /// Should be called during shutdown to properly clean up resources.
    async fn close_mcp(&self);

    /// Closes the LLM provider connection.
    ///
    /// Should be called during shutdown to properly clean up resources.
    async fn close_provider(&self);
}

/// Trait for providing skills to the agent context.
///
/// This abstraction allows for different implementations:
/// - File-based skills (current implementation)
/// - Cached skills (future)
/// - Remote skills (future)
/// - Composite providers (future)
///
/// # Progressive Disclosure Design
///
/// Skills follow a progressive disclosure pattern to minimize context usage:
///
/// 1. **Discovery Phase** (`build_skills_summary`):
///    - Shows only skill names, descriptions, and availability
///    - Presented as XML summary in system prompt
///    - Agent can see what skills exist without loading full content
///
/// 2. **Loading Phase** (`load_skill`):
///    - Full skill content loaded only when agent explicitly reads the SKILL.md file
///    - Agent uses `read_file` tool to access skill details on-demand
///    - Keeps context window lean until skill is actually needed
///
/// 3. **Always-On Skills** (`get_always_skills`):
///    - Exception to progressive disclosure
///    - Skills marked with `always: true` are loaded into every context
///    - Use sparingly for critical, frequently-needed skills
///
/// This design ensures the agent can discover hundreds of skills without
/// consuming context, loading full details only when relevant.
#[async_trait]
pub trait SkillsProvider: Send + Sync + std::fmt::Debug {
    /// Lists all available skills.
    ///
    /// # Arguments
    ///
    /// * `filter_unavailable` - If true, only return skills whose requirements are met
    ///
    /// # Returns
    ///
    /// Returns a list of skill metadata (name, path, source) without loading full content.
    async fn list_skills(&self, filter_unavailable: bool) -> Vec<SkillInfo>;

    /// Loads the full content of a specific skill.
    ///
    /// This is typically called when the agent uses `read_file` tool to access
    /// a skill's SKILL.md file. The full content is loaded on-demand.
    ///
    /// # Arguments
    ///
    /// * `name` - The skill name
    ///
    /// # Returns
    ///
    /// Returns the complete skill content if found, None otherwise.
    async fn load_skill(&self, name: &str) -> Option<String>;

    /// Gets the list of skills that should always be loaded into context.
    ///
    /// These skills bypass progressive disclosure and are loaded into every
    /// system prompt. Use this sparingly for critical skills that are needed
    /// in most conversations.
    ///
    /// # Returns
    ///
    /// Returns a list of skill names that have `always: true` in their metadata.
    async fn get_always_skills(&self) -> Vec<String>;

    /// Loads multiple skills and formats them for inclusion in context.
    ///
    /// Used for always-on skills that need to be in every system prompt.
    ///
    /// # Arguments
    ///
    /// * `skill_names` - List of skill names to load
    ///
    /// # Returns
    ///
    /// Returns formatted skill content ready for system prompt.
    async fn load_skills_for_context(&self, skill_names: &[String]) -> String;

    /// Builds a lightweight summary of all available skills in XML format.
    ///
    /// This is the discovery mechanism for progressive disclosure. The summary
    /// includes only:
    /// - Skill name
    /// - Brief description
    /// - Availability status (whether requirements are met)
    /// - File location (so agent can use read_file to load full content)
    ///
    /// The agent can see what skills exist without loading their full content,
    /// keeping the context window lean.
    ///
    /// # Returns
    ///
    /// Returns an XML-formatted summary of all skills with their availability status.
    async fn build_skills_summary(&self) -> String;
}

/// Trait for spawning background subagent tasks.
///
/// ```text
/// ToolRegistry → SpawnTool → SpawnService (trait)
/// ```
#[async_trait]
pub trait SpawnService: Send + Sync {
    /// Spawns a background subagent task.
    ///
    /// # Arguments
    ///
    /// * `task` - The task description for the subagent
    /// * `label` - Optional short label for display
    /// * `origin_channel` - Channel where the spawn request originated
    /// * `origin_chat_id` - Chat ID where the spawn request originated
    /// * `session_key` - Optional session key for task tracking
    ///
    /// # Returns
    ///
    /// A message indicating the task was spawned successfully.
    async fn spawn(
        &self,
        task: String,
        label: Option<String>,
        origin_channel: String,
        origin_chat_id: String,
        session_key: Option<SessionKey>,
    ) -> String;

    /// Cancels all tasks associated with a session.
    ///
    /// # Arguments
    ///
    /// * `session_key` - The session key to cancel tasks for
    ///
    /// # Returns
    ///
    /// The number of tasks cancelled.
    async fn cancel_by_session(&self, session_key: &SessionKey) -> Result<usize>;
}
