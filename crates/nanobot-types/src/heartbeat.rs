use serde::{Deserialize, Serialize};

/// Decision payload returned by heartbeat evaluation.
#[derive(Debug, Serialize, Deserialize)]
pub struct HeartbeatDecisionArgs {
    /// Action to take: `"run"`, `"skip"`, or `"stop"`.
    pub action: String,
    /// Optional task description or payload associated with the action.
    #[serde(default)]
    pub tasks: String,
}
