use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use crate::agent::SpawnService;
use crate::bus::MessageBus;
use crate::config::schema::{ExecToolConfig, WebToolsConfig};
use crate::cron::CronService;
use crate::error::{NanobotError, Result};
use crate::tools::base::{Tool, ToolContext, ToolDefinition};
use crate::tools::config::SharedToolConfig;
use crate::tools::cron::CronTool;
use crate::tools::message::MessageTool;
use crate::tools::spawn::SpawnTool;
use crate::tools::{filesystem, search, shell, web};
use crate::types::SessionKey;
use parking_lot::RwLock;

/// Central dispatcher for built-in tools.
///
/// The registry keeps runtime dependencies (workspace, configs, optional services)
/// and exposes a uniform `execute(name, args_json)` API to the agent loop.
pub struct ToolRegistry {
    tools: RwLock<HashMap<String, Arc<dyn Tool>>>,
    builtin_names: HashSet<String>,
    config: SharedToolConfig,
}

impl ToolRegistry {
    pub(crate) fn new(
        workspace: PathBuf,
        restrict_to_workspace: bool,
        exec_config: ExecToolConfig,
        web_config: WebToolsConfig,
        bus: Option<MessageBus>,
        cron_service: Option<Arc<CronService>>,
    ) -> Self {
        let config =
            SharedToolConfig::new(workspace, restrict_to_workspace, exec_config, web_config);

        let mut tools: HashMap<String, Arc<dyn Tool>> = HashMap::new();

        let builtin_tools: Vec<Arc<dyn Tool>> = vec![
            Arc::new(filesystem::ReadFileTool::new(config.clone())),
            Arc::new(filesystem::WriteFileTool::new(config.clone())),
            Arc::new(filesystem::EditFileTool::new(config.clone())),
            Arc::new(filesystem::ListDirTool::new(config.clone())),
            Arc::new(shell::ShellTool::new(config.clone())),
            Arc::new(web::WebSearchTool::new(config.clone())),
            Arc::new(web::WebFetchTool::new(config.clone())),
            Arc::new(search::SearchFilesTool::new(config.clone())),
            Arc::new(search::GrepCodeTool::new(config.clone())),
        ];

        for tool in builtin_tools {
            tools.insert(tool.name().to_string(), tool);
        }

        let message_tool: Arc<dyn Tool> = Arc::new(MessageTool::new(bus));
        tools.insert(message_tool.name().to_string(), message_tool);

        if let Some(service) = cron_service {
            let cron_tool: Arc<dyn Tool> = Arc::new(CronTool::new(service));
            tools.insert(cron_tool.name().to_string(), cron_tool);
        }

        let builtin_names = tools.keys().cloned().collect::<HashSet<_>>();

        Self {
            tools: RwLock::new(tools),
            builtin_names,
            config,
        }
    }

    pub fn definitions(&self) -> Vec<Arc<ToolDefinition>> {
        let mut defs = self
            .tools
            .read()
            .values()
            .map(|t| t.definition())
            .collect::<Vec<_>>();
        defs.sort_by(|a, b| a.function.name.cmp(&b.function.name));
        defs
    }

    pub fn register_dynamic_tool(&self, tool: Arc<dyn Tool>) -> Result<()> {
        let name = tool.name().to_string();
        if self.builtin_names.contains(&name) {
            return Err(NanobotError::config(format!(
                "tool '{}' conflicts with builtin tool name",
                name
            )));
        }
        let mut guard = self.tools.write();
        if guard.contains_key(&name) {
            return Err(NanobotError::config(format!(
                "tool '{}' already registered",
                name
            )));
        }
        guard.insert(name, tool);
        Ok(())
    }

    pub fn unregister_dynamic_tool(&self, name: &str) {
        if self.builtin_names.contains(name) {
            return;
        }
        self.tools.write().remove(name);
    }

    /// Sets the spawn service after initial construction.
    ///
    /// This allows setting the spawn service after registry creation,
    /// which is useful for breaking circular dependencies.
    pub fn set_spawn_service(&self, service: Arc<dyn SpawnService>) {
        let spawn_tool: Arc<dyn Tool> = Arc::new(SpawnTool::new(service));
        self.tools
            .write()
            .insert(spawn_tool.name().to_string(), spawn_tool);
    }

    pub async fn start_turn(&self) -> Result<()> {
        let snapshot = self.tools.read().values().cloned().collect::<Vec<_>>();
        for tool in snapshot {
            tool.start_turn().await?;
        }
        Ok(())
    }

    pub async fn message_sent_in_turn(&self) -> bool {
        let snapshot = self.tools.read().values().cloned().collect::<Vec<_>>();
        for tool in snapshot {
            if tool.sent_in_turn().await.unwrap_or(false) {
                return true;
            }
        }
        false
    }

