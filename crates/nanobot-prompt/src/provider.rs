//! File-based prompt provider implementation

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::{PromptError, PromptResult};
use async_trait::async_trait;
use dashmap::DashMap;
use tokio::fs;

use super::template::TemplateEngine;
use super::types::{AgentPrompt, PromptMetadata, PromptProvider, ValidationResult};

fn safe_filename(name: &str) -> String {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| regex::Regex::new(r#"[<>:"/\\|?*]"#).expect("invalid regex"));
    re.replace_all(name, "_").trim().to_string()
}

/// File-based prompt provider
///
/// Stores prompts as TOML files in a directory and caches them in memory.
pub struct FilePromptProvider {
    prompts_dir: PathBuf,
    cache: DashMap<String, AgentPrompt>,
    template_engine: Arc<TemplateEngine>,
}

impl FilePromptProvider {
    /// Create a new file-based prompt provider
    ///
    /// # Arguments
    ///
    /// * `prompts_dir` - Directory to store prompt files
    pub fn new(prompts_dir: PathBuf) -> PromptResult<Self> {
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

    /// Get the file path for a prompt
    fn prompt_path(&self, name: &str) -> PathBuf {
        self.prompts_dir.join(format!("{}.toml", safe_filename(name)))
    }

    /// Clear the cache for a specific prompt
    pub fn invalidate_cache(&self, name: &str) {
        self.cache.remove(name);
    }

    /// Clear all cached prompts
    pub fn clear_cache(&self) {
        self.cache.clear();
    }
}

#[async_trait]
impl PromptProvider for FilePromptProvider {
    async fn load(&self, name: &str) -> PromptResult<AgentPrompt> {
        // Check cache first
        if let Some(prompt) = self.cache.get(name) {
            return Ok(prompt.clone());
        }

        // Load from file
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

        // Cache it
        self.cache.insert(name.to_string(), prompt.clone());

        Ok(prompt)
    }

    async fn save(&self, prompt: &AgentPrompt) -> PromptResult<()> {
        let path = self.prompt_path(&prompt.metadata.name);
        let content = toml::to_string_pretty(prompt)?;

        fs::write(&path, content).await.map_err(|e| {
            PromptError::message(format!(
                "failed to write prompt file: {}: {}",
                path.display(),
                e
            ))
        })?;

        // Update cache
        self.cache.insert(prompt.metadata.name.clone(), prompt.clone());

        Ok(())
    }

    async fn list(&self) -> PromptResult<Vec<PromptMetadata>> {
        let mut metadata_list = Vec::new();

        let mut entries = fs::read_dir(&self.prompts_dir).await.map_err(|e| {
            PromptError::message(format!(
                "failed to read prompts directory: {}: {}",
                self.prompts_dir.display(),
                e
            ))
        })?;

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("toml") {
                if let Some(name) = path.file_stem().and_then(|s| s.to_str()) {
                    match self.load(name).await {
                        Ok(prompt) => metadata_list.push(prompt.metadata),
                        Err(e) => {
                            tracing::warn!("failed to load prompt {}: {}", name, e);
                        }
                    }
                }
            }
        }

        Ok(metadata_list)
    }

    async fn delete(&self, name: &str) -> PromptResult<()> {
        let path = self.prompt_path(name);

        fs::remove_file(&path).await.map_err(|e| {
            PromptError::message(format!(
                "failed to delete prompt file: {}: {}",
                path.display(),
                e
            ))
        })?;

        // Remove from cache
        self.cache.remove(name);

        Ok(())
    }

    fn validate(&self, prompt: &AgentPrompt) -> PromptResult<ValidationResult> {
        let mut errors = Vec::new();
        let mut warnings = Vec::new();

        // Check required fields
        if prompt.system.is_empty() {
            errors.push("system prompt cannot be empty".to_string());
        }

        if prompt.metadata.name.is_empty() {
            errors.push("prompt name cannot be empty".to_string());
        }

        if prompt.metadata.version.is_empty() {
            errors.push("prompt version cannot be empty".to_string());
        }

        // Estimate token count
        let rendered = self.render(prompt, &HashMap::new())?;
        let estimated_tokens = rendered.len() / 4; // Rough estimate: 1 token ≈ 4 chars

        if estimated_tokens > 8000 {
            warnings.push(format!(
                "prompt is very long (~{} tokens), may exceed context limits",
                estimated_tokens
            ));
        }

        // Check for unsubstituted variables
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

    fn render(&self, prompt: &AgentPrompt, vars: &HashMap<String, String>) -> PromptResult<String> {
        let mut sections = Vec::new();

        // Render system prompt
        sections.push(self.template_engine.render(&prompt.system, vars)?);

        // Render optional sections
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
        vars.insert("project".to_string(), "nanobot-rs".to_string());
        vars.insert("role".to_string(), "code reviewer".to_string());
        vars.insert("workspace".to_string(), "/home/user/project".to_string());

        let rendered = provider.render(&prompt, &vars).unwrap();

        assert!(rendered.contains("nanobot-rs"));
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
