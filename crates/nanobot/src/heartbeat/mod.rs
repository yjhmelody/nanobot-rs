//! Heartbeat subsystem for periodic LLM-driven task review and execution.
//!
//! The heartbeat service periodically reads the workspace `HEARTBEAT.md` file,
//! asks the LLM whether there are active tasks, and — if the LLM decides to
//! "run" — delegates task execution to a registered `HeartbeatExecuteHandler`
//! and optionally notifies via `HeartbeatNotifyHandler`.
//!
//! ## Flow
//!
//! 1. `HeartbeatService::start` spawns a background loop with a configurable
//!    interval (typically 30-60 seconds).
//! 2. Each tick reads `HEARTBEAT.md` and sends it to the LLM with a prompt
//!    that asks for a JSON decision: `{"action":"run|skip","tasks":"..."}`.
//! 3. If action is `run`, the `HeartbeatExecuteHandler` is called with the
//!    task string. The result is forwarded to `HeartbeatNotifyHandler`.
//!
//! ## Sub-modules
//!
//! | Module | Purpose |
//! |--------|---------|
//! | `error` | Error types for the heartbeat subsystem |
//! | `service` | Core `HeartbeatService` and handler trait definitions |

pub mod error;
pub mod service;

pub use error::{HeartbeatError, HeartbeatResult};
pub use service::{HeartbeatExecuteHandler, HeartbeatNotifyHandler, HeartbeatService};
