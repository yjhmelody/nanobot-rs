use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::config::schema::Config;

pub fn get_config_path() -> Result<PathBuf> {
    let home = dirs::home_dir().context("failed to resolve home directory")?;
    Ok(home.join(".nanobot").join("config.json"))
}

pub fn load_config(config_path: Option<&Path>) -> Result<Config> {
    let path = match config_path {
        Some(p) => p.to_path_buf(),
        None => get_config_path()?,
    };

    if !path.exists() {
        return Ok(Config::default());
    }

    let text =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;

    let cfg: Config = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Warning: failed to parse config {}: {}", path.display(), e);
            return Ok(Config::default());
        }
    };

    Ok(cfg)
}

pub fn save_config(config: &Config, config_path: Option<&Path>) -> Result<()> {
    let path = match config_path {
        Some(p) => p.to_path_buf(),
        None => get_config_path()?,
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    let text = serde_json::to_string_pretty(config)?;
    fs::write(&path, text).with_context(|| format!("failed to write {}", path.display()))?;
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
}
