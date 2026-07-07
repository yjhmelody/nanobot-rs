//! Simple template engine for variable substitution in prompts.
//!
//! Provides the `TemplateEngine` which supports:
//!
//! - `{{variable}}` placeholder syntax for substitution from a `HashMap`.
//! - Environment variable resolution via `render_env`.
//! - Recursive environment variable substitution in `serde_json::Value` trees
//!   via `render_json_env`.
//!
//! # Design
//!
//! - The regex `\{\{(\w+)\}\}` is compiled once in `new()` and reused across
//!   all render calls for performance.
//! - Unresolved variables are kept verbatim (e.g. `{{unknown}}` stays as-is)
//!   rather than erroring, making partial substitution safe.
//! - `render_json_env` walks the JSON tree recursively, mutating only string
//!   leaves. Non-string values are left untouched, which avoids unnecessary
//!   allocations.

use std::borrow::Cow;
use std::collections::HashMap;

use crate::PromptResult;
use regex::Regex;

/// Template engine for rendering prompts with variable substitution.
///
/// Uses a compiled regex (`\{\{(\w+)\}\}`) to find `{{variable}}` placeholders
/// in template strings and replace them with provided values.
///
/// # Examples
///
/// ```
/// use std::collections::HashMap;
/// use nanobot_prompt::TemplateEngine;
///
/// let engine = TemplateEngine::new();
/// let template = "Hello {{name}}, welcome to {{project}}!";
///
/// let mut vars = HashMap::new();
/// vars.insert("name".to_string(), "Alice".to_string());
/// vars.insert("project".to_string(), "nanobot".to_string());
///
/// let result = engine.render(template, &vars).unwrap();
/// assert_eq!(result, "Hello Alice, welcome to nanobot!");
/// ```
pub struct TemplateEngine {
    /// Compiled regex matching `{{variable}}` patterns.
    ///
    /// Capture group 1 isolates the variable name for lookup.
    var_regex: Regex,
}

impl TemplateEngine {
    /// Create a new template engine with the default `{{variable}}` syntax.
    ///
    /// The regex pattern `\{\{(\w+)\}\}` matches word characters only between
    /// double curly braces, so `{{foo_bar}}` is valid but `{{foo bar}}` is not.
    ///
    /// # Panics
    ///
    /// Panics if the regex pattern is invalid. Since the pattern is a compile-time
    /// constant, this panic will always occur at startup if the regex is malformed.
    pub fn new() -> Self {
        Self {
            var_regex: Regex::new(r"\{\{(\w+)\}\}").expect("invalid regex"),
        }
    }

    /// Render a template string by substituting `{{variable}}` placeholders.
    ///
    /// For each placeholder found, the engine looks up the variable name in
    /// `vars`. If found, it is replaced with the corresponding value. If not
    /// found, the original placeholder (e.g. `{{unknown}}`) is left unchanged.
    ///
    /// # Arguments
    ///
    /// * `template` - The template string containing `{{variable}}` placeholders.
    /// * `vars` - A map of variable names to their substitution values.
    ///
    /// # Returns
    ///
    /// `PromptResult<String>` containing the template with all known variables
    /// substituted. This method is infallible under normal operation — errors
    /// only occur if the regex replacement itself fails.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::HashMap;
    /// use nanobot_prompt::TemplateEngine;
    ///
    /// let engine = TemplateEngine::new();
    /// let mut vars = HashMap::new();
    /// vars.insert("name".to_string(), "Alice".to_string());
    ///
    /// let result = engine.render("Hello {{name}}!", &vars).unwrap();
    /// assert_eq!(result, "Hello Alice!");
    /// ```
    pub fn render(&self, template: &str, vars: &HashMap<String, String>) -> PromptResult<String> {
        // Use replace_all with a closure to avoid building an intermediate
        // iterator of matches. Each match is resolved immediately.
        let result = self
            .var_regex
            .replace_all(template, |caps: &regex::Captures| {
                let var_name = &caps[1];
                // Borrow from vars; if the key is missing, return the original
                // match text (caps[0]) so the placeholder survives.
                if let Some(value) = vars.get(var_name) {
                    Cow::Owned(value.clone())
                } else {
                    Cow::Owned(caps.get(0).unwrap().as_str().to_string())
                }
            });

        Ok(result.to_string())
    }

