//! Shared runtime configuration for all tools.
//!
//! This module provides [`SharedToolConfig`], a thread-safe handle to the
//! tool system's mutable configuration (workspace path, shell execution
//! settings, web/API configuration). It wraps an `Arc<RwLock<ToolConfig>>`
//! so that:
//!
//! - Multiple tools can share the same configuration handle cheaply (Arc).
//! - Configuration can be modified at runtime (RwLock), e.g., switching
//!   workspaces or updating timeouts.
//! - Snapshot semantics: tools call [`SharedToolConfig::snapshot`] at
//!   execution time to get a consistent view, avoiding holding the lock
//!   across the tool's entire execution.
//!
//! ## Locking Strategy
//!
//! Uses `parking_lot::RwLock` because:
//! - `snapshot()` is called every time a tool executes but does not cross
//!   await points.
//! - Configuration updates are rare (typically only on reconfiguration).
//! - `parking_lot` is 3-5x faster than `tokio::sync::RwLock` for short
//!   critical sections.

use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::RwLock;

use nanobot_config::{ExecToolConfig, WebToolsConfig};

/// Shared configuration for all tools.
///
/// A cloneable, thread-safe handle to the mutable runtime tool config.
/// All tools that need configuration data (filesystem, shell, web) hold
/// an instance of this struct and call [`snapshot`](SharedToolConfig::snapshot)
/// at execution time to get a point-in-time view.
///
/// The inner [`ToolConfig`] is protected by a `parking_lot::RwLock` for
/// high-performance reads without crossing await points.
///
/// # Example
///
/// ```ignore
/// let config = SharedToolConfig::new(
///     PathBuf::from("/workspace"),
///     true,
///     exec_config,
///     web_config,
/// );
/// let snapshot = config.snapshot().await;
/// ```
#[derive(Clone)]
pub struct SharedToolConfig {
    inner: Arc<RwLock<ToolConfig>>,
}

/// Configuration for shell command execution.
///
/// Controls timeouts, path appending, and safety guards that prevent
/// destructive commands from being run.
#[derive(Debug, Clone)]
pub struct ExecConfig {
    /// Maximum wall-clock time in seconds for a single shell command.
    pub timeout_secs: u64,
    /// Additional PATH entries to append when executing commands.
    pub path_append: String,
    /// Whether filesystem access is restricted to the workspace directory.
    pub restrict_to_workspace: bool,
    /// When true, pattern-based safety guards (`rm -rf`, `mkfs`, etc.)
    /// are bypassed.
    pub disable_safety_guard: bool,
    /// When true, **all** safety guards (patterns, path traversal checks,
    /// workspace boundary checks) are bypassed.
    pub disable_all_guards: bool,
}

/// Configuration for web fetching and search tools.
#[derive(Debug, Clone)]
pub struct WebConfig {
    /// API key for the Brave Search API.
    pub search_api_key: String,
    /// Maximum number of search results to return by default.
    pub search_max_results: usize,
    /// Optional HTTP proxy URL (e.g., `http://proxy:8080`).
    pub proxy: Option<String>,
}

impl SharedToolConfig {
    /// Creates a new shared configuration from application-level config values.
    ///
    /// # Arguments
    ///
    /// * `workspace` - The base working directory for file operations.
    /// * `restrict_to_workspace` - If true, file/exec operations are confined
    ///   to the workspace directory.
    /// * `exec_config` - Shell execution settings from the application config.
    /// * `web_config` - Web/fetch settings from the application config.
    pub fn new(
        workspace: PathBuf,
        restrict_to_workspace: bool,
        exec_config: ExecToolConfig,
        web_config: WebToolsConfig,
    ) -> Self {
        let allowed_dir = if restrict_to_workspace {
            Some(workspace.clone())
        } else {
            None
        };

        Self {
            inner: Arc::new(RwLock::new(ToolConfig {
                workspace,
                allowed_dir,
                exec: ExecConfig {
                    timeout_secs: exec_config.timeout,
                    path_append: exec_config.path_append,
                    restrict_to_workspace,
                    disable_safety_guard: exec_config.disable_safety_guard,
                    disable_all_guards: exec_config.disable_all_guards,
                },
                web: WebConfig {
                    search_api_key: web_config.search.api_key,
                    search_max_results: web_config.search.max_results,
                    proxy: web_config.proxy,
                },
            })),
        }
    }

    /// Returns a point-in-time snapshot of the current configuration.
    ///
    /// This is the primary accessor used by tools. It acquires a read lock
    /// briefly, clones the config, and releases the lock before any async
    /// work begins. This avoids holding locks across await points.
    pub async fn snapshot(&self) -> ToolConfigSnapshot {
        let guard = self.inner.read();
        ToolConfigSnapshot {
            workspace: guard.workspace.clone(),
            allowed_dir: guard.allowed_dir.clone(),
            exec: guard.exec.clone(),
            web: guard.web.clone(),
        }
    }

