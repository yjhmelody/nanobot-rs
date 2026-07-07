//! File-based prompt provider implementation.
//!
//! Provides `FilePromptProvider`, the primary implementation of the
//! `PromptProvider` trait. Prompts are stored as individual `.toml` files in a
//! configurable directory, with an in-memory cache via `DashMap` for fast
//! repeated lookups.
//!
//! # Design
//!
//! - **File layout**: Each prompt is stored as `<name>.toml` in the configured
//!   directory. The filename is sanitised via `safe_filename()` to prevent
//!   path-injection or filesystem-invalid characters in prompt names.
//! - **Cache layer**: A `DashMap` (lock-free concurrent hash map) caches
//!   deserialized prompts after first load. Cache entries are invalidated on
//!   `delete()` and can be selectively cleared via `invalidate_cache()` or
//!   wholesale via `clear_cache()`.
//! - **Async I/O**: All filesystem operations (`read_to_string`, `write`,
//!   `read_dir`, `remove_file`) use `tokio::fs` to avoid blocking the async
//!   runtime.
//! - **Validation**: `validate()` checks required fields, estimates token
//!   count (1 token ≈ 4 characters), and scans for unsubstituted template
//!   variables. It is synchronous and does not touch storage.
//! - **Rendering**: `render()` assembles the final prompt by concatenating
//!   sections, each with a markdown heading. Variable substitution is applied
//!   to every section individually.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::{PromptError, PromptResult};
use async_trait::async_trait;
use dashmap::DashMap;
use tokio::fs;

use super::template::TemplateEngine;
use super::types::{AgentPrompt, PromptMetadata, PromptProvider, ValidationResult};

