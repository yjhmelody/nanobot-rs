//! Task identifier type for sub-agent and background task tracking.
//!
//! This module provides [`TaskId`], a newtype wrapper around `uuid::Uuid`
//! used to uniquely identify spawned sub-agents and background operations.
//!
//! # Design
//!
//! - Uses v4 (random) UUIDs for globally unique, non-sequential identifiers.
//! - The `Display` implementation shows an 8-character short form for
//!   concise logging, while [`as_str`](TaskId::as_str) provides the
//!   full UUID string.
//! - Conversions to/from `uuid::Uuid` are provided for interop with
//!   storage backends.

use std::fmt;

use serde::{Deserialize, Serialize};

/// A unique identifier for a spawned sub-agent or background task.
///
/// Wraps a `uuid::Uuid` (v4, random) for global uniqueness. The short
/// display format (first 8 hex chars) is used in user-facing messages
/// to keep identifiers scannable.
///
/// # Derive rationale
///
/// - `Clone + Copy`: small identifier (128 bits) passed by value.
/// - `PartialEq + Eq + Hash`: used as keys in `DashMap` for task lookup.
/// - `Serialize + Deserialize`: stored in task records and session files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TaskId(uuid::Uuid);

impl TaskId {
    /// Generates a new random task ID (UUID v4).
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }

    /// Returns the full UUID string representation (36 characters,
    /// hyphen-separated).
    pub fn as_str(&self) -> String {
        self.0.to_string()
    }

    /// Returns the short (8-character) hex prefix for display in logs
    /// and user-facing messages.
    pub fn short(&self) -> String {
        self.0.to_string().chars().take(8).collect()
    }

    /// Returns the inner `uuid::Uuid` value.
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
