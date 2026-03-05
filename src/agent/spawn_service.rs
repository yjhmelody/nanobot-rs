use async_trait::async_trait;

use crate::error::Result;

/// Trait for spawning background subagent tasks.
///
/// This trait decouples the spawn tool from the concrete SubagentManager implementation,
/// breaking the circular dependency between ToolRegistry and SubagentManager.
///
/// # Architecture
///
/// Before:
/// ```text
/// ToolRegistry → SpawnTool → SubagentManager → ToolRegistry (circular!)
/// ```
///
/// After:
/// ```text
/// ToolRegistry → SpawnTool → SpawnService (trait)
///                                ↑
///                                |
///                         SubagentManager (impl)
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
        session_key: Option<String>,
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
    async fn cancel_by_session(&self, session_key: &str) -> Result<usize>;
}

/// A no-op implementation of SpawnService for testing or when spawn is disabled.
pub struct NoOpSpawnService;

#[async_trait]
impl SpawnService for NoOpSpawnService {
    async fn spawn(
        &self,
        _task: String,
        _label: Option<String>,
        _origin_channel: String,
        _origin_chat_id: String,
        _session_key: Option<String>,
    ) -> String {
        "Spawn service is not available".to_string()
    }

    async fn cancel_by_session(&self, _session_key: &str) -> Result<usize> {
        Ok(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn noop_spawn_service_returns_unavailable_message() {
        let service = NoOpSpawnService;
        let result = service
            .spawn(
                "test".to_string(),
                None,
                "cli".to_string(),
                "direct".to_string(),
                None,
            )
            .await;
        assert!(result.contains("not available"));
    }

    #[tokio::test]
    async fn noop_spawn_service_cancels_zero_tasks() {
        let service = NoOpSpawnService;
        let cancelled = service.cancel_by_session("test").await.unwrap();
        assert_eq!(cancelled, 0);
    }
}
