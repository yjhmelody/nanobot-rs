use std::path::{Path, PathBuf};
use anyhow::{Context, Result};
use regex::Regex;

pub fn ensure_dir(path: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(path)
        .with_context(|| format!("failed to create directory {}", path.display()))?;
    Ok(path.to_path_buf())
}

pub async fn ensure_dir_async(path: &Path) -> Result<PathBuf> {
    tokio::fs::create_dir_all(path)
        .await
        .with_context(|| format!("failed to create directory {}", path.display()))?;
    Ok(path.to_path_buf())
}

pub fn safe_filename(name: &str) -> String {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r#"[<>:"/\\|?*]"#).expect("invalid regex"));
    re.replace_all(name, "_").trim().to_string()
}
