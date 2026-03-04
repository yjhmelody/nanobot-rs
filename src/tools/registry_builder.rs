use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;

use crate::agent::SubagentManager;
use crate::bus::MessageBus;
use crate::config::schema::{ExecToolConfig, WebToolsConfig};
use crate::cron::CronService;
use crate::tools::base::Tool;
use crate::tools::registry::ToolRegistry;

/// Builder for ToolRegistry.
///
/// This builder pattern simplifies the construction of ToolRegistry by:
/// - Separating required parameters from optional ones
/// - Providing clear method names for each configuration option
/// - Allowing flexible construction order
pub struct ToolRegistryBuilder {
    workspace: PathBuf,
    restrict_to_workspace: bool,
    exec_config: ExecToolConfig,
    web_config: WebToolsConfig,
    bus: Option<Arc<MessageBus>>,
    spawn_manager: Option<Arc<SubagentManager>>,
    cron_service: Option<Arc<CronService>>,
    custom_tools: Vec<Arc<dyn Tool>>,
}

impl ToolRegistryBuilder {
    /// Creates a new builder with required parameters.
    pub fn new(workspace: PathBuf) -> Self {
        Self {
            workspace,
            restrict_to_workspace: false,
            exec_config: ExecToolConfig::default(),
            web_config: WebToolsConfig::default(),
            bus: None,
            spawn_manager: None,
            cron_service: None,
            custom_tools: Vec::new(),
        }
    }

    /// Sets whether to restrict file operations to workspace.
    pub fn with_restrict_to_workspace(mut self, restrict: bool) -> Self {
        self.restrict_to_workspace = restrict;
        self
    }

    /// Sets the exec tool configuration.
    pub fn with_exec_config(mut self, config: ExecToolConfig) -> Self {
        self.exec_config = config;
        self
    }

    /// Sets the web tools configuration.
    pub fn with_web_config(mut self, config: WebToolsConfig) -> Self {
        self.web_config = config;
        self
    }

    /// Sets the message bus for the message tool.
    pub fn with_bus(mut self, bus: Arc<MessageBus>) -> Self {
        self.bus = Some(bus);
        self
    }

    /// Sets the spawn manager for the spawn tool.
    pub fn with_spawn_manager(mut self, manager: Arc<SubagentManager>) -> Self {
        self.spawn_manager = Some(manager);
        self
    }

    /// Sets the cron service for the cron tool.
    pub fn with_cron_service(mut self, service: Arc<CronService>) -> Self {
        self.cron_service = Some(service);
        self
    }

    /// Registers a custom tool that will be added after builtin tools.
    pub fn with_custom_tool(mut self, tool: Arc<dyn Tool>) -> Self {
        self.custom_tools.push(tool);
        self
    }

    /// Builds the ToolRegistry.
    pub fn build(self) -> Result<ToolRegistry> {
        let registry = ToolRegistry::new(
            self.workspace,
            self.restrict_to_workspace,
            self.exec_config,
            self.web_config,
            self.bus,
            self.spawn_manager,
            self.cron_service,
        );

        for tool in self.custom_tools {
            registry.register_dynamic_tool(tool)?;
        }

        Ok(registry)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use async_trait::async_trait;

    use super::*;
    use crate::tools::base::{JsonSchema, Tool, ToolContext, ToolDefinition};

    #[test]
    fn builder_creates_registry_with_defaults() {
        let workspace = std::env::temp_dir().join("test-registry-builder");
        let registry = ToolRegistryBuilder::new(workspace.clone())
            .build()
            .expect("build registry");

        let defs = registry.definitions();
        assert!(!defs.is_empty());
        // Should have core tools but not spawn/cron
        let names: Vec<_> = defs.iter().map(|d| d.function.name.as_str()).collect();
        assert!(names.contains(&"read_file"));
        assert!(names.contains(&"exec"));
        assert!(!names.contains(&"spawn"));
        assert!(!names.contains(&"cron"));
    }

    #[test]
    fn builder_accepts_custom_config() {
        let workspace = std::env::temp_dir().join("test-registry-builder-custom");
        let mut exec_config = ExecToolConfig::default();
        exec_config.timeout = 120;

        let registry = ToolRegistryBuilder::new(workspace)
            .with_restrict_to_workspace(true)
            .with_exec_config(exec_config)
            .build()
            .expect("build registry");

        let defs = registry.definitions();
        assert!(!defs.is_empty());
    }

    struct BuilderEchoTool;

    #[async_trait]
    impl Tool for BuilderEchoTool {
        fn name(&self) -> &str {
            "builder_echo"
        }

        fn definition(&self) -> ToolDefinition {
            ToolDefinition::function(
                self.name(),
                "Echo tool for builder tests",
                JsonSchema::object(BTreeMap::new(), Vec::new()),
            )
        }

        async fn execute(&self, _args_json: &str, _ctx: &ToolContext) -> Result<String> {
            Ok("builder-ok".to_string())
        }
    }

    #[tokio::test]
    async fn builder_registers_custom_tool() {
        let workspace = std::env::temp_dir().join("test-registry-builder-custom-tool");
        let registry = ToolRegistryBuilder::new(workspace)
            .with_custom_tool(Arc::new(BuilderEchoTool))
            .build()
            .expect("build registry");

        let names: Vec<_> = registry
            .definitions()
            .into_iter()
            .map(|d| d.function.name)
            .collect();
        assert!(names.contains(&"builder_echo".to_string()));

        let out = registry
            .execute(
                "builder_echo",
                "{}",
                &ToolContext {
                    channel: "test".to_string(),
                    chat_id: "test".to_string(),
                    session_key: "test:test".to_string(),
                    message_id: None,
                },
            )
            .await
            .expect("execute custom tool");
        assert_eq!(out, "builder-ok");
    }

    struct BuilderConflictTool;

    #[async_trait]
    impl Tool for BuilderConflictTool {
        fn name(&self) -> &str {
            "exec"
        }

        fn definition(&self) -> ToolDefinition {
            ToolDefinition::function(
                self.name(),
                "Conflict tool for builder tests",
                JsonSchema::object(BTreeMap::new(), Vec::new()),
            )
        }

        async fn execute(&self, _args_json: &str, _ctx: &ToolContext) -> Result<String> {
            Ok(String::new())
        }
    }

    #[test]
    fn builder_rejects_builtin_name_conflict_for_custom_tool() {
        let workspace = std::env::temp_dir().join("test-registry-builder-conflict-tool");
        let result = ToolRegistryBuilder::new(workspace)
            .with_custom_tool(Arc::new(BuilderConflictTool))
            .build();
        assert!(
            result.is_err(),
            "build should fail on builtin tool conflict"
        );
        let err = result.err().expect("error should be present");
        assert!(err.to_string().contains("conflicts with builtin"));
    }
}
