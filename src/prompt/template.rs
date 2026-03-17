//! Simple template engine for variable substitution

use std::borrow::Cow;
use std::collections::HashMap;

use crate::prompt::PromptResult;
use regex::Regex;

/// Template engine for rendering prompts with variables
pub struct TemplateEngine {
    var_regex: Regex,
}

impl TemplateEngine {
    /// Create a new template engine
    pub fn new() -> Self {
        Self {
            var_regex: Regex::new(r"\{\{(\w+)\}\}").expect("invalid regex"),
        }
    }

    /// Render a template with variables
    ///
    /// Supports {{variable}} syntax for variable substitution.
    ///
    /// # Arguments
    ///
    /// * `template` - The template string with {{variable}} placeholders
    /// * `vars` - HashMap of variable names to values
    ///
    /// # Returns
    ///
    /// Rendered string with all variables substituted
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::HashMap;
    /// use nanobot_rs::prompt::TemplateEngine;
    ///
    /// let engine = TemplateEngine::new();
    /// let template = "Hello {{name}}, welcome to {{project}}!";
    ///
    /// let mut vars = HashMap::new();
    /// vars.insert("name".to_string(), "Alice".to_string());
    /// vars.insert("project".to_string(), "nanobot-rs".to_string());
    ///
    /// let result = engine.render(template, &vars).unwrap();
    /// assert_eq!(result, "Hello Alice, welcome to nanobot-rs!");
    /// ```
    pub fn render(&self, template: &str, vars: &HashMap<String, String>) -> PromptResult<String> {
        let result = self
            .var_regex
            .replace_all(template, |caps: &regex::Captures| {
                let var_name = &caps[1];
                if let Some(value) = vars.get(var_name) {
                    Cow::Owned(value.clone())
                } else {
                    Cow::Owned(caps.get(0).unwrap().as_str().to_string())
                }
            });

        Ok(result.to_string())
    }

    /// Extract all variable names from a template
    ///
    /// # Arguments
    ///
    /// * `template` - The template string
    ///
    /// # Returns
    ///
    /// Vector of variable names found in the template
    pub fn extract_variables(&self, template: &str) -> Vec<String> {
        self.var_regex
            .captures_iter(template)
            .map(|cap| cap[1].to_string())
            .collect()
    }

    /// Render a template with environment variables
    ///
    /// Substitutes {{VAR}} with the value of environment variable VAR.
    /// If the environment variable is not set, replaces with empty string.
    ///
    /// # Arguments
    ///
    /// * `template` - The template string with {{variable}} placeholders
    ///
    /// # Returns
    ///
    /// Rendered string with environment variables substituted
    ///
    /// # Examples
    ///
    /// ```no_compile
    /// use nanobot_rs::prompt::TemplateEngine;
    ///
    /// std::env::set_var("API_HOST", "api.example.com");
    ///
    /// let engine = TemplateEngine::new();
    /// let template = "https://{{API_HOST}}/v1";
    /// let result = engine.render_env(template).unwrap();
    /// assert_eq!(result, "https://api.example.com/v1");
    /// ```
    pub fn render_env(&self, template: &str) -> PromptResult<String> {
        let result = self
            .var_regex
            .replace_all(template, |caps: &regex::Captures| {
                let var_name = &caps[1];
                std::env::var(var_name).unwrap_or_default()
            });
        Ok(result.to_string())
    }

