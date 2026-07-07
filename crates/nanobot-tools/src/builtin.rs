//! Convenience re-exports of built-in tool identifiers from `nanobot-types`.
//!
//! This thin module re-exports [`BuiltinTool`] and [`UnknownToolError`]
//! so that consumers of this crate do not need to depend on `nanobot-types`
//! directly just for these types.

/// Re-exports from `nanobot-types` for convenience.
pub use nanobot_types::builtin::{BuiltinTool, UnknownToolError};
