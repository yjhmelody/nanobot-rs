use async_trait::async_trait;

use crate::error::Result;
use crate::types::SessionKey;

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
