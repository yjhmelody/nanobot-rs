//! Heartbeat decision types for periodic agent evaluation.
//!
//! The heartbeat system periodically evaluates the agent's state and
//! produces a decision about what action to take next. This module
//! defines the data type that carries that decision.
//!
//! # Usage
//!
//! A heartbeat evaluation returns a [`HeartbeatDecisionArgs`] indicating
//! whether the agent should run, skip, or stop. The `tasks` field carries
//! an optional description of what to do.

use serde::{Deserialize, Serialize};

/// Decision payload returned by heartbeat evaluation.
///
/// After evaluating the agent's current state (e.g., time of day, idle
/// duration, or other heuristics), the heartbeat system returns one of
/// these to signal the next action.
///
/// # Fields
///
/// * `action` — One of `"run"`, `"skip"`, or `"stop"`.
///   - `"run"` — Execute the agent turn.
///   - `"skip"` — Do nothing this cycle.
///   - `"stop"` — Stop the heartbeat service entirely.
/// * `tasks` — Optional task description or payload to execute when
///   `action` is `"run"`.
#[derive(Debug, Serialize, Deserialize)]
pub struct HeartbeatDecisionArgs {
    /// Action to take: `"run"`, `"skip"`, or `"stop"`.
    pub action: String,
    /// Optional task description or payload associated with the action.
    #[serde(default)]
    pub tasks: String,
}
