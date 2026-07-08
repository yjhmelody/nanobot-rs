//! Central tool registry for dispatching tool calls.
//!
//! The [`ToolRegistry`] is the main entry point for the agent loop to
//! discover and invoke tools. It holds all registered tools (built-in
//! and dynamic) and provides a uniform `execute(name, args, ctx)` API.
//!
//! ## Registration model
//!
//! - **Built-in tools** are statically defined at construction time:
//!   filesystem, shell, web, search. Their names are immutable
//!   and protected from accidental override.
//! - **Dynamic tools** are registered at runtime by MCP servers or user
//!   code via [`register_dynamic_tool`](ToolRegistry::register_dynamic_tool).
//! - The **spawn** tool is registered lazily via
//!   [`set_spawn_service`](ToolRegistry::set_spawn_service) to break
//!   circular dependency chains.
//!
//! ## Thread safety
//!
//! Uses `parking_lot::RwLock` to protect the tool map, since lookups
//! are frequent, critical sections are short, and no await points occur
//! while holding the lock.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use crate::base::{Tool, ToolContext, ToolDefinition};
use crate::config::SharedToolConfig;
use crate::cron::CronTool;
use crate::error::{ToolError, ToolResult};
use crate::spawn::SpawnService;
use crate::spawn::SpawnTool;
use crate::{filesystem, search, shell, web};
use nanobot_config::{ExecToolConfig, WebToolsConfig};
use nanobot_cron::CronService;
use nanobot_types::SessionKey;
use parking_lot::RwLock;

/// Central dispatcher for all agent tools.
///
/// Manages the lifecycle and execution of built-in and dynamically
/// registered tools. The agent loop holds one `ToolRegistry` instance
/// and uses it for all tool interactions.
///
/// ## Locking
///
/// Uses `parking_lot::RwLock` for the tool map because:
/// - Tool lookups are frequent (every LLM turn).
/// - Critical sections are short (just a HashMap lookup).
/// - No await points inside the locked section.
///
/// ## Example
///
/// ```no_run
/// use nanobot_tools::ToolRegistry;
/// use nanobot_tools::ToolContext;
/// use nanobot_types::SessionKey;
///
/// # async fn example(registry: &ToolRegistry) -> nanobot_tools::ToolResult<()> {
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
pub struct ToolRegistry {
    /// Map of tool name to tool implementation, protected by RwLock.
    tools: RwLock<HashMap<String, Arc<dyn Tool>>>,
    /// Names of built-in tools (immutable, protected from override).
    builtin_names: HashSet<String>,
    /// Shared runtime configuration for all tools.
    config: SharedToolConfig,
}

