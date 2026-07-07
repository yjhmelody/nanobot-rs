//! Agent prompt management system
//!
//! This crate provides a flexible system for managing custom agent prompts used
//! by the nanobot AI agent framework. Prompts can be loaded from TOML files,
//! validated for correctness, and rendered with variable substitution.
//!
//! # Architecture
//!
//! The crate is organized into four internal modules:
//!
//! - `types` — Core domain types: `AgentPrompt`, `PromptMetadata`, `PromptConfig`,
//!   `ValidationResult`, and the `PromptProvider` trait.
//! - `provider` — The `FilePromptProvider` implementation that persists prompts as
//!   TOML files on disk, with an in-memory `DashMap` cache for fast lookups.
//! - `template` — The `TemplateEngine` that performs `{{variable}}` substitution
//!   using regex-based replacement, with support for both explicit variable maps
//!   and environment variable resolution.
//! - `error` — The `PromptError` enum and `PromptResult` alias used throughout.
//!
//! # Design Decisions
//!
//! - **File-backed storage**: Prompts are stored as individual `.toml` files in a
//!   designated directory, making them version-controllable and easy to edit
//!   outside the application.
//! - **Lazy caching**: Prompts are cached on first load via `DashMap`, which
//!   provides lock-free concurrent reads across sessions.
//! - **Idempotent rendering**: The template engine leaves unresolved variables
//!   in place (e.g. `{{unknown}}`) rather than raising an error, allowing
//!   partial substitution.
//! - **Validation with warnings**: Validation checks for empty required fields,
//!   excessive token estimates, and unsubstituted template variables, returning
//!   both errors and warnings.
//!
//! # Status
//!
//! TODO: this module is still in early development and subject to change.
mod error;
mod provider;
mod template;
mod types;

pub use error::{PromptError, PromptResult};
pub use provider::FilePromptProvider;
pub use template::TemplateEngine;
pub use types::{AgentPrompt, PromptConfig, PromptMetadata, PromptProvider, ValidationResult};
