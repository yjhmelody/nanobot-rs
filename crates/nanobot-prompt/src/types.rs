//! Core types for the prompt management system.
//!
//! This module defines the fundamental data structures and trait abstractions
//! used throughout the `nanobot-prompt` crate:
//!
//! - `AgentPrompt` — A multi-section prompt with system, role, tools, context,
//!   and custom instruction fields.
//! - `PromptMetadata` — Versioning, authorship, and tagging metadata attached
//!   to each prompt.
//! - `ValidationResult` — The outcome of validating a prompt (errors, warnings,
//!   estimated token count).
//! - `PromptConfig` — A lightweight configuration object that can reference a
//!   template and/or supply inline overrides.
//! - `PromptProvider` — A trait abstracting the storage and retrieval of prompts
//!   (file-based, database-backed, etc.).
//!
//! # Design
//!
//! - All types implement `Serialize`/`Deserialize` for TOML-based persistence.
//! - Optional fields use `Option<String>` with `#[serde(skip_serializing_if)]`
//!   to keep serialized output clean when not set.
//! - `PromptProvider` is `#[async_trait]` so that implementations can use async
//!   I/O (e.g. the `FilePromptProvider` uses `tokio::fs`).
//! - The `PromptConfig` struct is separated from `AgentPrompt` so that
//!   configuration (which template to use, what variables to inject) is distinct
//!   from the prompt content itself.

use std::collections::HashMap;

use crate::PromptResult;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A complete agent prompt composed of multiple optional sections.
///
/// Each prompt has a mandatory `system` field plus optional sections for role
/// instructions, tool guidelines, context information, and custom instructions.
/// The `metadata` field carries versioning and authorship information.
///
/// Fields are serialized to/from TOML using serde. Optional sections that are
/// `None` are omitted from the serialized output.
///
/// # Examples
///
/// ```
/// use nanobot_prompt::{AgentPrompt, PromptMetadata};
/// use chrono::Utc;
///
/// let prompt = AgentPrompt {
///     system: "You are a helpful assistant.".to_string(),
///     role: Some("Your role is code reviewer.".to_string()),
///     tools: None,
///     context: None,
///     custom: None,
///     metadata: PromptMetadata {
///         name: "reviewer".to_string(),
///         description: Some("Code review specialist".to_string()),
///         version: "1.0.0".to_string(),
///         author: None,
///         tags: vec!["review".to_string()],
///         created_at: Utc::now(),
///         updated_at: Utc::now(),
///     },
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentPrompt {
    /// Base system prompt that defines the agent's core behavior and identity.
    ///
    /// This is the only mandatory text field. It sets the overall persona,
    /// constraints, and goals for the agent.
    pub system: String,

    /// Role-specific instructions, e.g. "You are a code reviewer".
    ///
    /// This section is rendered under a `## Role` heading when the prompt is
    /// assembled.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,

    /// Tool usage guidelines describing which tools are available and how they
    /// should be used.
    ///
    /// This section is rendered under a `## Tools` heading when the prompt is
    /// assembled.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<String>,

    /// Context information such as workspace paths, user preferences, or
    /// environment details.
    ///
    /// This section is rendered under a `## Context` heading when the prompt is
    /// assembled.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,

    /// Custom instructions provided by the user to override or augment the
    /// base system prompt for a specific session.
    ///
    /// This section is rendered under a `## Custom Instructions` heading when
    /// the prompt is assembled.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom: Option<String>,

    /// Metadata associated with this prompt (name, version, timestamps, etc.).
    ///
    /// This field is always present and is used for identification and
    /// bookkeeping.
    pub metadata: PromptMetadata,
}

