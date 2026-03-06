use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct HeartbeatDecisionArgs {
    pub(crate) action: String,
    #[serde(default)]
    pub(crate) tasks: String,
}
