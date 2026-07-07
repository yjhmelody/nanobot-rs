//! Utility functions for filesystem operations and filename sanitisation.
//!
//! These helpers are used internally by `JsonlSessionStore` and `MemoryStore`
//! to ensure safe filesystem interactions:
//!
//! - `ensure_dir` / `ensure_dir_async`: Create a directory (and parents) if it
//!   does not exist, returning the path.
//! - `safe_filename`: Replace filesystem-unsafe characters with underscores so
//!   that session keys (which may contain `:`, `/`, etc.) can be used as file
//!   names.

use anyhow::{Context, Result};
use regex::Regex;
use std::path::{Path, PathBuf};

/// Creates a directory synchronously, including all parent directories.
///
/// This is a blocking helper used during initialisation (e.g., `MemoryStore::new`).
/// For async contexts, use [`ensure_dir_async`] instead.
///
/// # Arguments
///
/// * `path` - The directory path to create.
///
/// # Returns
///
/// The same `path` if successful.
///
/// # Errors
///
/// Returns an error if the directory cannot be created (permissions, read-only
/// filesystem, etc.).
pub fn ensure_dir(path: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(path)
        .with_context(|| format!("failed to create directory {}", path.display()))?;
    Ok(path.to_path_buf())
}

/// Creates a directory asynchronously, including all parent directories.
///
/// Used by `JsonlSessionStore::new` during initialisation.
///
/// # Arguments
///
/// * `path` - The directory path to create.
///
/// # Returns
///
/// The same `path` if successful.
///
/// # Errors
///
/// Returns an error if the directory cannot be created.
pub async fn ensure_dir_async(path: &Path) -> Result<PathBuf> {
    tokio::fs::create_dir_all(path)
        .await
        .with_context(|| format!("failed to create directory {}", path.display()))?;
    Ok(path.to_path_buf())
}

/// Replaces filesystem-unsafe characters in a filename with underscores.
///
/// The following characters are replaced: `<`, `>`, `:`, `"`, `/`, `\`, `|`,
/// `?`, `*`. This ensures that session keys like `"telegram:123/456"` can be
/// used as safe file names.
///
/// The compiled regex is cached in a `OnceLock` for efficiency.
///
/// # Arguments
///
/// * `name` - The input string to sanitise.
///
/// # Returns
///
/// A sanitised string safe for use as a filename component.
///
/// # Examples
///
/// ```
/// use nanobot_session::helpers::safe_filename;
/// assert_eq!(safe_filename("telegram:123/456"), "telegram_123_456");
/// ```
pub fn safe_filename(name: &str) -> String {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r#"[<>:"/\\|?*]"#).expect("invalid regex"));
    re.replace_all(name, "_").trim().to_string()
}
