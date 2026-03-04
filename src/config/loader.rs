use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

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

    let mut cfg: Config = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Warning: failed to parse config {}: {}", path.display(), e);
            return Ok(Config::default());
        }
    };

    migrate_config(&mut cfg, &text);
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

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct LegacyConfigCompat {
    tools: LegacyToolsCompat,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct LegacyToolsCompat {
    restrict_to_workspace: Option<bool>,
    exec: LegacyExecCompat,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct LegacyExecCompat {
    restrict_to_workspace: Option<bool>,
}

fn migrate_config(config: &mut Config, raw_json: &str) {
    let legacy = serde_json::from_str::<LegacyConfigCompat>(raw_json).unwrap_or_default();

    if legacy.tools.restrict_to_workspace.is_none()
        && let Some(old_restrict) = legacy.tools.exec.restrict_to_workspace
    {
        config.tools.restrict_to_workspace = old_restrict;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrate_moves_restrict_flag_from_exec_to_tools() {
        let raw = r#"{
            "tools": {
                "exec": {
                    "restrictToWorkspace": true
                }
            }
        }"#;

        let mut cfg: Config = serde_json::from_str(raw).expect("parse config");
        migrate_config(&mut cfg, raw);
        assert!(cfg.tools.restrict_to_workspace);
    }

    #[test]
    fn migrate_keeps_existing_top_level_value() {
        let raw = r#"{
            "tools": {
                "restrictToWorkspace": false,
                "exec": {
                    "restrictToWorkspace": true
                }
            }
        }"#;

        let mut cfg: Config = serde_json::from_str(raw).expect("parse config");
        migrate_config(&mut cfg, raw);
        assert!(!cfg.tools.restrict_to_workspace);
    }

    #[test]
    fn load_config_applies_migration() {
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
        assert!(cfg.tools.restrict_to_workspace);

        let _ = std::fs::remove_file(tmp);
    }
}
