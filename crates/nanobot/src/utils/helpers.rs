//! Filesystem helper functions for path resolution and template synchronisation.
//!
//! Provides utilities for resolving the nanobot data directory
//! (`~/.nanobot`), the workspace path, and synchronising default workspace
//! templates (AGENTS.md, SOUL.md, HEARTBEAT.md, etc.).

use std::path::{Path, PathBuf};

use super::templates::{HISTORY_TEMPLATE_PATH, MEMORY_TEMPLATE, ROOT_TEMPLATES};
use anyhow::{Context, Result};
use tokio::fs;

/// Ensure a directory exists, creating it and any parents if necessary.
///
/// Returns the canonical path of the directory.
pub async fn ensure_dir_async(path: &Path) -> Result<PathBuf> {
    fs::create_dir_all(path)
        .await
        .with_context(|| format!("failed to create directory {}", path.display()))?;
    Ok(path.to_path_buf())
}

/// Resolve the nanobot data directory (`~/.nanobot`), creating it if needed.
pub async fn get_data_path() -> Result<PathBuf> {
    let home = dirs::home_dir().context("failed to resolve home directory")?;
    let path = home.join(".nanobot");
    ensure_dir_async(&path).await
}

/// Resolve the workspace directory, creating it if needed.
///
/// If `workspace` is `Some`, it may contain a `~/` prefix that will be
/// expanded. If `None`, defaults to `~/.nanobot/workspace`.
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

/// Expand a leading `~/` in a path to the user's home directory.
///
/// If the path does not start with `~/`, it is returned as-is.
pub fn expand_tilde(raw: &str) -> Result<PathBuf> {
    if let Some(rest) = raw.strip_prefix("~/") {
        let home = dirs::home_dir().context("failed to resolve home directory")?;
        Ok(home.join(rest))
    } else {
        Ok(PathBuf::from(raw))
    }
}

/// Sync workspace templates to the workspace directory.
///
/// Writes the following files if they do not exist (or if `overwrite` is true):
/// - Root templates: `AGENTS.md`, `SOUL.md`, `USER.md`, `TOOLS.md`, `HEARTBEAT.md`
/// - Memory template: `memory/MEMORY.md`
/// - History file: `memory/HISTORY.md` (empty)
/// - Skills directory: `skills/` (empty directory)
///
/// Returns a list of relative paths that were written.
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
        // HISTORY.md starts empty; it is populated by the agent loop.
        fs::write(&history_dest, "").await?;
        added.push(HISTORY_TEMPLATE_PATH.to_string());
    }

    let skills_dir = workspace.join("skills");
    if !skills_dir.exists() {
        fs::create_dir_all(&skills_dir).await?;
    }

    Ok(added)
}
