use serde::{Deserialize, Serialize};

/// Decision payload returned by heartbeat evaluation.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct HeartbeatDecisionArgs {
    pub(crate) action: String,
    #[serde(default)]
    pub(crate) tasks: String,
}
