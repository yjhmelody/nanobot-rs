//! ACP configuration types.
//!
//! This module re-exports the ACP configuration structures (`ACPConfig`,
//! `AgentConfig`) defined in the `nanobot-config` crate. Keeping them
//! accessible from `crate::acp::config` avoids deep import paths for
//! consumers within this crate.
//!
//! See `nanobot_config::acp` for the canonical definitions.

pub use nanobot_config::acp::{ACPConfig, AgentConfig};
