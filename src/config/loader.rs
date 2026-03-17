use std::fs;
use std::path::{Path, PathBuf};

use crate::config::error::{ConfigError, ConfigResult};
use crate::config::schema::Config;
use crate::prompt::TemplateEngine;

pub fn get_config_path() -> ConfigResult<PathBuf> {
    let home =
        dirs::home_dir().ok_or_else(|| ConfigError::invalid("failed to resolve home directory"))?;
    Ok(home.join(".nanobot").join("config.json"))
}

pub fn load_config(config_path: Option<&Path>) -> ConfigResult<Config> {
    let path = match config_path {
        Some(p) => p.to_path_buf(),
        None => get_config_path()?,
    };

    if !path.exists() {
        return Ok(Config::default());
    }

    let text = fs::read_to_string(&path)?;

    // Use TemplateEngine for environment variable substitution on raw text
    let engine = TemplateEngine::new();

    let substituted_text = match engine.render_env(&text) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("Warning: failed to substitute env vars in config: {}", e);
            text
        }
    };

    // Single deserialization from substituted text
    let cfg: Config = match serde_json::from_str(&substituted_text) {
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
    fn load_config_uses_current_tools_restrict_to_workspace() {
        let raw = r#"{
            "tools": {
                "restrictToWorkspace": true
            }
        }"#;

        let tmp =
            std::env::temp_dir().join(format!("nanobot-rs-config-{}.json", uuid::Uuid::new_v4()));
        std::fs::write(&tmp, raw).expect("write temp config");

        let cfg = load_config(Some(&tmp)).expect("load config");
        assert!(cfg.tools.restrict_to_workspace);

        let _ = std::fs::remove_file(tmp);
    }

    #[test]
    fn load_config_ignores_exec_restrict_to_workspace() {
        let tmp =
            std::env::temp_dir().join(format!("nanobot-rs-config-{}.json", uuid::Uuid::new_v4()));
        std::fs::write(
            &tmp,
            r#"{
                "tools": {
                    "exec": {
                        "restrictToWorkspace": true
                    }
                }
            }"#,
        )
        .expect("write temp config");

        let cfg = load_config(Some(&tmp)).expect("load config");
        assert!(!cfg.tools.restrict_to_workspace);

        let _ = std::fs::remove_file(tmp);
    }

    #[test]
    fn load_config_resolves_env_placeholders() {
        let key = "NANOBOT_TEST_ENV_TOKEN";
        unsafe {
            std::env::set_var(key, "sk-test-123");
        }

        let tmp =
            std::env::temp_dir().join(format!("nanobot-rs-config-{}.json", uuid::Uuid::new_v4()));
        std::fs::write(
            &tmp,
            r#"{
                "providers": {
                    "openai": {
                        "apiKey": "{{NANOBOT_TEST_ENV_TOKEN}}"
                    }
                }
            }"#,
        )
        .expect("write temp config");

        let cfg = load_config(Some(&tmp)).expect("load config");
        assert_eq!(cfg.providers.openai.api_key, "sk-test-123");

        let _ = std::fs::remove_file(tmp);
        unsafe {
            std::env::remove_var(key);
        }
    }

    #[test]
    fn load_config_clears_missing_env_placeholders() {
        let key = "NANOBOT_TEST_ENV_MISSING";
        unsafe {
            std::env::remove_var(key);
        }

        let tmp =
            std::env::temp_dir().join(format!("nanobot-rs-config-{}.json", uuid::Uuid::new_v4()));
        std::fs::write(
            &tmp,
            r#"{
                "providers": {
                    "openai": {
                        "apiKey": "{{NANOBOT_TEST_ENV_MISSING}}"
                    }
                }
            }"#,
        )
        .expect("write temp config");

        let cfg = load_config(Some(&tmp)).expect("load config");
        assert!(cfg.providers.openai.api_key.is_empty());

        let _ = std::fs::remove_file(tmp);
    }

    #[test]
    fn load_config_supports_partial_env_substitution() {
        let key = "NANOBOT_TEST_HOST";
        unsafe {
            std::env::set_var(key, "api.example.com");
        }

        let tmp =
            std::env::temp_dir().join(format!("nanobot-rs-config-{}.json", uuid::Uuid::new_v4()));
        std::fs::write(
            &tmp,
            r#"{
                "providers": {
                    "custom": {
                        "apiBase": "https://{{NANOBOT_TEST_HOST}}/v1"
                    }
                }
            }"#,
        )
        .expect("write temp config");

        let cfg = load_config(Some(&tmp)).expect("load config");
        assert_eq!(
            cfg.providers.custom.api_base,
            Some("https://api.example.com/v1".to_string())
        );

        let _ = std::fs::remove_file(tmp);
        unsafe {
            std::env::remove_var(key);
        }
    }

    #[test]
    fn load_config_supports_env_in_keys() {
        let key = "NANOBOT_TEST_PROVIDER";
        unsafe {
            std::env::set_var(key, "openai");
        }

        let tmp =
            std::env::temp_dir().join(format!("nanobot-rs-config-{}.json", uuid::Uuid::new_v4()));
        std::fs::write(
            &tmp,
            r#"{
                "providers": {
                    "{{NANOBOT_TEST_PROVIDER}}": {
                        "apiKey": "sk-test-key"
                    }
                }
            }"#,
        )
        .expect("write temp config");

        let cfg = load_config(Some(&tmp)).expect("load config");
        assert_eq!(cfg.providers.openai.api_key, "sk-test-key");

        let _ = std::fs::remove_file(tmp);
        unsafe {
            std::env::remove_var(key);
        }
    }
}