    /// Updates the workspace directory at runtime.
    ///
    /// If `restrict_to_workspace` is enabled, the allowed directory is also
    /// updated to match the new workspace path.
    pub async fn set_workspace(&self, workspace: PathBuf) {
        let mut guard = self.inner.write();
        if guard.exec.restrict_to_workspace {
            guard.allowed_dir = Some(workspace.clone());
        }
        guard.workspace = workspace;
    }

    /// Updates shell execution settings at runtime.
    ///
    /// Applies new timeout, path append, and safety guard settings.
    pub async fn update_exec_config(&self, config: ExecToolConfig) {
        let mut guard = self.inner.write();
        guard.exec.timeout_secs = config.timeout;
        guard.exec.path_append = config.path_append;
        guard.exec.disable_safety_guard = config.disable_safety_guard;
        guard.exec.disable_all_guards = config.disable_all_guards;
    }

    /// Updates web/search configuration at runtime.
    pub async fn update_web_config(&self, config: WebToolsConfig) {
        let mut guard = self.inner.write();
        guard.web.search_api_key = config.search.api_key;
        guard.web.search_max_results = config.search.max_results;
        guard.web.proxy = config.proxy;
    }

    /// Updates only the shell execution timeout.
    ///
    /// Convenience method to avoid reconstructing the full `ExecToolConfig`.
    pub async fn set_exec_timeout(&self, timeout_secs: u64) {
        let mut inner = self.inner.write();
        inner.exec.timeout_secs = timeout_secs;
    }

    /// Enables or disables workspace restriction at runtime.
    ///
    /// When restricted, the allowed directory is set to the current workspace.
    /// When unrestricted, the allowed directory is cleared (allowing any path).
    pub async fn set_restrict_to_workspace(&self, restrict: bool) {
        let mut inner = self.inner.write();
        inner.allowed_dir = if restrict {
            Some(inner.workspace.clone())
        } else {
            None
        };
        inner.exec.restrict_to_workspace = restrict;
    }
}

/// Internal mutable configuration state.
///
/// This struct is not exposed publicly. All access goes through
/// `SharedToolConfig` methods.
struct ToolConfig {
    workspace: PathBuf,
    allowed_dir: Option<PathBuf>,
    exec: ExecConfig,
    web: WebConfig,
}

/// A point-in-time snapshot of the tool configuration.
///
/// Tools obtain this via [`SharedToolConfig::snapshot`] and use it
/// throughout their execution without holding any locks.
///
/// # Fields
///
/// * `workspace` - Current workspace directory for file operations.
/// * `allowed_dir` - If set, the directory to which operations are
///   restricted (typically the workspace).
/// * `exec` - Shell execution configuration.
/// * `web` - Web search/fetch configuration.
#[derive(Debug, Clone)]
pub struct ToolConfigSnapshot {
    pub workspace: PathBuf,
    pub allowed_dir: Option<PathBuf>,
    pub exec: ExecConfig,
    pub web: WebConfig,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn new_config_has_no_allowed_dir_when_unrestricted() {
        let config = SharedToolConfig::new(
            PathBuf::from("/workspace"),
            false,
            ExecToolConfig::default(),
            WebToolsConfig::default(),
        );
        let snapshot = config.snapshot().await;
        assert!(snapshot.allowed_dir.is_none());
    }

    #[tokio::test]
    async fn new_config_sets_allowed_dir_when_restricted() {
        let config = SharedToolConfig::new(
            PathBuf::from("/workspace"),
            true,
            ExecToolConfig::default(),
            WebToolsConfig::default(),
        );
        let snapshot = config.snapshot().await;
        assert_eq!(
            snapshot.allowed_dir.as_deref(),
            Some(PathBuf::from("/workspace").as_path())
        );
    }

    #[tokio::test]
    async fn update_exec_config_changes_timeout() {
        let config = SharedToolConfig::new(
            PathBuf::from("/workspace"),
            false,
            ExecToolConfig::default(),
            WebToolsConfig::default(),
        );
        config
            .update_exec_config(ExecToolConfig {
                timeout: 120,
                path_append: String::new(),
                disable_safety_guard: true,
                disable_all_guards: true,
            })
            .await;
        let snapshot = config.snapshot().await;
        assert_eq!(snapshot.exec.timeout_secs, 120);
        assert!(snapshot.exec.disable_safety_guard);
        assert!(snapshot.exec.disable_all_guards);
    }

    #[tokio::test]
    async fn set_workspace_updates_allowed_dir_when_restricted() {
        let config = SharedToolConfig::new(
            PathBuf::from("/workspace1"),
            true,
            ExecToolConfig::default(),
            WebToolsConfig::default(),
        );

        let snapshot1 = config.snapshot().await;
        assert_eq!(
            snapshot1.allowed_dir.as_deref(),
            Some(PathBuf::from("/workspace1").as_path())
        );

        config.set_workspace(PathBuf::from("/workspace2")).await;

        let snapshot2 = config.snapshot().await;
        assert_eq!(snapshot2.workspace, PathBuf::from("/workspace2"));
        assert_eq!(
            snapshot2.allowed_dir.as_deref(),
            Some(PathBuf::from("/workspace2").as_path())
        );
    }
}
