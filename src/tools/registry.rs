use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::RwLock;

use anyhow::{Result, anyhow, bail};

use crate::agent::SubagentManager;
use crate::bus::MessageBus;
use crate::config::schema::{ExecToolConfig, WebToolsConfig};
use crate::cron::CronService;
use crate::tools::base::{Tool, ToolContext, ToolDefinition};
use crate::tools::cron::CronTool;
use crate::tools::message::MessageTool;
use crate::tools::shared_config::SharedToolConfig;
use crate::tools::spawn::SpawnTool;
use crate::tools::{filesystem, shell, web};

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
        bus: Option<Arc<MessageBus>>,
        spawn_manager: Option<Arc<SubagentManager>>,
        cron_service: Option<Arc<CronService>>,
    ) -> Self {
        let config =
            SharedToolConfig::new(workspace, restrict_to_workspace, exec_config, web_config);

        let mut tools: HashMap<String, Arc<dyn Tool>> = HashMap::new();
        for tool in filesystem::build_tools(config.clone()) {
            tools.insert(tool.name().to_string(), tool);
        }

        let shell_tool = shell::build_tool(config.clone());
        tools.insert(shell_tool.name().to_string(), shell_tool);

        for tool in web::build_tools(config.clone()) {
            tools.insert(tool.name().to_string(), tool);
        }

        let message_tool: Arc<dyn Tool> = Arc::new(MessageTool::new(bus));
        tools.insert(message_tool.name().to_string(), message_tool);

        if let Some(manager) = spawn_manager {
            let spawn_tool: Arc<dyn Tool> = Arc::new(SpawnTool::new(manager));
            tools.insert(spawn_tool.name().to_string(), spawn_tool);
        }

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

    pub fn definitions(&self) -> Vec<ToolDefinition> {
        let mut defs = self
            .tools
            .read()
            .ok()
            .map(|map| map.values().map(|t| t.definition()).collect::<Vec<_>>())
            .unwrap_or_default();
        defs.sort_unstable_by(|a, b| a.function.name.cmp(&b.function.name));
        defs
    }

    pub fn register_dynamic_tool(&self, tool: Arc<dyn Tool>) -> Result<()> {
        let name = tool.name().to_string();
        if self.builtin_names.contains(&name) {
            bail!("tool '{}' conflicts with builtin tool name", name);
        }
        let mut guard = self
            .tools
            .write()
            .map_err(|_| anyhow!("tool registry poisoned"))?;
        if guard.contains_key(&name) {
            bail!("tool '{}' already registered", name);
        }
        guard.insert(name, tool);
        Ok(())
    }

    pub fn unregister_dynamic_tool(&self, name: &str) {
        if self.builtin_names.contains(name) {
            return;
        }
        if let Ok(mut guard) = self.tools.write() {
            guard.remove(name);
        }
    }

    /// Sets the spawn manager after initial construction.
    ///
    /// This is used to break the circular dependency between ToolRegistry and SubagentManager.
    pub fn set_spawn_manager(&self, manager: Arc<SubagentManager>) {
        let spawn_tool: Arc<dyn Tool> = Arc::new(SpawnTool::new(manager));
        if let Ok(mut guard) = self.tools.write() {
            guard.insert(spawn_tool.name().to_string(), spawn_tool);
        }
    }

    /// Sets the runtime context for all tools.
    ///
    /// This propagates context information (channel, chat_id, session_key, message_id)
    /// to all stateful tools that need it for execution.
    ///
    /// # Arguments
    ///
    /// * `ctx` - Runtime context containing channel and session information
    ///
    /// # Deprecated
    ///
    /// This method is deprecated. Context is now passed directly to execute().
    #[deprecated(note = "Context is now passed directly to execute()")]
    pub async fn set_context(&self, _ctx: ToolContext) {
        // No-op: context is now passed to execute()
    }

    pub async fn start_turn(&self) {
        let snapshot = self
            .tools
            .read()
            .ok()
            .map(|m| m.values().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        for tool in snapshot {
            let _ = tool.start_turn().await;
        }
    }

    pub async fn message_sent_in_turn(&self) -> bool {
        let snapshot = self
            .tools
            .read()
            .ok()
            .map(|m| m.values().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        for tool in snapshot {
            if tool.sent_in_turn().await.unwrap_or(false) {
                return true;
            }
        }
        false
    }

    pub async fn cancel_spawn_by_session(&self, session_key: &str) -> usize {
        let mut cancelled = 0usize;
        let snapshot = self
            .tools
            .read()
            .ok()
            .map(|m| m.values().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        for tool in snapshot {
            cancelled += tool.cancel_by_session(session_key).await.unwrap_or(0);
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
    /// # async fn example(registry: &ToolRegistry) -> anyhow::Result<()> {
    /// let ctx = ToolContext {
    ///     channel: "cli".to_string(),
    ///     chat_id: "direct".to_string(),
    ///     session_key: "cli:direct".to_string(),
    ///     message_id: None,
    /// };
    /// let result = registry.execute("read_file", r#"{"path": "/tmp/test.txt"}"#, &ctx).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn execute(&self, name: &str, args_json: &str, ctx: &ToolContext) -> Result<String> {
        let tool = self.tools.read().ok().and_then(|m| m.get(name).cloned());
        if let Some(tool) = tool {
            tool.execute(args_json, ctx).await
        } else {
            bail!("tool '{}' not found", name)
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

    fn definition_names(defs: Vec<ToolDefinition>) -> HashSet<String> {
        defs.into_iter().map(|d| d.function.name).collect()
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
            None,
        );
        let names = definition_names(reg.definitions());
        assert!(!names.contains("spawn"));
        assert!(!names.contains("cron"));
        assert!(names.contains("message"));
        assert!(names.contains("exec"));
    }

    #[tokio::test]
    #[ignore] // TODO: Update test after SubagentManager refactoring
    async fn registry_with_optional_tools_includes_spawn_and_cron() {
        // This test needs to be updated to work with the new SubagentManager
        // that requires ToolRegistry, which creates a circular dependency in tests.
        // The spawn tool is now added via set_spawn_manager() after registry creation.
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

        fn definition(&self) -> ToolDefinition {
            ToolDefinition::function(
                self.name(),
                "Echoes a constant value.",
                JsonSchema::object(BTreeMap::new(), Vec::new()),
            )
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
            session_key: "cli:direct".to_string(),
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

        fn definition(&self) -> ToolDefinition {
            ToolDefinition::function(
                self.name(),
                "conflicts on purpose",
                JsonSchema::object(BTreeMap::new(), Vec::new()),
            )
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
            None,
        );
        let err = reg
            .register_dynamic_tool(Arc::new(BuiltinConflictTool))
            .expect_err("builtin name conflict should fail");
        assert!(err.to_string().contains("conflicts with builtin"));
    }
}
