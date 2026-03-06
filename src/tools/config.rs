use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::RwLock;

use crate::config::schema::{ExecToolConfig, WebToolsConfig};

/// Shared configuration for all tools.
///
/// This configuration is wrapped in Arc<RwLock<>> to allow:
/// - Sharing across multiple tools (Arc)
/// - Runtime modification (RwLock)
/// - Thread-safe access (RwLock)
/// ```
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
    /// Creates a new shared tool configuration.
    ///
    /// # Arguments
    ///
    /// * `workspace` - Base workspace directory
    /// * `restrict_to_workspace` - Whether to restrict file operations to workspace
    /// * `exec_config` - Shell execution configuration
    /// * `web_config` - Web tools configuration
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

    /// Get a snapshot of current configuration.
    ///
    /// This returns an immutable config that can be used for tool execution
    /// without holding the lock.
    pub async fn snapshot(&self) -> ToolConfig {
        let inner = self.inner.read().await;
        ToolConfig {
            workspace: inner.workspace.clone(),
            allowed_dir: inner.allowed_dir.clone(),
            exec: inner.exec.clone(),
            web: inner.web.clone(),
        }
    }

    /// Update workspace directory.
    ///
    /// If workspace restriction is enabled, this also updates the allowed_dir.
    pub async fn set_workspace(&self, workspace: PathBuf) {
        let mut inner = self.inner.write().await;
        inner.workspace = workspace.clone();
        if inner.exec.restrict_to_workspace {
            inner.allowed_dir = Some(workspace);
        }
    }

    /// Update exec timeout in seconds.
    pub async fn set_exec_timeout(&self, timeout_secs: u64) {
        let mut inner = self.inner.write().await;
        inner.exec.timeout_secs = timeout_secs;
    }

    /// Update PATH append string for shell execution.
    pub async fn set_path_append(&self, path_append: String) {
        let mut inner = self.inner.write().await;
        inner.exec.path_append = path_append;
    }

    /// Update web search API key.
    pub async fn set_search_api_key(&self, api_key: String) {
        let mut inner = self.inner.write().await;
        inner.web.search_api_key = api_key;
    }

    /// Update web search max results.
    pub async fn set_search_max_results(&self, max_results: usize) {
        let mut inner = self.inner.write().await;
        inner.web.search_max_results = max_results;
    }

    /// Update web proxy setting.
    pub async fn set_proxy(&self, proxy: Option<String>) {
        let mut inner = self.inner.write().await;
        inner.web.proxy = proxy;
    }

    /// Enable/disable workspace restriction.
    ///
    /// When enabled, all file operations are restricted to the workspace directory.
    /// When disabled, file operations can access any path.
    pub async fn set_restrict_to_workspace(&self, restrict: bool) {
        let mut inner = self.inner.write().await;
        inner.allowed_dir = if restrict {
            Some(inner.workspace.clone())
        } else {
            None
        };
        inner.exec.restrict_to_workspace = restrict;
    }
}

/// Immutable snapshot of configuration for tool execution.
///
/// This snapshot can be used without holding any locks, making it safe
/// to use across async boundaries.
#[derive(Debug, Clone)]
pub struct ToolConfig {
    /// Base workspace directory for all file operations
    pub workspace: PathBuf,
    /// Optional restriction directory (if Some, all file ops must be within this dir)
    pub allowed_dir: Option<PathBuf>,
    /// Shell execution configuration
    pub exec: ExecConfig,
    /// Web tools configuration
    pub web: WebConfig,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn shared_config_allows_runtime_modification() {
        let config = SharedToolConfig::new(
            PathBuf::from("/workspace"),
            false,
            ExecToolConfig {
                timeout: 30,
                path_append: String::new(),
            },
            WebToolsConfig {
                search: crate::config::schema::WebSearchConfig {
                    api_key: "key1".to_string(),
                    max_results: 5,
                },
                proxy: None,
            },
        );

        let snapshot1 = config.snapshot().await;
        assert_eq!(snapshot1.exec.timeout_secs, 30);
        assert_eq!(snapshot1.web.search_api_key, "key1");

        config.set_exec_timeout(60).await;
        config.set_search_api_key("key2".to_string()).await;

        let snapshot2 = config.snapshot().await;
        assert_eq!(snapshot2.exec.timeout_secs, 60);
        assert_eq!(snapshot2.web.search_api_key, "key2");
    }

    #[tokio::test]
    async fn set_restrict_to_workspace_updates_allowed_dir() {
        let config = SharedToolConfig::new(
            PathBuf::from("/workspace"),
            false,
            ExecToolConfig::default(),
            WebToolsConfig::default(),
        );

        let snapshot1 = config.snapshot().await;
        assert!(snapshot1.allowed_dir.is_none());

        config.set_restrict_to_workspace(true).await;

        let snapshot2 = config.snapshot().await;
        assert_eq!(
            snapshot2.allowed_dir.as_deref(),
            Some(PathBuf::from("/workspace").as_path())
        );

        config.set_restrict_to_workspace(false).await;

        let snapshot3 = config.snapshot().await;
        assert!(snapshot3.allowed_dir.is_none());
    }

    #[tokio::test]
    async fn batch_update_is_atomic() {
        let config = SharedToolConfig::new(
            PathBuf::from("/workspace"),
            false,
            ExecToolConfig {
                timeout: 30,
                path_append: String::new(),
            },
            WebToolsConfig {
                search: crate::config::schema::WebSearchConfig {
                    api_key: "key1".to_string(),
                    max_results: 5,
                },
                proxy: None,
            },
        );

        // Batch update using individual setters
        config.set_exec_timeout(120).await;
        config.set_search_max_results(20).await;
        config
            .set_proxy(Some("http://proxy:8080".to_string()))
            .await;

        let snapshot = config.snapshot().await;
        assert_eq!(snapshot.exec.timeout_secs, 120);
        assert_eq!(snapshot.web.search_max_results, 20);
        assert_eq!(snapshot.web.proxy.as_deref(), Some("http://proxy:8080"));
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
