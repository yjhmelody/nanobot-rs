//! Shell execution tool with safety guards.
//!
//! Provides the `exec` tool that allows the LLM to run shell commands.
//! This is one of the most powerful (and dangerous) tools in the system,
//! so it includes multiple layers of safety guards.
//!
//! ## Safety Architecture
//!
//! Commands pass through three guard layers before execution:
//!
//! 1. **Pattern-based safety guard** (default: enabled, controlled by
//!    `disable_safety_guard`): Blocks destructive patterns like `rm -rf`,
//!    `mkfs`, `dd`, `shutdown`, etc. Uses regex matching on the command.
//! 2. **Workspace boundary guard** (default: enabled, controlled by
//!    `restrict_to_workspace`): Blocks path traversal (`../`), home path
//!    references (`~/`), and absolute paths that point outside the workspace.
//! 3. **Master switch** (`disable_all_guards`): When true, bypasses all
//!    guards entirely (for trusted environments).
//!
//! ## Execution model
//!
//! Commands are executed through `/bin/sh -lc` on Unix or `cmd /C` on
//! Windows for consistent behavior across platforms. Output is capped at
//! 10,000 bytes to keep LLM token usage manageable. Commands time out
//! after a configurable duration.

use std::path::Path;
use std::sync::Arc;
use std::sync::OnceLock;

use async_trait::async_trait;
use regex::Regex;
use serde_json::json;
use tokio::process::Command;
use tracing::{info, warn};

use crate::error::{ToolError, ToolResult};
use crate::tool_error;

use crate::base::{Tool, ToolContext, ToolDefinition, parse_args, tool_definition_from_json};
use crate::config::SharedToolConfig;
use nanobot_types::text::truncate_utf8_in_place;
use nanobot_types::tools::ExecArgs;

// Tool descriptions
const EXEC_DESC: &str = "Execute a shell command and return its output. Use with caution.";
const EXEC_COMMAND_DESC: &str = "The shell command to execute";
const EXEC_WORKING_DIR_DESC: &str = "Optional working directory for the command";
const TARGET: &str = "nanobot::tools::exec";

/// Tool for executing shell commands.
///
/// Wraps the underlying shell and enforces safety guards, timeouts,
/// and output size limits.
pub struct ShellTool {
    config: SharedToolConfig,
}

impl ShellTool {
    /// Creates a new `ShellTool` with the given shared configuration.
    pub fn new(config: SharedToolConfig) -> Self {
        Self { config }
    }
}

/// Returns the static tool definition (name: "exec").
///
/// Uses a `OnceLock` to cache the definition after first construction.
pub fn definition() -> Arc<ToolDefinition> {
    static DEF: OnceLock<Arc<ToolDefinition>> = OnceLock::new();
    DEF.get_or_init(|| {
        Arc::new(tool_definition_from_json(json!({
            "type": "function",
            "function": {
                "name": "exec",
                "description": EXEC_DESC,
                "parameters": {
                    "type": "object",
                    "properties": {
                        "command": {
                            "type": "string",
                            "description": EXEC_COMMAND_DESC
                        },
                        "working_dir": {
                            "type": "string",
                            "description": EXEC_WORKING_DIR_DESC
                        }
                    },
                    "required": ["command"]
                }
            }
        })))
    })
    .clone()
}

#[async_trait]
impl Tool for ShellTool {
    fn name(&self) -> &str {
        "exec"
    }

    fn definition(&self) -> Arc<ToolDefinition> {
        definition()
    }

    async fn execute(&self, args_json: &str, _ctx: &ToolContext) -> ToolResult<String> {
        let snapshot = self.config.snapshot().await;
        execute(
            args_json,
            snapshot.workspace.as_path(),
            snapshot.allowed_dir.as_deref(),
            snapshot.exec.timeout_secs,
            snapshot.exec.restrict_to_workspace,
            snapshot.exec.disable_safety_guard,
            snapshot.exec.disable_all_guards,
            &snapshot.exec.path_append,
        )
        .await
    }
}