impl ToolRegistry {
    /// Creates a new `ToolRegistry` with all built-in tools.
    ///
    /// # Arguments
    ///
    /// * `workspace` - Base directory for file operations.
    /// * `restrict_to_workspace` - If true, file/exec are confined to the workspace.
    /// * `exec_config` - Shell execution configuration.
    /// * `web_config` - Web search/fetch configuration.
    /// * `cron_service` - Optional cron service for the cron tool.
    ///
    /// The spawn tool is **not** registered here; it must be set separately
    /// via [`set_spawn_service`](ToolRegistry::set_spawn_service) to avoid
    /// circular dependencies during construction.
    pub(crate) fn new(
        workspace: PathBuf,
        restrict_to_workspace: bool,
        exec_config: ExecToolConfig,
        web_config: WebToolsConfig,
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

    /// Returns the definition of every registered tool, sorted by name.
    ///
    /// The agent loop uses this list to inform the LLM of available tools
    /// at each turn.
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

    /// Registers a dynamic tool (typically from an MCP server).
    ///
    /// # Errors
    ///
    /// Returns a configuration error if:
    /// - The tool name conflicts with a built-in tool name.
    /// - The tool name is already registered.
    pub fn register_dynamic_tool(&self, tool: Arc<dyn Tool>) -> ToolResult<()> {
        let name = tool.name().to_string();
        if self.builtin_names.contains(&name) {
            return Err(ToolError::config(format!(
                "tool '{}' conflicts with builtin tool name",
                name
            )));
        }
        let mut guard = self.tools.write();
        if guard.contains_key(&name) {
            return Err(ToolError::config(format!(
                "tool '{}' already registered",
                name
            )));
        }
        guard.insert(name, tool);
        Ok(())
    }

    /// Unregisters a dynamic tool by name.
    ///
    /// Built-in tools cannot be unregistered; calls for built-in names
    /// are silently ignored.
    pub fn unregister_dynamic_tool(&self, name: &str) {
        if self.builtin_names.contains(name) {
            return;
        }
        self.tools.write().remove(name);
    }

    /// Sets the spawn service after initial construction.
    ///
    /// Registers the `spawn` tool in the registry. This is deferred to
    /// break circular dependencies between `ToolRegistry` and the subagent
    /// manager (which needs the registry).
    pub fn set_spawn_service(&self, service: Arc<dyn SpawnService>) {
        let spawn_tool: Arc<dyn Tool> = Arc::new(SpawnTool::new(service));
        self.tools
            .write()
            .insert(spawn_tool.name().to_string(), spawn_tool);
    }

    /// Cancels all spawned tasks associated with a session.
    ///
    /// Called when a session ends or is interrupted. Returns the total
    /// number of tasks that were cancelled.
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
    /// This is the main entry point for tool execution, called by the
    /// agent loop.
    ///
    /// # Arguments
    ///
    /// * `name` - Tool name (e.g., "read_file", "exec", or dynamic tool name).
    /// * `args_json` - JSON string containing tool arguments.
    /// * `ctx` - Runtime context containing channel, chat_id, session_key, and message_id.
    ///
    /// # Returns
    ///
    /// The tool execution result as a string.
    ///
    /// # Errors
    ///
    /// * [`ToolError::NotFound`] if the tool name is not registered.
    /// * Delegated to the tool's `execute` method for argument parsing and
    ///   execution errors.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use nanobot_tools::ToolRegistry;
    /// # use nanobot_tools::ToolContext;
    /// # use nanobot_types::SessionKey;
    /// # async fn example(registry: &ToolRegistry) -> nanobot_tools::ToolResult<()> {
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
    pub async fn execute(
        &self,
        name: &str,
        args_json: &str,
        ctx: &ToolContext,
    ) -> ToolResult<String> {
        let tool = self.tools.read().get(name).cloned();
        if let Some(tool) = tool {
            tool.execute(args_json, ctx).await
        } else {
            Err(ToolError::not_found(name.to_string()))
        }
    }

    /// Returns a reference to the shared configuration for runtime modification.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use nanobot_tools::ToolRegistry;
    /// # async fn example(registry: &ToolRegistry) {
    /// let config = registry.config();
    /// config.set_exec_timeout(120).await;
    /// # }
    /// ```
    pub fn config(&self) -> &SharedToolConfig {
        &self.config
    }

    /// Updates the shell execution timeout at runtime.
    ///
    /// All subsequent shell commands will use the new timeout.
    pub async fn set_exec_timeout(&self, timeout_secs: u64) {
        self.config.set_exec_timeout(timeout_secs).await;
    }

    /// Enables or disables workspace restriction at runtime.
    ///
    /// When enabled, all file operations are restricted to the workspace
    /// directory. When disabled, file operations can access any path.
    pub async fn set_restrict_to_workspace(&self, restrict: bool) {
        self.config.set_restrict_to_workspace(restrict).await;
    }

    /// Updates the workspace directory at runtime.
    ///
    /// All subsequent file operations will use the new workspace as the
    /// base directory for resolving relative paths.
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

    use crate::base::{JsonSchema, ToolDefinition};
    use nanobot_types::SessionKey;

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
        );
        let names = definition_names(reg.definitions());
        assert!(!names.contains("spawn"));
        assert!(!names.contains("cron"));
        assert!(names.contains("exec"));
    }

    #[tokio::test]
    async fn registry_with_optional_tools_includes_spawn_and_cron() {
        use crate::spawn::SpawnService;
        use nanobot_cron::CronService;
        use nanobot_types::SessionKey;

        struct MockSpawn;
        #[async_trait::async_trait]
        impl SpawnService for MockSpawn {
            async fn spawn(
                &self,
                task: String,
                _: Option<String>,
                _: String,
                _: String,
                _: Option<SessionKey>,
            ) -> String {
                format!("spawned: {}", task)
            }
            async fn cancel_by_session(&self, _: &SessionKey) -> anyhow::Result<usize> {
                Ok(0)
            }
        }

        let workspace = std::env::temp_dir().join("nanobot-reg-with-optional");
        let cron_store = workspace.join("cron.json");
        let cron = Arc::new(CronService::new(cron_store));

        // Create registry without spawn service initially
        let reg = ToolRegistry::new(
            workspace.clone(),
            false,
            ExecToolConfig::default(),
            WebToolsConfig::default(),
            Some(cron),
        );

        // Verify cron is registered but spawn is not yet
        let names = definition_names(reg.definitions());
        assert!(names.contains("cron"));
        assert!(!names.contains("spawn"));

        // Now set the spawn service using a mock
        let reg2 = ToolRegistry::new(
            std::env::temp_dir().join("nanobot-reg-with-spawn"),
            false,
            ExecToolConfig::default(),
            WebToolsConfig::default(),
            None,
        );
        reg2.set_spawn_service(Arc::new(MockSpawn));

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

        async fn execute(&self, _args_json: &str, _ctx: &ToolContext) -> ToolResult<String> {
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
            message_id: Some(nanobot_types::bus::MessageId::External("m1".to_string())),
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

        async fn execute(&self, _args_json: &str, _ctx: &ToolContext) -> ToolResult<String> {
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
        );
        let err = reg
            .register_dynamic_tool(Arc::new(BuiltinConflictTool))
            .expect_err("builtin name conflict should fail");
        assert!(err.to_string().contains("conflicts with builtin"));
    }
}
