use std::fmt;

use serde::{Deserialize, Serialize};

/// A task identifier wrapping a UUID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TaskId(uuid::Uuid);

impl TaskId {
    /// Generate a new random task ID.
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }

    /// Get the full UUID string representation.
    pub fn as_str(&self) -> String {
        self.0.to_string()
    }

    /// Get the short (8-character) representation for display.
    pub fn short(&self) -> String {
        self.0.to_string().chars().take(8).collect()
    }

    /// Get the inner UUID.
    pub fn inner(&self) -> uuid::Uuid {
        self.0
    }
}

impl Default for TaskId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for TaskId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.short())
    }
}

impl From<uuid::Uuid> for TaskId {
    fn from(uuid: uuid::Uuid) -> Self {
        Self(uuid)
    }
}

impl From<TaskId> for uuid::Uuid {
    fn from(task_id: TaskId) -> Self {
        task_id.0
    }
}
