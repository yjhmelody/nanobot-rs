use std::path::{Path, PathBuf};

use super::templates::{HISTORY_TEMPLATE_PATH, MEMORY_TEMPLATE, ROOT_TEMPLATES};
use anyhow::{Context, Result};
use tokio::fs;

/// Asynchronous version of ensure_dir.
pub async fn ensure_dir_async(path: &Path) -> Result<PathBuf> {
    fs::create_dir_all(path)
        .await
        .with_context(|| format!("failed to create directory {}", path.display()))?;
    Ok(path.to_path_buf())
}

pub async fn get_data_path() -> Result<PathBuf> {
    let home = dirs::home_dir().context("failed to resolve home directory")?;
    let path = home.join(".nanobot");
    ensure_dir_async(&path).await
}

pub async fn get_workspace_path(workspace: Option<&str>) -> Result<PathBuf> {
    let path = if let Some(raw) = workspace {
        expand_tilde(raw)?
    } else {
        dirs::home_dir()
            .context("failed to resolve home directory")?
            .join(".nanobot")
            .join("workspace")
    };
    ensure_dir_async(&path).await
}

pub fn expand_tilde(raw: &str) -> Result<PathBuf> {
    if let Some(rest) = raw.strip_prefix("~/") {
        let home = dirs::home_dir().context("failed to resolve home directory")?;
        Ok(home.join(rest))
    } else {
        Ok(PathBuf::from(raw))
    }
}

// pub fn safe_filename(name: &str) -> String {
//     static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
//     let re = RE.get_or_init(|| Regex::new(r#"[<>:"/\\|?*]"#).expect("invalid regex"));
//     re.replace_all(name, "_").trim().to_string()
// }

/// Sync workspace templates to the workspace directory.
/// Returns a list of files that were added.
pub async fn sync_workspace_templates(workspace: &Path, overwrite: bool) -> Result<Vec<String>> {
    let mut added = Vec::new();

    for tpl in ROOT_TEMPLATES {
        let dest = workspace.join(tpl.rel_path);
        if overwrite || !dest.exists() {
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent).await?;
            }
            fs::write(&dest, tpl.content).await?;
            added.push(tpl.rel_path.to_string());
        }
    }

    let memory_dest = workspace.join(MEMORY_TEMPLATE.rel_path);
    if overwrite || !memory_dest.exists() {
        if let Some(parent) = memory_dest.parent() {
            fs::create_dir_all(parent).await?;
        }
        fs::write(&memory_dest, MEMORY_TEMPLATE.content).await?;
        added.push(MEMORY_TEMPLATE.rel_path.to_string());
    }

    let history_dest = workspace.join(HISTORY_TEMPLATE_PATH);
    if overwrite || !history_dest.exists() {
        if let Some(parent) = history_dest.parent() {
            fs::create_dir_all(parent).await?;
        }
        fs::write(&history_dest, "").await?;
        added.push(HISTORY_TEMPLATE_PATH.to_string());
    }

    let skills_dir = workspace.join("skills");
    if !skills_dir.exists() {
        fs::create_dir_all(&skills_dir).await?;
    }

    Ok(added)
}