/// Executes a shell command with the given configuration.
///
/// This is the main execution function, made public for use in tests and
/// direct invocation outside the [`Tool`] trait.
///
/// # Arguments
///
/// * `args_json` - JSON-serialized [`ExecArgs`].
/// * `default_working_dir` - Default working directory (usually workspace root).
/// * `allowed_dir` - If set, restricts command to this directory tree.
/// * `timeout_secs` - Maximum execution time in seconds.
/// * `restrict_to_workspace` - If true, enforces workspace boundary checks.
/// * `disable_safety_guard` - If true, skips pattern-based danger detection.
/// * `disable_all_guards` - If true, skips all safety checks.
/// * `path_append` - Additional PATH entries to append.
///
/// # Returns
///
/// Combined stdout and stderr output, capped at 10,000 bytes.
///
/// # Errors
///
/// Returns a tool error if:
/// - The command fails safety guard checks.
/// - The command cannot be spawned.
/// - The command times out.
#[allow(clippy::too_many_arguments)]
pub async fn execute(
    args_json: &str,
    default_working_dir: &Path,
    allowed_dir: Option<&Path>,
    timeout_secs: u64,
    restrict_to_workspace: bool,
    disable_safety_guard: bool,
    disable_all_guards: bool,
    path_append: &str,
) -> ToolResult<String> {
    let typed = parse_args::<ExecArgs>(args_json)?;
    let command = typed.command;
    let cwd = resolve_working_dir(
        typed.working_dir.as_deref(),
        default_working_dir,
        if restrict_to_workspace {
            allowed_dir
        } else {
            None
        },
    )?;

    guard_command(
        &command,
        &cwd,
        allowed_dir,
        restrict_to_workspace,
        disable_safety_guard,
        disable_all_guards,
    )?;

    // Keep the tool contract stable across platforms by always executing
    // through the platform-default non-interactive shell.
    let (program, shell_args) = platform_shell(&command);
    let mut cmd = Command::new(program);
    cmd.args(shell_args).current_dir(&cwd);
    cmd.kill_on_drop(true);

    if !path_append.trim().is_empty() {
        let old_path = std::env::var("PATH").unwrap_or_default();
        cmd.env("PATH", join_path_env(&old_path, path_append));
    }

    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    cmd.stdin(std::process::Stdio::null());

    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return Err(tool_error!("exec", "executing command: {}", e));
        }
    };

    info!(
        target: TARGET,
        command = %command,
        cwd = %cwd.display(),
        timeout_secs,
        "exec tool started command"
    );

    let output = match tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs),
        child.wait_with_output(),
    )
    .await
    {
        Ok(res) => match res {
            Ok(o) => o,
            Err(e) => {
                return Err(ToolError::execution(
                    "exec",
                    anyhow::anyhow!("waiting command output: {}", e),
                ));
            }
        },
        Err(_) => {
            warn!(
                target: TARGET,
                command = %command,
                cwd = %cwd.display(),
                timeout_secs,
                "exec tool timed out"
            );
            return Err(ToolError::execution(
                "exec",
                anyhow::anyhow!(
                    "command timed out after {} seconds (cwd={}, command={})",
                    timeout_secs,
                    cwd.display(),
                    command
                ),
            ));
        }
    };

    let mut parts = Vec::new();
    if !output.stdout.is_empty() {
        parts.push(String::from_utf8_lossy(&output.stdout).to_string());
    }
    if !output.stderr.is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        if !stderr.trim().is_empty() {
            parts.push(format!("STDERR:\n{}", stderr));
        }
    }
    if !output.status.success() {
        parts.push(format!(
            "\nExit code: {}",
            output.status.code().unwrap_or(-1)
        ));
    }

    let mut result = if parts.is_empty() {
        "(no output)".to_string()
    } else {
        parts.join("\n")
    };

    // Cap output at 10KB to avoid blowing the LLM context window.
    const MAX_LEN: usize = 10_000;
    if result.len() > MAX_LEN {
        let remaining = truncate_utf8_in_place(&mut result, MAX_LEN);
        result.push_str(&format!("\n... (truncated, {} more bytes)", remaining));
    }

    Ok(result)
}

/// Returns the platform-appropriate shell command.
///
/// - Unix: `/bin/sh -lc <command>` (login shell with PATH).
/// - Windows: `cmd /C <command>` (lowest common denominator).
fn platform_shell(command: &str) -> (&'static str, Vec<&str>) {
    #[cfg(target_os = "windows")]
    {
        // `cmd /C` is the lowest common denominator on GitHub-hosted Windows
        // runners and on end-user machines. It avoids assuming Git Bash exists.
        ("cmd", vec!["/C", command])
    }

    #[cfg(not(target_os = "windows"))]
    {
        ("/bin/sh", vec!["-lc", command])
    }
}

