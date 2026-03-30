//! Agent prompt management system
//!
//! This module provides a flexible system for managing custom agent prompts.
//! Prompts can be loaded from files, validated, and rendered with variable substitution.

mod error;
mod provider;
mod template;
mod types;

pub use error::{PromptError, PromptResult};
pub use provider::FilePromptProvider;
pub use template::TemplateEngine;
pub use types::{AgentPrompt, PromptConfig, PromptMetadata, PromptProvider, ValidationResult};
