use std::path::PathBuf;
use std::sync::Arc;

use crate::error::ToolResult;

use crate::base::Tool;
use crate::registry::ToolRegistry;
use crate::spawn::SpawnService;
use nanobot_bus::MessageBus;
use nanobot_config::{ExecToolConfig, WebToolsConfig};
use nanobot_cron::CronService;

/// Builder for ToolRegistry.
pub struct ToolRegistryBuilder;

#[bon::bon]
impl ToolRegistryBuilder {
    /// Builds the ToolRegistry.
    #[allow(clippy::new_ret_no_self)]
    #[builder(start_fn = new, finish_fn = build)]
    pub fn create(
        #[builder(start_fn)] workspace: PathBuf,
        #[builder(default)] restrict_to_workspace: bool,
        #[builder(default)] exec_config: ExecToolConfig,
        #[builder(default)] web_config: WebToolsConfig,
        bus: Option<MessageBus>,
        spawn_service: Option<Arc<dyn SpawnService>>,
        cron_service: Option<Arc<CronService>>,
        #[builder(default)] custom_tools: Vec<Arc<dyn Tool>>,
    ) -> ToolResult<ToolRegistry> {
        let registry = ToolRegistry::new(
            workspace,
            restrict_to_workspace,
            exec_config,
            web_config,
            bus,
            cron_service,
        );

        if let Some(service) = spawn_service {
            registry.set_spawn_service(service);
        }

        for tool in custom_tools {
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
    use crate::base::{JsonSchema, Tool, ToolContext, ToolDefinition};
    use crate::spawn::SpawnService;
    use nanobot_types::SessionKey;

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
        let exec_config = ExecToolConfig {
            timeout: 120,
            ..ExecToolConfig::default()
        };

        let registry = ToolRegistryBuilder::new(workspace)
            .restrict_to_workspace(true)
            .exec_config(exec_config)
            .build()
            .expect("build registry");

        let defs = registry.definitions();
        assert!(!defs.is_empty());
    }

    struct BuilderSpawnService;

    #[async_trait]
    impl SpawnService for BuilderSpawnService {
        async fn spawn(
            &self,
            task: String,
            label: Option<String>,
            _origin_channel: String,
            _origin_chat_id: String,
            _session_key: Option<SessionKey>,
        ) -> String {
            format!("spawned {} {:?}", task, label)
        }

        async fn cancel_by_session(&self, _session_key: &SessionKey) -> anyhow::Result<usize> {
            Ok(0)
        }
    }

    #[tokio::test]
    async fn builder_registers_spawn_tool_when_service_is_provided() {
        let workspace = std::env::temp_dir().join("test-registry-builder-spawn");
        let registry = ToolRegistryBuilder::new(workspace)
            .spawn_service(Arc::new(BuilderSpawnService))
            .build()
            .expect("build registry");

        let names: Vec<_> = registry
            .definitions()
            .into_iter()
            .map(|d| d.function.name.clone())
            .collect();
        assert!(names.contains(&"spawn".to_string()));

        let out = registry
            .execute(
                "spawn",
                r#"{"task":"test task","label":"demo"}"#,
                &ToolContext {
                    channel: "test".to_string(),
                    chat_id: "test".to_string(),
                    session_key: SessionKey::from("test:test"),
                    message_id: None,
                },
            )
            .await
            .expect("execute spawn tool");
        assert!(out.contains("spawned test task"));
    }

    struct BuilderEchoTool;

    #[async_trait]
    impl Tool for BuilderEchoTool {
        fn name(&self) -> &str {
            "builder_echo"
        }

        fn definition(&self) -> Arc<ToolDefinition> {
            Arc::new(ToolDefinition::function(
                self.name(),
                "Echo tool for builder tests",
                JsonSchema::object(BTreeMap::new(), Vec::new()),
            ))
        }

        async fn execute(&self, _args_json: &str, _ctx: &ToolContext) -> crate::ToolResult<String> {
            Ok("builder-ok".to_string())
        }
    }

    #[tokio::test]
    async fn builder_registers_custom_tool() {
        let workspace = std::env::temp_dir().join("test-registry-builder-custom-tool");
        let registry = ToolRegistryBuilder::new(workspace)
            .custom_tools(vec![Arc::new(BuilderEchoTool)])
            .build()
            .expect("build registry");

        let names: Vec<_> = registry
            .definitions()
            .into_iter()
            .map(|d| d.function.name.clone())
            .collect();
        assert!(names.contains(&"builder_echo".to_string()));

        let out = registry
            .execute(
                "builder_echo",
                "{}",
                &ToolContext {
                    channel: "test".to_string(),
                    chat_id: "test".to_string(),
                    session_key: SessionKey::from("test:test"),
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

        fn definition(&self) -> Arc<ToolDefinition> {
            Arc::new(ToolDefinition::function(
                self.name(),
                "Conflict tool for builder tests",
                JsonSchema::object(BTreeMap::new(), Vec::new()),
            ))
        }

        async fn execute(&self, _args_json: &str, _ctx: &ToolContext) -> crate::ToolResult<String> {
            Ok(String::new())
        }
    }

    #[test]
    fn builder_rejects_builtin_name_conflict_for_custom_tool() {
        let workspace = std::env::temp_dir().join("test-registry-builder-conflict-tool");
        let result = ToolRegistryBuilder::new(workspace)
            .custom_tools(vec![Arc::new(BuilderConflictTool)])
            .build();
        assert!(
            result.is_err(),
            "build should fail on builtin tool conflict"
        );
        let err = result.err().expect("error should be present");
        assert!(err.to_string().contains("conflicts with builtin"));
    }
}