/// Joins an existing PATH with additional entries in a cross-platform way.
///
/// Uses `std::env::join_paths` for proper platform-specific path separator
/// handling, falling back to colon-joined strings if that fails.
fn join_path_env(existing: &str, append: &str) -> String {
    let existing_paths = std::env::split_paths(existing).collect::<Vec<_>>();
    let append_paths = std::env::split_paths(append).collect::<Vec<_>>();
    let joined = existing_paths
        .into_iter()
        .chain(append_paths)
        .collect::<Vec<_>>();
    std::env::join_paths(joined)
        .ok()
        .and_then(|value| value.into_string().ok())
        .unwrap_or_else(|| {
            if existing.trim().is_empty() {
                append.to_string()
            } else {
                format!("{}:{}", existing, append)
            }
        })
}

/// Applies safety guards to a command before execution.
///
/// The guard checks are applied in order:
/// 1. Master switch: if `disable_all_guards`, return immediately.
/// 2. Pattern guard: if `disable_safety_guard` is false, check against
///    known dangerous patterns.
/// 3. Workspace guard: if `restrict_to_workspace`, check for path
///    traversal, home references, and absolute paths outside the workspace.
fn guard_command(
    command: &str,
    cwd: &Path,
    allowed_dir: Option<&Path>,
    restrict_to_workspace: bool,
    disable_safety_guard: bool,
    disable_all_guards: bool,
) -> ToolResult<()> {
    if disable_all_guards {
        return Ok(());
    }

    // Pattern list for obviously destructive commands.
    let deny_patterns = [
        r"\brm\s+-[rf]{1,2}\b",
        r"\bdel\s+/[fq]\b",
        r"\brmdir\s+/s\b",
        r"(?:^|[;&|]\s*)format\b",
        r"\b(mkfs|diskpart)\b",
        r"\bdd\s+if=",
        r">\s*/dev/sd",
        r"\b(shutdown|reboot|poweroff)\b",
        r":\(\)\s*\{.*\};\s*:",
    ];

    if !disable_safety_guard {
        let lower = command.to_lowercase();
        for p in deny_patterns {
            // Pattern-based hard block for obviously destructive commands.
            if Regex::new(p)
                .ok()
                .map(|r| r.is_match(&lower))
                .unwrap_or(false)
            {
                return Err(ToolError::execution(
                    "exec",
                    anyhow::anyhow!("command blocked by safety guard (dangerous pattern detected)"),
                ));
            }
        }
    }

    if restrict_to_workspace {
        // Block path traversal patterns.
        if command.contains("../") || command.contains("..\\") {
            return Err(ToolError::execution(
                "exec",
                anyhow::anyhow!("command blocked by safety guard (path traversal detected)"),
            ));
        }
        // Block home path references.
        if command.contains("~/") || command.contains("~\\") {
            return Err(ToolError::execution(
                "exec",
                anyhow::anyhow!("command blocked by safety guard (home path detected)"),
            ));
        }

        let cwd = cwd.canonicalize().map_err(|e| {
            ToolError::execution(
                "exec",
                anyhow::anyhow!("canonicalizing cwd {}: {}", cwd.display(), e),
            )
        })?;
        if let Some(allowed_dir) = allowed_dir {
            let allowed_dir = allowed_dir.canonicalize().map_err(|e| {
                ToolError::execution(
                    "exec",
                    anyhow::anyhow!("canonicalizing workspace {}: {}", allowed_dir.display(), e),
                )
            })?;
            if cwd != allowed_dir && !cwd.starts_with(&allowed_dir) {
                return Err(ToolError::execution(
                    "exec",
                    anyhow::anyhow!("working_dir outside workspace"),
                ));
            }
        }
        // Best-effort scan for absolute paths referenced in the shell string.
        for abs in extract_absolute_paths(command) {
            let p = std::path::PathBuf::from(abs);
            if p.is_absolute()
                && let Ok(resolved) = p.canonicalize()
            {
                let base = if let Some(allowed_dir) = allowed_dir {
                    allowed_dir
                } else {
                    &cwd
                };
                if resolved != base && !resolved.starts_with(base) {
                    return Err(ToolError::execution(
                        "exec",
                        anyhow::anyhow!(
                            "command blocked by safety guard (path outside working dir)"
                        ),
                    ));
                }
            }
        }
    }

    Ok(())
}

