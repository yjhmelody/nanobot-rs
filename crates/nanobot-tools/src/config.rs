use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::RwLock;

use nanobot_config::{ExecToolConfig, WebToolsConfig};

/// Shared configuration for all tools.
///
/// This configuration is wrapped in Arc<RwLock<>> to allow:
/// - Sharing across multiple tools (Arc)
/// - Runtime modification (RwLock)
/// - Thread-safe access (RwLock)
///
/// Uses parking_lot::RwLock for better performance since:
/// - snapshot() is called frequently but doesn't cross await points
/// - Configuration updates are rare
/// - parking_lot is 3-5x faster than tokio::sync::RwLock for this use case
#[derive(Clone)]
pub struct SharedToolConfig {
    inner: Arc<RwLock<ToolConfig>>,
}

/// Config for shell execution
#[derive(Debug, Clone)]
pub struct ExecConfig {
    pub timeout_secs: u64,
    pub path_append: String,
    pub restrict_to_workspace: bool,
}

#[derive(Debug, Clone)]
pub struct WebConfig {
    pub search_api_key: String,
    pub search_max_results: usize,
    pub proxy: Option<String>,
}

impl SharedToolConfig {
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
                },
                web: WebConfig {
                    search_api_key: web_config.search.api_key,
                    search_max_results: web_config.search.max_results,
                    proxy: web_config.proxy,
                },
            })),
        }
    }

    pub async fn snapshot(&self) -> ToolConfigSnapshot {
        let guard = self.inner.read();
        ToolConfigSnapshot {
            workspace: guard.workspace.clone(),
            allowed_dir: guard.allowed_dir.clone(),
            exec: guard.exec.clone(),
            web: guard.web.clone(),
        }
    }

    pub async fn set_workspace(&self, workspace: PathBuf) {
        let mut guard = self.inner.write();
        if guard.exec.restrict_to_workspace {
            guard.allowed_dir = Some(workspace.clone());
        }
        guard.workspace = workspace;
    }

    pub async fn update_exec_config(&self, config: ExecToolConfig) {
        let mut guard = self.inner.write();
        guard.exec.timeout_secs = config.timeout;
        guard.exec.path_append = config.path_append;
    }

    pub async fn update_web_config(&self, config: WebToolsConfig) {
        let mut guard = self.inner.write();
        guard.web.search_api_key = config.search.api_key;
        guard.web.search_max_results = config.search.max_results;
        guard.web.proxy = config.proxy;
    }

    pub async fn set_exec_timeout(&self, timeout_secs: u64) {
        let mut inner = self.inner.write();
        inner.exec.timeout_secs = timeout_secs;
    }

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

struct ToolConfig {
    workspace: PathBuf,
    allowed_dir: Option<PathBuf>,
    exec: ExecConfig,
    web: WebConfig,
}

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
                ..Default::default()
            })
            .await;
        let snapshot = config.snapshot().await;
        assert_eq!(snapshot.exec.timeout_secs, 120);
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
