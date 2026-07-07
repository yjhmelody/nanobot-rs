//! Configuration file loading and saving.
//!
//! This module handles reading the nanobot configuration from disk,
//! parsing it as JSON with comment-stripping and environment variable
//! substitution, and writing configuration back to disk.
//!
//! # Design
//!
//! - Config files are JSON files (`.json`) that may optionally contain
//!   JavaScript-style comments (`//` and `/* ... */`) and environment
//!   variable placeholders (`{{VAR_NAME}}`). The file is therefore closer
//!   to JSONC (JSON with Comments) than strict JSON.
//! - The loading pipeline is:
//!   1. Read raw text from disk.
//!   2. Substitute `{{VAR_NAME}}` placeholders with environment variable
//!      values (missing variables become empty strings).
//!   3. Strip comments, preserving string contents.
//!   4. Deserialize with [`serde_json`].
//! - If the config file does not exist or fails to parse, [`load_config`]
//!   returns [`Config::default()`] with a warning printed to stderr —
//!   the application can always start with a working default config.
//! - The default config path is `~/.nanobot/config.json`, resolved via
//!   [`dirs::home_dir`].
//!
//! # Relationships
//!
//! - Re-exported at the crate root via `pub use loader::{get_config_path, load_config, save_config}`.
//! - Used by `nanobot-agent` and `nanobot-gateway` at startup.
//! - Relies on [`crate::schema::Config`] for the deserialization target.

use std::fs;
use std::path::{Path, PathBuf};

use regex::Regex;

use crate::error::{ConfigError, ConfigResult};
use crate::schema::Config;

/// Returns the default config file path: `~/.nanobot/config.json`.
///
/// # Errors
///
/// Returns [`ConfigError::Invalid`] if the home directory cannot be resolved
/// (e.g., `$HOME` is not set on Unix).
///
/// # Example
///
/// ```
/// use nanobot_config::get_config_path;
///
/// let path = get_config_path().unwrap();
/// assert!(path.ends_with(".nanobot/config.json"));
/// ```
pub fn get_config_path() -> ConfigResult<PathBuf> {
    let home =
        dirs::home_dir().ok_or_else(|| ConfigError::invalid("failed to resolve home directory"))?;
    Ok(home.join(".nanobot").join("config.json"))
}

/// Replaces `{{VAR_NAME}}` placeholders with the value of the corresponding
/// environment variable.
///
/// Placeholder names must match `[A-Za-z0-9_]+`. If the environment variable
/// is not set, the placeholder is replaced with an empty string.
///
/// # Why a custom approach?
///
/// Using a simple regex avoids pulling in a full templating engine for such a
/// narrow use case. The placeholders intentionally use a `{{...}}` syntax that
/// does not conflict with JSON syntax.
///
/// # Limitations
///
/// - Does not support default values (e.g., `{{VAR:-default}}`).
/// - Nested or recursive substitution is not performed.
fn substitute_env_vars(text: &str) -> String {
    // Regex captures `{{...}}` with alphanumeric/underscore content.
    // If the regex fails to compile (shouldn't happen), we use a never-matching
    // pattern as a safe fallback.
    let re = Regex::new(r"\{\{([A-Za-z0-9_]+)\}\}").unwrap_or_else(|_| Regex::new("a^").unwrap());
    re.replace_all(text, |caps: &regex::Captures| {
        std::env::var(&caps[1]).unwrap_or_default()
    })
    .to_string()
}

/// Strips JavaScript-style comments (`//` line comments and `/* ... */`
/// block comments) from a JSON text, preserving string contents.
///
/// This function is a character-level parser, not a full JSON parser. It
/// tracks whether it is inside a quoted string so that comment markers
/// appearing inside strings are preserved.
///
/// # Approach
///
/// - Single-character lookahead to identify `//` and `/*` sequences.
/// - Line comments consume everything up to (and including) the next `\n`.
/// - Block comments track the previous character to detect `*/`.
/// - Newlines inside block comments are preserved so line numbers stay
///   roughly consistent for error reporting.
///
/// # Limitations
///
/// - Does not handle escape sequences beyond `\"` and `\\`.
/// - Does not handle multi-line string literals (not valid JSON).
fn strip_json_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;

    while let Some(ch) = chars.next() {
        if in_string {
            out.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        if ch == '"' {
            in_string = true;
            out.push(ch);
            continue;
        }

        if ch == '/' {
            match chars.peek().copied() {
                Some('/') => {
                    // Line comment: skip until newline
                    chars.next();
                    for next in chars.by_ref() {
                        if next == '\n' {
                            out.push('\n');
                            break;
                        }
                    }
                    continue;
                }
                Some('*') => {
                    // Block comment: skip until */
                    chars.next();
                    let mut prev = '\0';
                    for next in chars.by_ref() {
                        if next == '\n' {
                            out.push('\n');
                        }
                        if prev == '*' && next == '/' {
                            break;
                        }
                        prev = next;
                    }
                    continue;
                }
                _ => {}
            }
        }

        out.push(ch);
    }

    out
}