/// Extracts absolute paths from a shell command string.
///
/// Handles both Windows-style (`C:\path`) and POSIX-style (`/path`)
/// absolute path syntaxes.
fn extract_absolute_paths(command: &str) -> Vec<String> {
    let mut paths = Vec::new();

    // Windows-style absolute paths, e.g. C:\\Users\\foo.
    let win = Regex::new(r#"[A-Za-z]:\\[^\s\"'|><;]+"#).expect("invalid regex");
    for m in win.find_iter(command) {
        paths.push(m.as_str().to_string());
    }

    // POSIX-style absolute paths, e.g. /tmp/a.txt.
    let posix = Regex::new(r#"(?:^|[\s|>])(/[^\s\"'>]+)"#).expect("invalid regex");
    for cap in posix.captures_iter(command) {
        if let Some(m) = cap.get(1) {
            paths.push(m.as_str().to_string());
        }
    }

    paths
}

/// Resolves the working directory for a command.
///
/// If `working_dir` is provided and absolute, uses it directly.
/// If relative, joins it against the workspace. Falls back to
/// the workspace if no `working_dir` is specified.
///
/// When `allowed_dir` is set, validates that the resolved directory
/// is within the allowed boundary.
fn resolve_working_dir(
    working_dir: Option<&str>,
    workspace: &Path,
    allowed_dir: Option<&Path>,
) -> ToolResult<std::path::PathBuf> {
    let raw_path = match working_dir {
        Some(value) => std::path::PathBuf::from(value),
        None => workspace.to_path_buf(),
    };
    let candidate = if raw_path.is_absolute() {
        raw_path
    } else {
        workspace.join(raw_path)
    };

    let resolved = candidate.canonicalize().map_err(|e| {
        ToolError::execution(
            "exec",
            anyhow::anyhow!("canonicalizing working_dir {}: {}", candidate.display(), e),
        )
    })?;

    if let Some(allowed_dir) = allowed_dir {
        let allowed_dir = allowed_dir.canonicalize().map_err(|e| {
            ToolError::execution(
                "exec",
                anyhow::anyhow!("canonicalizing workspace {}: {}", allowed_dir.display(), e),
            )
        })?;
        if resolved != allowed_dir && !resolved.starts_with(&allowed_dir) {
            return Err(ToolError::execution(
                "exec",
                anyhow::anyhow!("working_dir outside workspace"),
            ));
        }
    }

    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_exec_args_with_working_dir() {
        let json = r#"{"command":"echo ok","working_dir":"/tmp"}"#;
        let args: ExecArgs = crate::base::parse_args(json).expect("parse exec args");
        assert_eq!(args.command, "echo ok");
        assert_eq!(args.working_dir.as_deref(), Some("/tmp"));
    }

    #[test]
    fn guard_blocks_path_traversal_when_restricted() {
        let cwd = std::path::PathBuf::from("/tmp");
        let blocked = guard_command("cat ../secret.txt", &cwd, Some(&cwd), true, false, false);
        assert!(blocked.is_err());
        assert!(
            blocked
                .err()
                .map(|e| e.to_string())
                .unwrap_or_default()
                .contains("path traversal")
        );
    }

    #[test]
    fn guard_allows_safe_command() {
        let cwd = std::path::PathBuf::from("/tmp");
        let blocked = guard_command("echo hello", &cwd, None, false, false, false);
        assert!(blocked.is_ok());
    }

    #[test]
    fn guard_allows_dangerous_pattern_when_safety_guard_disabled() {
        let cwd = std::path::PathBuf::from("/tmp");
        let blocked = guard_command("rm -rf build", &cwd, None, false, true, false);
        assert!(blocked.is_ok());
    }

    #[test]
    fn guard_allows_all_when_all_guards_disabled() {
        let cwd = std::path::PathBuf::from("/tmp");
        let blocked = guard_command("cat ../secret.txt", &cwd, Some(&cwd), true, false, true);
        assert!(blocked.is_ok());
    }

    #[test]
    fn join_path_env_uses_os_separator() {
        let joined = join_path_env("first", "second");
        let parts = std::env::split_paths(&joined)
            .map(|p| p.to_string_lossy().to_string())
            .collect::<Vec<_>>();
        assert_eq!(parts, vec!["first".to_string(), "second".to_string()]);
    }

    #[test]
    fn resolve_working_dir_rejects_outside_workspace() {
        let workspace = std::env::temp_dir().join("nanobot-shell-ws");
        std::fs::create_dir_all(&workspace).expect("create workspace");
        let outside = std::env::temp_dir().join("nanobot-shell-out");
        std::fs::create_dir_all(&outside).expect("create outside");

        let resolved = resolve_working_dir(
            Some(outside.to_str().expect("outside str")),
            workspace.as_path(),
            Some(workspace.as_path()),
        );
        assert!(resolved.is_err());
    }

    #[test]
    fn truncate_utf8_respects_char_boundaries() {
        let mut value = "hello你好世界".to_string();
        let remaining = truncate_utf8_in_place(&mut value, 8);
        assert_eq!(value, "hello你");
        assert_eq!(remaining, "好世界".len());
    }
}
