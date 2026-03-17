use std::path::{Path, PathBuf};

use super::templates::{HISTORY_TEMPLATE_PATH, MEMORY_TEMPLATE, ROOT_TEMPLATES, TemplateFile};
use anyhow::{Context, Result};
use regex::Regex;
use tokio::fs;

pub fn init_tracing() {
    crate::observability::init();
}

/// Synchronous version of ensure_dir for use in constructors.
pub fn ensure_dir(path: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(path)
        .with_context(|| format!("failed to create directory {}", path.display()))?;
    Ok(path.to_path_buf())
}

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

pub fn safe_filename(name: &str) -> String {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r#"[<>:"/\\|?*]"#).expect("invalid regex"));
    re.replace_all(name, "_").trim().to_string()
}

/// Previews text by truncating it to a maximum character count.
///
/// If the text is longer than `max_chars`, it will be truncated and "..." will be appended.
/// This function properly handles Unicode characters by using character boundaries.
///
/// # Arguments
///
/// * `text` - The text to preview
/// * `max_chars` - Maximum number of characters to include in the preview
///
/// # Returns
///
/// A string containing the preview text, possibly truncated with "..." appended
///
/// # Examples
///
/// ```
/// use nanobot_rs::utils::helpers::preview_text;
///
/// assert_eq!(preview_text("Hello", 10), "Hello");
/// assert_eq!(preview_text("Hello, World!", 8), "Hello, W...");
/// assert_eq!(preview_text("你好世界", 2), "你好...");
/// ```
pub fn preview_text(text: &str, max_chars: usize) -> String {
    let char_count = text.chars().count();
    if char_count <= max_chars {
        text.to_string()
    } else {
        format!("{}...", &text.chars().take(max_chars).collect::<String>())
    }
}

pub async fn sync_workspace_templates(workspace: &Path, silent: bool) -> Result<Vec<String>> {
    ensure_dir_async(workspace).await?;
    let mut added = Vec::new();

    for tpl in ROOT_TEMPLATES {
        write_if_missing(workspace, tpl, &mut added).await?;
    }
    write_if_missing(workspace, &MEMORY_TEMPLATE, &mut added).await?;

    let history_path = workspace.join(HISTORY_TEMPLATE_PATH);
    if !fs::try_exists(&history_path).await? {
        if let Some(parent) = history_path.parent() {
            ensure_dir_async(parent).await?;
        }
        fs::write(&history_path, "")
            .await
            .with_context(|| format!("failed to write {}", history_path.display()))?;
        added.push(HISTORY_TEMPLATE_PATH.to_string());
    }

    ensure_dir_async(&workspace.join("skills")).await?;

    if !silent {
        for item in &added {
            println!("  Created {}", item);
        }
    }

    Ok(added)
}

async fn write_if_missing(
    workspace: &Path,
    tpl: &TemplateFile,
    added: &mut Vec<String>,
) -> Result<()> {
    let path = workspace.join(tpl.rel_path);
    if fs::try_exists(&path).await? {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        ensure_dir_async(parent).await?;
    }
    fs::write(&path, tpl.content)
        .await
        .with_context(|| format!("failed to write {}", path.display()))?;
    added.push(tpl.rel_path.to_string());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_workspace(case: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "nanobot-rs-helpers-{}-{}",
            case,
            uuid::Uuid::new_v4()
        ))
    }

    #[test]
    fn safe_filename_replaces_invalid_characters() {
        let sanitized = safe_filename(r#"a<b>c:d"e/f\g|h?i*"#);
        assert_eq!(sanitized, "a_b_c_d_e_f_g_h_i_");
    }

    #[test]
    fn preview_text_preserves_short_strings() {
        assert_eq!(preview_text("Hello", 10), "Hello");
        assert_eq!(preview_text("Test", 100), "Test");
    }

    #[test]
    fn preview_text_truncates_long_strings() {
        assert_eq!(preview_text("Hello, World!", 8), "Hello, W...");
        assert_eq!(preview_text("1234567890", 5), "12345...");
    }

    #[test]
    fn preview_text_handles_unicode() {
        assert_eq!(preview_text("你好世界", 2), "你好...");
        assert_eq!(preview_text("🎉🎊🎈", 2), "🎉🎊...");
        assert_eq!(preview_text("Hello 世界", 8), "Hello 世界");
    }

    #[tokio::test]
    async fn sync_workspace_templates_creates_files_and_is_idempotent() {
        let workspace = temp_workspace("templates");

        let added = sync_workspace_templates(&workspace, true)
            .await
            .expect("sync templates first time");
        assert!(!added.is_empty());

        for tpl in ROOT_TEMPLATES {
            assert!(
                tokio::fs::try_exists(workspace.join(tpl.rel_path))
                    .await
                    .unwrap(),
                "template should exist: {}",
                tpl.rel_path
            );
        }
        assert!(
            tokio::fs::try_exists(workspace.join(MEMORY_TEMPLATE.rel_path))
                .await
                .unwrap()
        );
        assert!(
            tokio::fs::try_exists(workspace.join(HISTORY_TEMPLATE_PATH))
                .await
                .unwrap()
        );
        assert!(
            tokio::fs::try_exists(workspace.join("skills"))
                .await
                .unwrap()
        );

        let added_second = sync_workspace_templates(&workspace, true)
            .await
            .expect("sync templates second time");
        assert!(added_second.is_empty());

        let _ = tokio::fs::remove_dir_all(workspace).await;
    }
}
