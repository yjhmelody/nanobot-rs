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

    let cfg: Config = match serde_json::from_str(&substituted) {
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
