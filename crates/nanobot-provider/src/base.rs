//! Re-exports from [`crate::traits`] for backward compatibility.
//!
//! This module exists to preserve the old public API path (`nanobot_provider::base::*`)
//! after the core trait definitions were moved into `traits.rs`. Consumers can still
//! `use nanobot_provider::base::*` or `use nanobot_provider::{ChatRequest, LLMProvider}`
//! as before.

// Re-export traits for backward compatibility
pub use crate::traits::{ChatRequest, LLMProvider};