/// Metadata describing a prompt's identity, version, and provenance.
///
/// Attached to every `AgentPrompt` to enable listing, filtering, and
/// version tracking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptMetadata {
    /// Unique name used to identify and reference this prompt.
    ///
    /// The name is used as the filename stem (e.g. `reviewer` → `reviewer.toml`)
    /// in file-based storage and as the lookup key in the cache.
    pub name: String,

    /// Human-readable description of what this prompt is for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Semantic version string (e.g. "1.0.0", "0.2.0-beta").
    ///
    /// While not enforced programmatically, callers should follow semver
    /// conventions to communicate backward-compatibility of prompt changes.
    pub version: String,

    /// Optional author identifier, typically an email address or username.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,

    /// Tags for categorization and discovery (e.g. "code-review", "chat", "debug").
    ///
    /// Defaults to an empty vector when not present in serialized form.
    #[serde(default)]
    pub tags: Vec<String>,

    /// Timestamp of when this prompt was first created.
    pub created_at: DateTime<Utc>,

    /// Timestamp of the most recent modification to this prompt.
    pub updated_at: DateTime<Utc>,
}

/// The result of validating an `AgentPrompt` before use.
///
/// Validation checks structural requirements (non-empty required fields) and
/// produces warnings for potential issues (excessive length, unsubstituted
/// template variables).
#[derive(Debug, Clone)]
pub struct ValidationResult {
    /// Whether the prompt passed validation (no errors).
    ///
    /// A prompt with `valid: false` should not be used.
    pub valid: bool,

    /// Validation errors — issues that must be fixed before the prompt can be used.
    ///
    /// Examples: empty system prompt, missing name, missing version.
    pub errors: Vec<String>,

    /// Validation warnings — issues that should be reviewed but do not block usage.
    ///
    /// Examples: very long prompt, unsubstituted `{{variable}}` placeholders.
    pub warnings: Vec<String>,

    /// Rough estimate of the prompt's token count (1 token ≈ 4 characters).
    ///
    /// This is a heuristic suitable for quick checks; production usage should
    /// prefer a tokenizer-accurate count.
    pub estimated_tokens: usize,
}

/// Configuration specifying which prompt template to use and what variables
/// to inject.
///
/// This struct is used when starting a new agent session to determine the
/// initial system prompt. It can reference a named template, supply an inline
/// system prompt, or both. When both are provided, the inline `system` field
/// takes precedence over the named template.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PromptConfig {
    /// Name of the prompt template to load from storage.
    ///
    /// If `None`, no template is loaded and the inline `system` field (if set)
    /// is used as the sole system prompt.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,

    /// Variables to substitute into the template during rendering.
    ///
    /// Each key must match a `{{variable}}` placeholder in the template text.
    /// Unused variables in the map are silently ignored.
    #[serde(default)]
    pub variables: HashMap<String, String>,

    /// Inline system prompt that overrides or replaces the template's system section.
    ///
    /// When set, this value is used as the system prompt regardless of whether
    /// a template is also configured.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,

    /// Additional custom instructions appended to the rendered prompt.
    ///
    /// These are merged with any `custom` section from the loaded template.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom: Option<String>,
}

