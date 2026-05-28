use std::fs;
use std::path::{Path, PathBuf};

use regex::Regex;

use crate::error::{ConfigError, ConfigResult};
use crate::schema::Config;

/// Returns the default config file path: `~/.nanobot/config.json`.
pub fn get_config_path() -> ConfigResult<PathBuf> {
    let home =
        dirs::home_dir().ok_or_else(|| ConfigError::invalid("failed to resolve home directory"))?;
    Ok(home.join(".nanobot").join("config.json"))
}

fn substitute_env_vars(text: &str) -> String {
    let re = Regex::new(r"\{\{([A-Za-z0-9_]+)\}\}").unwrap_or_else(|_| Regex::new("a^").unwrap());
    re.replace_all(text, |caps: &regex::Captures| {
        std::env::var(&caps[1]).unwrap_or_default()
    })
    .to_string()
}

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
/// `{{ENV_VAR}}` placeholders in the file are substituted with the corresponding
/// environment variable values before parsing. Returns `Config::default()` if the
/// file does not exist or fails to parse.
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
    "sendProgress": true,
    "lark": {
      "enabled": true,
      "allowFrom": ["*"], /* inline block comment */
      "appId": "demo",
      "appSecret": "secret"
    }
  }
}"#,
        )
        .expect("write config");

        let cfg = load_config(Some(&path)).expect("load config");
        assert!(cfg.channels.send_progress);
        assert!(cfg.channels.feishu.enabled);
        assert_eq!(cfg.channels.feishu.allow_from, vec!["*".to_string()]);
        assert_eq!(
            cfg.channels
                .feishu
                .extra
                .get("appId")
                .and_then(|v| v.as_str()),
            Some("demo")
        );
    }
}
