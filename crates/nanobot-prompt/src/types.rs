//! Core types for the prompt system

use std::collections::HashMap;

use crate::PromptResult;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Agent prompt composed of multiple sections
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentPrompt {
    /// Base system prompt defining core behavior
    pub system: String,

    /// Role-specific instructions (e.g., "code reviewer", "data analyst")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,

    /// Tool usage guidelines
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<String>,

    /// Context information (workspace, user preferences, etc.)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,

    /// Custom instructions from user
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom: Option<String>,

    /// Metadata
    pub metadata: PromptMetadata,
}

/// Metadata about a prompt
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptMetadata {
    /// Unique name identifier
    pub name: String,

    /// Human-readable description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Version string (e.g., "1.0.0")
    pub version: String,

    /// Author email or name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,

    /// Tags for categorization
    #[serde(default)]
    pub tags: Vec<String>,

    /// Creation timestamp
    pub created_at: DateTime<Utc>,

    /// Last update timestamp
    pub updated_at: DateTime<Utc>,
}

/// Result of prompt validation
#[derive(Debug, Clone)]
pub struct ValidationResult {
    /// Whether the prompt is valid
    pub valid: bool,

    /// Validation errors (must be fixed)
    pub errors: Vec<String>,

    /// Validation warnings (should be reviewed)
    pub warnings: Vec<String>,

    /// Estimated token count
    pub estimated_tokens: usize,
}

/// Configuration for agent prompts
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PromptConfig {
    /// Name of the prompt template to use
    #[serde(skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,

    /// Variables to substitute in the template
    #[serde(default)]
    pub variables: HashMap<String, String>,

    /// Inline system prompt (overrides template)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,

    /// Additional custom instructions
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom: Option<String>,
}

/// Trait for loading and managing prompts
#[async_trait]
pub trait PromptProvider: Send + Sync {
    /// Load a prompt by name
    async fn load(&self, name: &str) -> PromptResult<AgentPrompt>;

    /// Save a prompt
    async fn save(&self, prompt: &AgentPrompt) -> PromptResult<()>;

    /// List available prompts
    async fn list(&self) -> PromptResult<Vec<PromptMetadata>>;

    /// Delete a prompt
    async fn delete(&self, name: &str) -> PromptResult<()>;

    /// Validate a prompt
    fn validate(&self, prompt: &AgentPrompt) -> PromptResult<ValidationResult>;

    /// Render a prompt with variables
    fn render(&self, prompt: &AgentPrompt, vars: &HashMap<String, String>) -> PromptResult<String>;
}