    /// Extract all unique variable names from a template string.
    ///
    /// Scans the template for `{{variable}}` patterns and returns the variable
    /// names in the order they appear (duplicates are not deduplicated; callers
    /// that need uniqueness should collect into a `HashSet`).
    ///
    /// # Arguments
    ///
    /// * `template` - The template string to scan.
    ///
    /// # Returns
    ///
    /// A `Vec<String>` of variable names found.
    ///
    /// # Examples
    ///
    /// ```
    /// use nanobot_prompt::TemplateEngine;
    ///
    /// let engine = TemplateEngine::new();
    /// let vars = engine.extract_variables("Hello {{name}}, welcome to {{project}}!");
    /// assert_eq!(vars, vec!["name", "project"]);
    /// ```
    pub fn extract_variables(&self, template: &str) -> Vec<String> {
        self.var_regex
            .captures_iter(template)
            .map(|cap| cap[1].to_string())
            .collect()
    }

    /// Render a template string substituting `{{VAR}}` with environment variables.
    ///
    /// For each `{{variable}}` placeholder, the engine reads the environment
    /// variable with that name. If the environment variable is set, it replaces
    /// the placeholder with its value. If unset, the placeholder is replaced
    /// with an empty string.
    ///
    /// # Arguments
    ///
    /// * `template` - The template string with `{{variable}}` placeholders.
    ///
    /// # Returns
    ///
    /// `PromptResult<String>` containing the rendered template.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use nanobot_prompt::TemplateEngine;
    /// std::env::set_var("API_HOST", "api.example.com");
    ///
    /// let engine = TemplateEngine::new();
    /// let result = engine.render_env("https://{{API_HOST}}/v1").unwrap();
    /// assert_eq!(result, "https://api.example.com/v1");
    /// ```
    pub fn render_env(&self, template: &str) -> PromptResult<String> {
        let result = self
            .var_regex
            .replace_all(template, |caps: &regex::Captures| {
                let var_name = &caps[1];
                // Replace missing environment variables with empty string
                // rather than leaving the placeholder — secrets/tokens that
                // reference env vars should not leak the template syntax.
                std::env::var(var_name).unwrap_or_default()
            });
        Ok(result.to_string())
    }

    /// Recursively substitute `{{VAR}}` placeholders with environment variables
    /// in a `serde_json::Value` tree.
    ///
    /// Walks every node in the JSON value:
    /// - String values are rendered through `render_env`.
    /// - Arrays have each element processed recursively.
    /// - Objects have each value processed recursively.
    /// - Numbers, booleans, and nulls are left unchanged.
    ///
    /// # Arguments
    ///
    /// * `value` - A mutable reference to a `serde_json::Value` whose string
    ///   leaves will be mutated in place.
    ///
    /// # Returns
    ///
    /// `PromptResult<()>`. Errors are propagated from `render_env` if the
    /// regex replacement fails.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use nanobot_prompt::TemplateEngine;
    /// use serde_json::json;
    ///
    /// std::env::set_var("API_KEY", "sk-test-123");
    ///
    /// let engine = TemplateEngine::new();
    /// let mut config = json!({ "apiKey": "{{API_KEY}}" });
    /// engine.render_json_env(&mut config).unwrap();
    /// assert_eq!(config["apiKey"], "sk-test-123");
    /// ```
    pub fn render_json_env(&self, value: &mut serde_json::Value) -> PromptResult<()> {
        // Recursive descent — match on the variant to avoid cloning.
        match value {
            serde_json::Value::String(s) => {
                // Replace in place to avoid rebuilding the entire Value tree.
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
            // Numbers, booleans, null: nothing to substitute.
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
        vars.insert("project".to_string(), "nanobot".to_string());

        let result = engine.render(template, &vars).unwrap();
        assert_eq!(result, "Hello Alice, welcome to nanobot!");
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
    fn test_render_env_substitutes_env_vars() {
        let engine = TemplateEngine::new();

        unsafe {
            std::env::set_var("TEST_API_HOST", "api.example.com");
        }

        let template = "https://{{TEST_API_HOST}}/v1";
        let result = engine.render_env(template).unwrap();
        assert_eq!(result, "https://api.example.com/v1");

        unsafe {
            std::env::remove_var("TEST_API_HOST");
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