    pub async fn cancel_spawn_by_session(&self, session_key: &SessionKey) -> usize {
        let mut cancelled = 0usize;
        let snapshot = self.tools.read().values().cloned().collect::<Vec<_>>();
        for tool in snapshot {
            cancelled += tool
                .cancel_by_session(session_key.as_str())
                .await
                .unwrap_or(0);
        }
        cancelled
    }

    /// Executes a tool by name with JSON arguments and runtime context.
    ///
    /// This is the main entry point for tool execution.
    ///
    /// # Arguments
    ///
    /// * `name` - Tool name (e.g., "read_file", "exec", or dynamic tool name)
    /// * `args_json` - JSON string containing tool arguments
    /// * `ctx` - Runtime context containing channel, chat_id, session_key, and message_id
    ///
    /// # Returns
    ///
    /// Returns the tool execution result as a string.
    ///
    /// # Errors
    ///
    /// * Returns an error if the tool name is not registered
    /// * Returns an error if args_json cannot be parsed
    /// * Returns an error if tool execution fails
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use nanobot_rs::tools::registry::ToolRegistry;
    /// # use nanobot_rs::tools::base::ToolContext;
    /// # use nanobot_rs::types::SessionKey;
    /// # async fn example(registry: &ToolRegistry) -> nanobot_rs::error::Result<()> {
    /// let ctx = ToolContext {
    ///     channel: "cli".to_string(),
    ///     chat_id: "direct".to_string(),
    ///     session_key: SessionKey::from("cli:direct"),
    ///     message_id: None,
    /// };
    /// let result = registry.execute("read_file", r#"{"path": "/tmp/test.txt"}"#, &ctx).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn execute(&self, name: &str, args_json: &str, ctx: &ToolContext) -> Result<String> {
        let tool = self.tools.read().get(name).cloned();
        if let Some(tool) = tool {
            tool.execute(args_json, ctx).await
        } else {
            Err(NanobotError::ToolNotFound(name.to_string()))
        }
    }

    /// Get shared configuration for runtime modification.
    ///
    /// Returns a reference to the shared configuration that can be used to
    /// modify tool settings at runtime.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use nanobot_rs::tools::registry::ToolRegistry;
    /// # async fn example(registry: &ToolRegistry) {
    /// let config = registry.config();
    /// config.set_exec_timeout(120).await;
    /// # }
    /// ```
    pub fn config(&self) -> &SharedToolConfig {
        &self.config
    }

    /// Update exec timeout at runtime.
    ///
    /// This is a convenience method that updates the timeout for shell execution.
    /// All subsequent shell commands will use the new timeout.
    pub async fn set_exec_timeout(&self, timeout_secs: u64) {
        self.config.set_exec_timeout(timeout_secs).await;
    }

    /// Update workspace restriction at runtime.
    ///
    /// When enabled, all file operations are restricted to the workspace directory.
    /// When disabled, file operations can access any path.
    pub async fn set_restrict_to_workspace(&self, restrict: bool) {
        self.config.set_restrict_to_workspace(restrict).await;
    }

    /// Update workspace directory at runtime.
    ///
    /// All subsequent file operations will use the new workspace as the base directory.
    pub async fn set_workspace(&self, workspace: PathBuf) {
        self.config.set_workspace(workspace).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::collections::HashSet;

    use async_trait::async_trait;

    use crate::provider::{ChatRequest, LLMProvider, LLMResponse, UsageStats};
    use crate::tools::base::{JsonSchema, ToolDefinition};
    use crate::types::SessionKey;

    #[allow(unused)]
    struct DummyProvider;

    #[async_trait]
    impl LLMProvider for DummyProvider {
        async fn chat(&self, _req: ChatRequest) -> LLMResponse {
            LLMResponse {
                content: Some("dummy".to_string()),
                tool_calls: Vec::new(),
                finish_reason: "stop".to_string(),
                usage: UsageStats::default(),
                reasoning_content: None,
                thinking_blocks: None,
            }
        }

        fn default_model(&self) -> &str {
            "openai/gpt-4o-mini"
        }
    }

    fn definition_names(defs: Vec<Arc<ToolDefinition>>) -> HashSet<String> {
        defs.into_iter().map(|d| d.function.name.clone()).collect()
    }

    #[tokio::test]
    async fn registry_without_optional_tools_excludes_spawn_and_cron() {
        let reg = ToolRegistry::new(
            std::env::temp_dir().join("nanobot-reg-no-optional"),
            false,
            ExecToolConfig::default(),
            WebToolsConfig::default(),
            None,
            None,
        );
        let names = definition_names(reg.definitions());
        assert!(!names.contains("spawn"));
        assert!(!names.contains("cron"));
        assert!(names.contains("message"));
        assert!(names.contains("exec"));
    }

    #[tokio::test]
    async fn registry_with_optional_tools_includes_spawn_and_cron() {
        use crate::agent::SubagentManager;
        use crate::bus::MessageBus;
        use crate::cron::CronService;

        let workspace = std::env::temp_dir().join("nanobot-reg-with-optional");
        let bus = MessageBus::new();
        let cron_store = workspace.join("cron.json");
        let cron = Arc::new(CronService::new(cron_store));

        // Create registry without spawn service initially
        let reg = ToolRegistry::new(
            workspace.clone(),
            false,
            ExecToolConfig::default(),
            WebToolsConfig::default(),
            Some(bus.clone()),
            Some(cron),
        );

        // Verify cron is registered but spawn is not yet
        let names = definition_names(reg.definitions());
        assert!(names.contains("cron"));
        assert!(!names.contains("spawn"));

        // Create SubagentManager with the registry
        let provider = Arc::new(DummyProvider);
        let subagent_manager = Arc::new(SubagentManager::new(
            provider,
            workspace,
            bus,
            Arc::new(reg),
            "test/model".to_string(),
            0.7,
            1000,
            None,
        ));

        // Now set the spawn service
        let reg2 = ToolRegistry::new(
            std::env::temp_dir().join("nanobot-reg-with-spawn"),
            false,
            ExecToolConfig::default(),
            WebToolsConfig::default(),
            None,
            None,
        );
        reg2.set_spawn_service(subagent_manager);

        // Verify spawn is now registered
        let names = definition_names(reg2.definitions());
        assert!(names.contains("spawn"));
    }

    struct EchoDynamicTool {
        value: String,
    }

    impl EchoDynamicTool {
        fn new(value: &str) -> Self {
            Self {
                value: value.to_string(),
            }
        }
    }

    #[async_trait]
    impl Tool for EchoDynamicTool {
        fn name(&self) -> &str {
            "dynamic_echo"
        }

        fn definition(&self) -> Arc<ToolDefinition> {
            Arc::new(ToolDefinition::function(
                self.name(),
                "Echoes a constant value.",
                JsonSchema::object(BTreeMap::new(), Vec::new()),
            ))
        }

        async fn execute(&self, _args_json: &str, _ctx: &ToolContext) -> Result<String> {
            Ok(self.value.clone())
        }
    }

    #[tokio::test]
    async fn dynamic_tool_register_execute_and_unregister() {
        let reg = ToolRegistry::new(
            std::env::temp_dir().join("nanobot-reg-dynamic"),
            false,
            ExecToolConfig::default(),
            WebToolsConfig::default(),
            None,
            None,
        );

        let tool = Arc::new(EchoDynamicTool::new("ok"));
        reg.register_dynamic_tool(tool.clone())
            .expect("register dynamic tool");

        let names = definition_names(reg.definitions());
        assert!(names.contains("dynamic_echo"));

        let ctx = ToolContext {
            channel: "cli".to_string(),
            chat_id: "direct".to_string(),
            session_key: SessionKey::from("cli:direct"),
            message_id: Some("m1".to_string()),
        };

        let out = reg
            .execute("dynamic_echo", "{}", &ctx)
            .await
            .expect("execute");
        assert_eq!(out, "ok");

        reg.unregister_dynamic_tool("dynamic_echo");
        let names = definition_names(reg.definitions());
        assert!(!names.contains("dynamic_echo"));
    }

    struct BuiltinConflictTool;

    #[async_trait]
    impl Tool for BuiltinConflictTool {
        fn name(&self) -> &str {
            "exec"
        }

        fn definition(&self) -> Arc<ToolDefinition> {
            Arc::new(ToolDefinition::function(
                self.name(),
                "conflicts on purpose",
                JsonSchema::object(BTreeMap::new(), Vec::new()),
            ))
        }

        async fn execute(&self, _args_json: &str, _ctx: &ToolContext) -> Result<String> {
            Ok(String::new())
        }
    }

    #[test]
    fn dynamic_tool_cannot_override_builtin_name() {
        let reg = ToolRegistry::new(
            std::env::temp_dir().join("nanobot-reg-conflict"),
            false,
            ExecToolConfig::default(),
            WebToolsConfig::default(),
            None,
            None,
        );
        let err = reg
            .register_dynamic_tool(Arc::new(BuiltinConflictTool))
            .expect_err("builtin name conflict should fail");
        assert!(err.to_string().contains("conflicts with builtin"));
    }
}