    /// Render a JSON value recursively with environment variables
    ///
    /// Traverses the JSON structure and substitutes {{VAR}} placeholders
    /// in all string values with environment variable values.
    ///
    /// # Arguments
    ///
    /// * `value` - Mutable reference to a serde_json::Value
    ///
    /// # Examples
    ///
    /// ```no_compile
    /// use nanobot_rs::prompt::TemplateEngine;
    /// use serde_json::json;
    ///
    /// std::env::set_var("API_KEY", "sk-test-123");
    ///
    /// let engine = TemplateEngine::new();
    /// let mut config = json!({
    ///     "apiKey": "{{API_KEY}}",
    ///     "apiBase": "https://{{API_HOST}}/v1"
    /// });
    ///
    /// engine.render_json_env(&mut config).unwrap();
    /// assert_eq!(config["apiKey"], "sk-test-123");
    /// ```
    pub fn render_json_env(&self, value: &mut serde_json::Value) -> PromptResult<()> {
        match value {
            serde_json::Value::String(s) => {
                *s = self.render_env(s)?;
            }
            serde_json::Value::Array(items) => {
                for item in items {
                    self.render_json_env(item)?;
                }
            }
            serde_json::Value::Object(map) => {
                for item in map.values_mut() {
                    self.render_json_env(item)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
}

impl Default for TemplateEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_simple_variables() {
        let engine = TemplateEngine::new();
        let template = "Hello {{name}}!";

        let mut vars = HashMap::new();
        vars.insert("name".to_string(), "Alice".to_string());

        let result = engine.render(template, &vars).unwrap();
        assert_eq!(result, "Hello Alice!");
    }

    #[test]
    fn test_render_multiple_variables() {
        let engine = TemplateEngine::new();
        let template = "Hello {{name}}, welcome to {{project}}!";

        let mut vars = HashMap::new();
        vars.insert("name".to_string(), "Alice".to_string());
        vars.insert("project".to_string(), "nanobot-rs".to_string());

        let result = engine.render(template, &vars).unwrap();
        assert_eq!(result, "Hello Alice, welcome to nanobot-rs!");
    }

    #[test]
    fn test_render_missing_variable() {
        let engine = TemplateEngine::new();
        let template = "Hello {{name}}, {{missing}} variable!";

        let mut vars = HashMap::new();
        vars.insert("name".to_string(), "Alice".to_string());

        let result = engine.render(template, &vars).unwrap();
        assert_eq!(result, "Hello Alice, {{missing}} variable!");
    }

    #[test]
    fn test_render_no_variables() {
        let engine = TemplateEngine::new();
        let template = "Hello world!";

        let vars = HashMap::new();
        let result = engine.render(template, &vars).unwrap();
        assert_eq!(result, "Hello world!");
    }

    #[test]
    fn test_extract_variables() {
        let engine = TemplateEngine::new();
        let template = "Hello {{name}}, welcome to {{project}}! Your role is {{role}}.";

        let vars = engine.extract_variables(template);
        assert_eq!(vars.len(), 3);
        assert!(vars.contains(&"name".to_string()));
        assert!(vars.contains(&"project".to_string()));
        assert!(vars.contains(&"role".to_string()));
    }

    #[test]
    fn test_extract_variables_empty() {
        let engine = TemplateEngine::new();
        let template = "No variables here!";

        let vars = engine.extract_variables(template);
        assert_eq!(vars.len(), 0);
    }

    #[test]
    fn test_render_multiline_template() {
        let engine = TemplateEngine::new();
        let template = r#"
You are {{role}}.
Project: {{project}}
Language: {{language}}
"#;

        let mut vars = HashMap::new();
        vars.insert("role".to_string(), "code reviewer".to_string());
        vars.insert("project".to_string(), "nanobot-rs".to_string());
        vars.insert("language".to_string(), "Rust".to_string());

        let result = engine.render(template, &vars).unwrap();
        assert!(result.contains("code reviewer"));
        assert!(result.contains("nanobot-rs"));
        assert!(result.contains("Rust"));
    }

    #[test]
    fn test_render_env_substitutes_variables() {
        let engine = TemplateEngine::new();

        unsafe {
            std::env::set_var("TEST_API_HOST", "api.example.com");
            std::env::set_var("TEST_API_PORT", "8080");
        }

        let template = "https://{{TEST_API_HOST}}:{{TEST_API_PORT}}/v1";
        let result = engine.render_env(template).unwrap();
        assert_eq!(result, "https://api.example.com:8080/v1");

        unsafe {
            std::env::remove_var("TEST_API_HOST");
            std::env::remove_var("TEST_API_PORT");
        }
    }

    #[test]
    fn test_render_env_clears_missing_variables() {
        let engine = TemplateEngine::new();

        unsafe {
            std::env::remove_var("TEST_MISSING_VAR");
        }

        let template = "Value: {{TEST_MISSING_VAR}}";
        let result = engine.render_env(template).unwrap();
        assert_eq!(result, "Value: ");
    }

    #[test]
    fn test_render_env_partial_substitution() {
        let engine = TemplateEngine::new();

        unsafe {
            std::env::set_var("TEST_PREFIX", "my");
        }

        let template = "{{TEST_PREFIX}}-api-key-suffix";
        let result = engine.render_env(template).unwrap();
        assert_eq!(result, "my-api-key-suffix");

        unsafe {
            std::env::remove_var("TEST_PREFIX");
        }
    }

    #[test]
    fn test_render_json_env_string_values() {
        let engine = TemplateEngine::new();

        unsafe {
            std::env::set_var("TEST_JSON_KEY", "sk-test-123");
        }

        let mut value = serde_json::json!({
            "apiKey": "{{TEST_JSON_KEY}}"
        });

        engine.render_json_env(&mut value).unwrap();
        assert_eq!(value["apiKey"], "sk-test-123");

        unsafe {
            std::env::remove_var("TEST_JSON_KEY");
        }
    }

    #[test]
    fn test_render_json_env_nested_objects() {
        let engine = TemplateEngine::new();

        unsafe {
            std::env::set_var("TEST_NESTED_HOST", "api.example.com");
            std::env::set_var("TEST_NESTED_KEY", "sk-test-456");
        }

        let mut value = serde_json::json!({
            "providers": {
                "custom": {
                    "apiBase": "https://{{TEST_NESTED_HOST}}/v1",
                    "apiKey": "{{TEST_NESTED_KEY}}"
                }
            }
        });

        engine.render_json_env(&mut value).unwrap();
        assert_eq!(
            value["providers"]["custom"]["apiBase"],
            "https://api.example.com/v1"
        );
        assert_eq!(value["providers"]["custom"]["apiKey"], "sk-test-456");

        unsafe {
            std::env::remove_var("TEST_NESTED_HOST");
            std::env::remove_var("TEST_NESTED_KEY");
        }
    }

    #[test]
    fn test_render_json_env_arrays() {
        let engine = TemplateEngine::new();

        unsafe {
            std::env::set_var("TEST_ARRAY_VAR", "value");
        }

        let mut value = serde_json::json!({
            "items": ["{{TEST_ARRAY_VAR}}", "static", "{{TEST_ARRAY_VAR}}"]
        });

        engine.render_json_env(&mut value).unwrap();
        assert_eq!(value["items"][0], "value");
        assert_eq!(value["items"][1], "static");
        assert_eq!(value["items"][2], "value");

        unsafe {
            std::env::remove_var("TEST_ARRAY_VAR");
        }
    }

    #[test]
    fn test_render_json_env_preserves_non_strings() {
        let engine = TemplateEngine::new();

        let mut value = serde_json::json!({
            "number": 42,
            "boolean": true,
            "null": null,
            "string": "{{TEST_VAR}}"
        });

        engine.render_json_env(&mut value).unwrap();
        assert_eq!(value["number"], 42);
        assert_eq!(value["boolean"], true);
        assert_eq!(value["null"], serde_json::Value::Null);
        assert_eq!(value["string"], "");
    }
}