/// Trait for loading, saving, listing, deleting, validating, and rendering prompts.
///
/// Implementations provide a storage backend for `AgentPrompt` objects. The
/// crate ships with `FilePromptProvider` (file-based, TOML-serialized), but
/// alternative backends (database, cloud storage, etc.) can be created by
/// implementing this trait.
///
/// All async methods use `#[async_trait]` so they can be called through
/// `dyn PromptProvider` trait objects.
///
/// # Contract for implementors
///
/// - `load` should return an error if the named prompt does not exist.
/// - `save` should persist the prompt and make it immediately available via
///   `load` — i.e. `save(p).await; load(p.name).await` must succeed.
/// - `list` should return metadata for all available prompts; errors loading
///   individual prompts should be logged rather than propagated.
/// - `delete` should remove the prompt from storage; subsequent `load` calls
///   with the same name should fail.
/// - `validate` and `render` are synchronous (they do not touch storage) and
///   should be cheap to call.
///
/// # Examples
///
/// ```
/// use nanobot_prompt::{PromptProvider, AgentPrompt, PromptMetadata, PromptResult};
/// use async_trait::async_trait;
/// use std::collections::HashMap;
///
/// struct MockProvider;
///
/// #[async_trait]
/// impl PromptProvider for MockProvider {
///     async fn load(&self, name: &str) -> PromptResult<AgentPrompt> {
///         unimplemented!()
///     }
///     async fn save(&self, prompt: &AgentPrompt) -> PromptResult<()> {
///         Ok(())
///     }
///     async fn list(&self) -> PromptResult<Vec<PromptMetadata>> {
///         Ok(vec![])
///     }
///     async fn delete(&self, name: &str) -> PromptResult<()> {
///         Ok(())
///     }
///     fn validate(&self, prompt: &AgentPrompt) -> PromptResult<ValidationResult> {
///         unimplemented!()
///     }
///     fn render(&self, prompt: &AgentPrompt, vars: &HashMap<String, String>) -> PromptResult<String> {
///         unimplemented!()
///     }
/// }
/// ```
#[async_trait]
pub trait PromptProvider: Send + Sync {
    /// Load a prompt by name.
    ///
    /// # Arguments
    ///
    /// * `name` — The unique name identifying the prompt (matching
    ///   `PromptMetadata.name`).
    ///
    /// # Returns
    ///
    /// `PromptResult<AgentPrompt>` containing the fully deserialized prompt.
    ///
    /// # Errors
    ///
    /// Returns `PromptError::Message` if the prompt is not found or if the
    /// underlying storage cannot be read.
    async fn load(&self, name: &str) -> PromptResult<AgentPrompt>;

    /// Persist a prompt to storage.
    ///
    /// If a prompt with the same `name` already exists, it is overwritten.
    ///
    /// # Arguments
    ///
    /// * `prompt` — The `AgentPrompt` to save.
    ///
    /// # Errors
    ///
    /// Returns `PromptError::Io` if the storage layer encounters a write error,
    /// or `PromptError::TomlSer` if serialization fails.
    async fn save(&self, prompt: &AgentPrompt) -> PromptResult<()>;

    /// List metadata for all available prompts.
    ///
    /// Implementations should attempt to load every prompt in the store and
    /// return their metadata. Prompts that fail to load should be logged via
    /// `tracing::warn!` rather than causing the entire listing to fail.
    ///
    /// # Returns
    ///
    /// `PromptResult<Vec<PromptMetadata>>` — an (possibly empty) list of
    /// prompt metadata descriptors.
    async fn list(&self) -> PromptResult<Vec<PromptMetadata>>;

    /// Delete a prompt by name.
    ///
    /// # Arguments
    ///
    /// * `name` — The name of the prompt to remove.
    ///
    /// # Errors
    ///
    /// Returns `PromptError::Message` if the prompt does not exist.
    async fn delete(&self, name: &str) -> PromptResult<()>;

    /// Validate a prompt's structure and content.
    ///
    /// This is a synchronous operation — it does not touch the storage layer.
    /// Implementations should check:
    ///
    /// - Required fields are non-empty (system, name, version).
    /// - Token count is within reasonable bounds (heuristic: 1 token ≈ 4 chars).
    /// - No unresolved `{{variable}}` placeholders remain after rendering.
    ///
    /// # Arguments
    ///
    /// * `prompt` — The prompt to validate.
    ///
    /// # Returns
    ///
    /// `PromptResult<ValidationResult>` containing errors, warnings, and an
    /// estimated token count.
    fn validate(&self, prompt: &AgentPrompt) -> PromptResult<ValidationResult>;

    /// Render a prompt by substituting variables into all sections.
    ///
    /// Concatenates the system, role, tools, context, and custom sections
    /// (when present) into a single string, with each optional section
    /// prefixed by a markdown heading.
    ///
    /// # Arguments
    ///
    /// * `prompt` — The prompt to render.
    /// * `vars` — Variables to substitute into `{{variable}}` placeholders.
    ///
    /// # Returns
    ///
    /// `PromptResult<String>` — the fully assembled and substituted prompt text.
    fn render(&self, prompt: &AgentPrompt, vars: &HashMap<String, String>) -> PromptResult<String>;
}