/// Sanitise a prompt name for use as a filename.
///
/// Replaces characters that are invalid in filenames (`<`, `>`, `:`, `"`, `/`,
/// `\`, `|`, `?`, `*`) with underscores. This prevents path-injection attacks
/// where a prompt name like `../../etc/passwd` could escape the prompts
/// directory.
///
/// The regex is compiled once into a `OnceLock` (lazy static) and reused across
/// all calls for performance.
fn safe_filename(name: &str) -> String {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| regex::Regex::new(r#"[<>:"/\\|?*]"#).expect("invalid regex"));
    re.replace_all(name, "_").trim().to_string()
}

/// File-based prompt provider that persists prompts as TOML files on disk.
///
/// Prompts are stored in a designated directory, one file per prompt. An
/// in-memory cache (`DashMap`) avoids redundant I/O on repeated loads of the
/// same prompt.
///
/// # Examples
///
/// ```
/// use nanobot_prompt::{FilePromptProvider, AgentPrompt, PromptMetadata};
/// use chrono::Utc;
/// use tempfile::tempdir;
///
/// # async fn example() {
/// let dir = tempdir().unwrap();
/// let provider = FilePromptProvider::new(dir.path().to_path_buf()).unwrap();
///
/// let prompt = AgentPrompt {
///     system: "You are a helpful assistant.".to_string(),
///     role: None,
///     tools: None,
///     context: None,
///     custom: None,
///     metadata: PromptMetadata {
///         name: "helper".to_string(),
///         description: None,
///         version: "1.0.0".to_string(),
///         author: None,
///         tags: vec![],
///         created_at: Utc::now(),
///         updated_at: Utc::now(),
///     },
/// };
///
/// provider.save(&prompt).await.unwrap();
/// let loaded = provider.load("helper").await.unwrap();
/// assert_eq!(loaded.system, "You are a helpful assistant.");
/// # }
/// ```
pub struct FilePromptProvider {
    /// Directory in which `.toml` prompt files are stored.
    prompts_dir: PathBuf,
    /// In-memory cache mapping prompt names to deserialized `AgentPrompt`s.
    ///
    /// Uses `DashMap` for lock-free concurrent access across multiple sessions.
    cache: DashMap<String, AgentPrompt>,
    /// Shared template engine used for variable substitution.
    template_engine: Arc<TemplateEngine>,
}

impl FilePromptProvider {
    /// Create a new file-based prompt provider.
    ///
    /// Creates the prompts directory (and all parent directories) if it does
    /// not already exist. The cache and template engine are initialised as
    /// empty.
    ///
    /// # Arguments
    ///
    /// * `prompts_dir` — The directory path where `.toml` prompt files will be
    ///   stored and loaded from.
    ///
    /// # Errors
    ///
    /// Returns `PromptError::Message` if the directory cannot be created (e.g.
    /// due to permission denied on the parent path).
    pub fn new(prompts_dir: PathBuf) -> PromptResult<Self> {
        // Synchronous create_dir_all at construction time — we want the
        // directory to exist before any async operation touches it.
        // This is called once at startup, so blocking briefly is acceptable.
        std::fs::create_dir_all(&prompts_dir).map_err(|e| {
            PromptError::message(format!(
                "failed to create prompts directory: {}: {}",
                prompts_dir.display(),
                e
            ))
        })?;

        Ok(Self {
            prompts_dir,
            cache: DashMap::new(),
            template_engine: Arc::new(TemplateEngine::new()),
        })
    }

    /// Compute the filesystem path for a prompt by name.
    ///
    /// Sanitises the name via `safe_filename()` and appends the `.toml`
    /// extension.
    fn prompt_path(&self, name: &str) -> PathBuf {
        self.prompts_dir
            .join(format!("{}.toml", safe_filename(name)))
    }

    /// Remove the cached entry for a specific prompt.
    ///
    /// The next call to `load` for this name will re-read from disk, picking
    /// up any external modifications to the file.
    pub fn invalidate_cache(&self, name: &str) {
        self.cache.remove(name);
    }

    /// Clear the entire in-memory cache.
    ///
    /// All subsequent `load` calls will re-read prompts from disk.
    pub fn clear_cache(&self) {
        self.cache.clear();
    }
}

#[async_trait]
impl PromptProvider for FilePromptProvider {
    /// Load a prompt by name, consulting the cache first.
    ///
    /// If the prompt is found in the in-memory cache, it is cloned and returned
    /// without touching the filesystem. Otherwise, the corresponding `.toml`
    /// file is read, parsed, cached, and returned.
    ///
    /// The cache uses a `DashMap` with `get()` which holds only a short-lived
    /// read guard — it does not block other concurrent operations.
    async fn load(&self, name: &str) -> PromptResult<AgentPrompt> {
        // Fast path: check the lock-free cache first.
        if let Some(prompt) = self.cache.get(name) {
            return Ok(prompt.clone());
        }

        // Slow path: read from disk, parse the TOML, and cache the result.
        let path = self.prompt_path(name);
        let content = fs::read_to_string(&path).await.map_err(|e| {
            PromptError::message(format!(
                "failed to read prompt file: {}: {}",
                path.display(),
                e
            ))
        })?;

        let prompt: AgentPrompt = toml::from_str(&content).map_err(|e| {
            PromptError::message(format!(
                "failed to parse prompt file: {}: {}",
                path.display(),
                e
            ))
        })?;

        // Insert into cache before returning so future loads hit the fast path.
        self.cache.insert(name.to_string(), prompt.clone());

        Ok(prompt)
    }

    /// Save (create or overwrite) a prompt to disk and update the cache.
    ///
    /// Serialises the prompt to prettified TOML, writes it to
    /// `<prompts_dir>/<name>.toml`, and inserts the prompt into the cache.
    async fn save(&self, prompt: &AgentPrompt) -> PromptResult<()> {
        let path = self.prompt_path(&prompt.metadata.name);
        let content = toml::to_string_pretty(prompt)?;

        // `tokio::fs::write` is an async atomic write (creates/truncates).
        fs::write(&path, content).await.map_err(|e| {
            PromptError::message(format!(
                "failed to write prompt file: {}: {}",
                path.display(),
                e
            ))
        })?;

        // Keep the cache in sync with disk.
        self.cache
            .insert(prompt.metadata.name.clone(), prompt.clone());

        Ok(())
    }

    /// List all available prompts by reading the prompts directory.
    ///
    /// Iterates over `.toml` files in the prompts directory, loads each one,
    /// and collects their metadata. Prompts that fail to load (e.g. corrupt
    /// TOML) are skipped with a warning logged via `tracing::warn!`.
    async fn list(&self) -> PromptResult<Vec<PromptMetadata>> {
        let mut metadata_list = Vec::new();

        let mut entries = fs::read_dir(&self.prompts_dir).await.map_err(|e| {
            PromptError::message(format!(
                "failed to read prompts directory: {}: {}",
                self.prompts_dir.display(),
                e
            ))
        })?;

        // Iterate over directory entries, filtering for `.toml` files.
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("toml")
                && let Some(name) = path.file_stem().and_then(|s| s.to_str())
            {
                match self.load(name).await {
                    Ok(prompt) => metadata_list.push(prompt.metadata),
                    Err(e) => {
                        // Don't fail the entire listing — just log and skip.
                        tracing::warn!("failed to load prompt {}: {}", name, e);
                    }
                }
            }
        }

        Ok(metadata_list)
    }

    /// Delete a prompt by name from disk and cache.
    ///
    /// Removes the `.toml` file from the prompts directory and evicts the
    /// entry from the in-memory cache.
    async fn delete(&self, name: &str) -> PromptResult<()> {
        let path = self.prompt_path(name);

        fs::remove_file(&path).await.map_err(|e| {
            PromptError::message(format!(
                "failed to delete prompt file: {}: {}",
                path.display(),
                e
            ))
        })?;

        // Evict from cache to prevent stale data.
        self.cache.remove(name);

        Ok(())
    }

    /// Validate a prompt's structure and content.
    ///
    /// Checks performed:
    ///
    /// 1. The `system` field is not empty.
    /// 2. The prompt `name` is not empty.
    /// 3. The `version` string is not empty.
    /// 4. The total rendered length is within a reasonable token budget
    ///    (heuristic: one token ≈ 4 characters; warning if > 8000 tokens).
    /// 5. No `{{variable}}` placeholders remain after rendering with an empty
    ///    variable map.
    ///
    /// This method is **synchronous** — it does not perform I/O.
    fn validate(&self, prompt: &AgentPrompt) -> PromptResult<ValidationResult> {
        let mut errors = Vec::new();
        let mut warnings = Vec::new();

        // Check required fields are non-empty.
        if prompt.system.is_empty() {
            errors.push("system prompt cannot be empty".to_string());
        }

        if prompt.metadata.name.is_empty() {
            errors.push("prompt name cannot be empty".to_string());
        }

        if prompt.metadata.version.is_empty() {
            errors.push("prompt version cannot be empty".to_string());
        }

        // Heuristic token estimation: render the prompt with no variables and
        // divide character count by 4. This is a rough approximation — a proper
        // tokenizer should be used for production billing or context management.
        let rendered = self.render(prompt, &HashMap::new())?;
        let estimated_tokens = rendered.len() / 4;

        if estimated_tokens > 8000 {
            warnings.push(format!(
                "prompt is very long (~{} tokens), may exceed context limits",
                estimated_tokens
            ));
        }

        // Scan all prompt text for remaining `{{variable}}` placeholders that
        // would leak into the rendered output because no values were provided.
        let all_text = format!(
            "{} {} {} {} {}",
            prompt.system,
            prompt.role.as_deref().unwrap_or(""),
            prompt.tools.as_deref().unwrap_or(""),
            prompt.context.as_deref().unwrap_or(""),
            prompt.custom.as_deref().unwrap_or("")
        );

        let variables = self.template_engine.extract_variables(&all_text);
        if !variables.is_empty() {
            warnings.push(format!(
                "prompt contains template variables that may not be substituted: {}",
                variables.join(", ")
            ));
        }

        Ok(ValidationResult {
            valid: errors.is_empty(),
            errors,
            warnings,
            estimated_tokens,
        })
    }

    /// Render a full prompt by substituting variables and assembling sections.
    ///
    /// The output is constructed by:
    ///
    /// 1. Rendering the `system` field (no heading — it is the preamble).
    /// 2. Rendering each present optional section under its markdown heading:
    ///    `## Role`, `## Tools`, `## Context`, `## Custom Instructions`.
    /// 3. Joining all sections with newlines.
    ///
    /// Variable substitution is applied to each section individually via the
    /// shared `TemplateEngine`.
    fn render(&self, prompt: &AgentPrompt, vars: &HashMap<String, String>) -> PromptResult<String> {
        let mut sections = Vec::new();

        // System prompt is the preamble — no heading is prepended.
        sections.push(self.template_engine.render(&prompt.system, vars)?);

        // Optional sections each get their own markdown heading.
        if let Some(role) = &prompt.role {
            let rendered = self.template_engine.render(role, vars)?;
            sections.push(format!("\n## Role\n\n{}", rendered));
        }

        if let Some(tools) = &prompt.tools {
            let rendered = self.template_engine.render(tools, vars)?;
            sections.push(format!("\n## Tools\n\n{}", rendered));
        }

        if let Some(context) = &prompt.context {
            let rendered = self.template_engine.render(context, vars)?;
            sections.push(format!("\n## Context\n\n{}", rendered));
        }

        if let Some(custom) = &prompt.custom {
            let rendered = self.template_engine.render(custom, vars)?;
            sections.push(format!("\n## Custom Instructions\n\n{}", rendered));
        }

        Ok(sections.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use tempfile::tempdir;

    fn create_test_prompt(name: &str) -> AgentPrompt {
        AgentPrompt {
            system: "You are a helpful assistant for {{project}}.".to_string(),
            role: Some("Your role is {{role}}.".to_string()),
            tools: None,
            context: Some("Workspace: {{workspace}}".to_string()),
            custom: None,
            metadata: PromptMetadata {
                name: name.to_string(),
                description: Some("Test prompt".to_string()),
                version: "1.0.0".to_string(),
                author: Some("test@example.com".to_string()),
                tags: vec!["test".to_string()],
                created_at: Utc::now(),
                updated_at: Utc::now(),
            },
        }
    }

    #[tokio::test]
    async fn test_save_and_load_prompt() {
        let temp_dir = tempdir().unwrap();
        let provider = FilePromptProvider::new(temp_dir.path().to_path_buf()).unwrap();

        let prompt = create_test_prompt("test-agent");
        provider.save(&prompt).await.unwrap();

        let loaded = provider.load("test-agent").await.unwrap();
        assert_eq!(loaded.system, prompt.system);
        assert_eq!(loaded.metadata.name, prompt.metadata.name);
    }

    #[tokio::test]
    async fn test_cache_works() {
        let temp_dir = tempdir().unwrap();
        let provider = FilePromptProvider::new(temp_dir.path().to_path_buf()).unwrap();

        let prompt = create_test_prompt("cached-agent");
        provider.save(&prompt).await.unwrap();

        // First load - from file
        let loaded1 = provider.load("cached-agent").await.unwrap();

        // Second load - from cache
        let loaded2 = provider.load("cached-agent").await.unwrap();

        assert_eq!(loaded1.system, loaded2.system);
    }

    #[tokio::test]
    async fn test_list_prompts() {
        let temp_dir = tempdir().unwrap();
        let provider = FilePromptProvider::new(temp_dir.path().to_path_buf()).unwrap();

        provider.save(&create_test_prompt("agent1")).await.unwrap();
        provider.save(&create_test_prompt("agent2")).await.unwrap();

        let list = provider.list().await.unwrap();
        assert_eq!(list.len(), 2);
    }

    #[tokio::test]
    async fn test_delete_prompt() {
        let temp_dir = tempdir().unwrap();
        let provider = FilePromptProvider::new(temp_dir.path().to_path_buf()).unwrap();

        let prompt = create_test_prompt("to-delete");
        provider.save(&prompt).await.unwrap();

        provider.delete("to-delete").await.unwrap();

        let result = provider.load("to-delete").await;
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_valid_prompt() {
        let temp_dir = tempdir().unwrap();
        let provider = FilePromptProvider::new(temp_dir.path().to_path_buf()).unwrap();

        let prompt = create_test_prompt("valid");
        let validation = provider.validate(&prompt).unwrap();

        assert!(validation.valid);
        assert!(validation.errors.is_empty());
    }

    #[test]
    fn test_validate_empty_system_prompt() {
        let temp_dir = tempdir().unwrap();
        let provider = FilePromptProvider::new(temp_dir.path().to_path_buf()).unwrap();

        let mut prompt = create_test_prompt("invalid");
        prompt.system = "".to_string();

        let validation = provider.validate(&prompt).unwrap();

        assert!(!validation.valid);
        assert!(!validation.errors.is_empty());
    }

    #[test]
    fn test_render_with_variables() {
        let temp_dir = tempdir().unwrap();
        let provider = FilePromptProvider::new(temp_dir.path().to_path_buf()).unwrap();

        let prompt = create_test_prompt("render-test");

        let mut vars = HashMap::new();
        vars.insert("project".to_string(), "nanobot".to_string());
        vars.insert("role".to_string(), "code reviewer".to_string());
        vars.insert("workspace".to_string(), "/home/user/project".to_string());

        let rendered = provider.render(&prompt, &vars).unwrap();

        assert!(rendered.contains("nanobot"));
        assert!(rendered.contains("code reviewer"));
        assert!(rendered.contains("/home/user/project"));
    }

    #[test]
    fn test_render_without_variables() {
        let temp_dir = tempdir().unwrap();
        let provider = FilePromptProvider::new(temp_dir.path().to_path_buf()).unwrap();

        let prompt = create_test_prompt("render-test");
        let vars = HashMap::new();

        let rendered = provider.render(&prompt, &vars).unwrap();

        // Variables should remain unsubstituted
        assert!(rendered.contains("{{project}}"));
        assert!(rendered.contains("{{role}}"));
    }

    #[test]
    fn test_invalidate_cache() {
        let temp_dir = tempdir().unwrap();
        let provider = FilePromptProvider::new(temp_dir.path().to_path_buf()).unwrap();

        let prompt = create_test_prompt("cache-test");
        provider.cache.insert("cache-test".to_string(), prompt);

        assert!(provider.cache.contains_key("cache-test"));

        provider.invalidate_cache("cache-test");

        assert!(!provider.cache.contains_key("cache-test"));
    }
}