/// Loads and parses the config file at `config_path`, or the default path if `None`.
///
/// The loading pipeline performs the following steps:
/// 1. **Path resolution** — If `config_path` is `Some`, use it directly;
///    otherwise resolve `~/.nanobot/config.json` via [`get_config_path`].
/// 2. **File existence check** — If the file does not exist, return
///    [`Config::default()`] with no error.
/// 3. **Read** — Read the raw file contents as UTF-8.
/// 4. **Environment substitution** — Replace `{{VAR_NAME}}` placeholders with
///    the corresponding environment variable values via [`substitute_env_vars`].
/// 5. **Comment stripping** — Remove `//` and `/* */` comments via
///    [`strip_json_comments`].
/// 6. **Deserialize** — Parse the sanitized text as JSON.
///
/// # Arguments
///
/// * `config_path` — An optional explicit path to the config file. Pass
///   `None` to use the default path (`~/.nanobot/config.json`).
///
/// # Returns
///
/// Returns `Ok(Config)` with the parsed configuration. If the file is missing
/// or fails to parse, a warning is printed to stderr and `Config::default()`
/// is returned — the application can always start with a safe default.
///
/// # Errors
///
/// Returns [`ConfigError::Io`] if the file exists but cannot be read.
///
/// # Example
///
/// ```
/// use nanobot_config::load_config;
/// use std::path::Path;
///
/// // Load from default path
/// let config = load_config(None).unwrap();
///
/// // Load from an explicit path
/// let config = load_config(Some(Path::new("/tmp/my_config.json"))).unwrap();
/// ```
pub fn load_config(config_path: Option<&Path>) -> ConfigResult<Config> {
    let path = match config_path {
        Some(p) => p.to_path_buf(),
        None => get_config_path()?,
    };

    if !path.exists() {
        return Ok(Config::default());
    }

    let text = fs::read_to_string(&path)?;
    let substituted = substitute_env_vars(&text);
    let sanitized = strip_json_comments(&substituted);

    // If JSON parsing fails, log a warning and fall back to defaults
    // rather than preventing the application from starting.
    let cfg: Config = match serde_json::from_str(&sanitized) {
        Ok(v) => v,
        Err(e) => {
            eprintln!(
                "Warning: failed to parse config {} after env substitution: {}",
                path.display(),
                e
            );
            return Ok(Config::default());
        }
    };

    Ok(cfg)
}

/// Serialises `config` as pretty-printed JSON and writes it to `config_path`,
/// or the default path if `None`. Creates parent directories as needed.
///
/// # Arguments
///
/// * `config` — The [`Config`] to serialize and persist.
/// * `config_path` — An optional explicit path. Pass `None` to use the
///   default path (`~/.nanobot/config.json`).
///
/// # Errors
///
/// Returns [`ConfigError::Io`] if the parent directory cannot be created or
/// the file cannot be written. Returns [`ConfigError::Json`] if serialization
/// fails (should not happen under normal circumstances).
///
/// # Example
///
/// ```
/// use nanobot_config::{Config, save_config};
///
/// let config = Config::default();
/// // This would write to ~/.nanobot/config.json:
/// // save_config(&config, None).unwrap();
/// ```
///
/// # Notes
///
/// - The output is pretty-printed for readability.
/// - The saved JSON does **not** include environment variable placeholders
///   or comments — those are a loading-only feature.
/// - Fields annotated with `#[serde(skip_serializing_if = "Option::is_none")]`
///   are omitted from the output when `None`.
pub fn save_config(config: &Config, config_path: Option<&Path>) -> ConfigResult<()> {
    let path = match config_path {
        Some(p) => p.to_path_buf(),
        None => get_config_path()?,
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let text = serde_json::to_string_pretty(config)?;
    fs::write(&path, text)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_json_comments_keeps_comment_markers_inside_strings() {
        let input = r#"{
  // top level
  "url": "https://example.com//path",
  "note": "/* not a comment */",
  "nested": {
    /* block
       comment */
    "enabled": true
  }
}"#;

        let output = strip_json_comments(input);
        assert!(output.contains(r#""https://example.com//path""#));
        assert!(output.contains(r#""/* not a comment */""#));
        assert!(output.contains(r#""enabled": true"#));
        assert!(!output.contains("top level"));
        assert!(!output.contains("block\n       comment"));
    }

    #[test]
    fn load_config_accepts_jsonc_comments() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("config.json");
        fs::write(
            &path,
            r#"{
  // comment before channels
  "channels": {
    "defaults": {
      "sendProgress": true
    },
    "instances": {
      "test_feishu": {
        "channelType": "lark",
        "enabled": true,
        "allowFrom": ["*"], /* inline block comment */
        "appId": "demo",
        "appSecret": "secret"
      }
    }
  }
}"#,
        )
        .expect("write config");

        let cfg = load_config(Some(&path)).expect("load config");
        assert!(cfg.channels.defaults.send_progress);
        let feishu_instance = cfg
            .channels
            .instances
            .get("test_feishu")
            .expect("feishu instance");
        assert!(feishu_instance.enabled());
        assert_eq!(feishu_instance.allow_from(), &["*".to_string()]);
    }
}
