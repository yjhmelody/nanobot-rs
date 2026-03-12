use std::path::Path;
use std::sync::Arc;
use std::sync::OnceLock;

use async_trait::async_trait;
use regex::Regex;
use serde_json::json;
use tokio::process::Command;

use crate::error::{NanobotError, Result};
use crate::tool_error;
use crate::tools::base::{
    Tool, ToolContext, ToolDefinition, parse_args, tool_definition_from_json,
};
use crate::tools::config::SharedToolConfig;
use crate::types::tools::ExecArgs;

// Tool descriptions
const EXEC_DESC: &str = "Execute a shell command and return its output. Use with caution.";
const EXEC_COMMAND_DESC: &str = "The shell command to execute";
const EXEC_WORKING_DIR_DESC: &str = "Optional working directory for the command";

pub struct ShellTool {
    config: SharedToolConfig,
}

impl ShellTool {
    pub fn new(config: SharedToolConfig) -> Self {
        Self { config }
    }
}
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

    async fn execute(&self, args_json: &str, _ctx: &ToolContext) -> Result<String> {
        let snapshot = self.config.snapshot().await;
        execute(
            args_json,
            snapshot.workspace.as_path(),
            snapshot.allowed_dir.as_deref(),
            snapshot.exec.timeout_secs,
            snapshot.exec.restrict_to_workspace,
            &snapshot.exec.path_append,
        )
        .await
    }
}

pub async fn execute(
    args_json: &str,
    default_working_dir: &Path,
    allowed_dir: Option<&Path>,
    timeout_secs: u64,
    restrict_to_workspace: bool,
    path_append: &str,
) -> Result<String> {
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

    guard_command(&command, &cwd, allowed_dir, restrict_to_workspace)?;

    let mut cmd = Command::new("/bin/sh");
    cmd.arg("-lc").arg(&command).current_dir(&cwd);

    if !path_append.trim().is_empty() {
        let old_path = std::env::var("PATH").unwrap_or_default();
        cmd.env("PATH", format!("{}:{}", old_path, path_append));
    }

    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return Err(tool_error!("exec", "executing command: {}", e));
        }
    };

    let output = match tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs),
        child.wait_with_output(),
    )
    .await
    {
        Ok(res) => match res {
            Ok(o) => o,
            Err(e) => {
                return Err(NanobotError::tool_execution(
                    "exec",
                    anyhow::anyhow!("waiting command output: {}", e),
                ));
            }
        },
        Err(_) => {
            return Err(NanobotError::tool_execution(
                "exec",
                anyhow::anyhow!("command timed out after {} seconds", timeout_secs),
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

    const MAX_LEN: usize = 10_000;
    if result.len() > MAX_LEN {
        let remaining = result.len() - MAX_LEN;
        result.truncate(MAX_LEN);
        result.push_str(&format!("\n... (truncated, {} more chars)", remaining));
    }

    Ok(result)
}

fn guard_command(
    command: &str,
    cwd: &Path,
    allowed_dir: Option<&Path>,
    restrict_to_workspace: bool,
) -> Result<()> {
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

    let lower = command.to_lowercase();
    for p in deny_patterns {
        // Pattern-based hard block for obviously destructive commands.
        if Regex::new(p)
            .ok()
            .map(|r| r.is_match(&lower))
            .unwrap_or(false)
        {
            return Err(NanobotError::tool_execution(
                "exec",
                anyhow::anyhow!("command blocked by safety guard (dangerous pattern detected)"),
            ));
        }
    }

    if restrict_to_workspace {
        if command.contains("../") || command.contains("..\\") {
            return Err(NanobotError::tool_execution(
                "exec",
                anyhow::anyhow!("command blocked by safety guard (path traversal detected)"),
            ));
        }
        if command.contains("~/") || command.contains("~\\") {
            return Err(NanobotError::tool_execution(
                "exec",
                anyhow::anyhow!("command blocked by safety guard (home path detected)"),
            ));
        }

        let cwd = cwd.canonicalize().map_err(|e| {
            NanobotError::tool_execution(
                "exec",
                anyhow::anyhow!("canonicalizing cwd {}: {}", cwd.display(), e),
            )
        })?;
        if let Some(allowed_dir) = allowed_dir {
            let allowed_dir = allowed_dir.canonicalize().map_err(|e| {
                NanobotError::tool_execution(
                    "exec",
                    anyhow::anyhow!(
                        "canonicalizing workspace {}: {}",
                        allowed_dir.display(),
                        e
                    ),
                )
            })?;
            if cwd != allowed_dir && !cwd.starts_with(&allowed_dir) {
                return Err(NanobotError::tool_execution(
                    "exec",
                    anyhow::anyhow!("working_dir outside workspace"),
                ));
            }
        }
        // Best-effort scan for absolute paths referenced in the shell string.
        for abs in extract_absolute_paths(command) {
            let p = std::path::PathBuf::from(abs);
            if p.is_absolute() {
                if let Ok(resolved) = p.canonicalize() {
                    let base = if let Some(allowed_dir) = allowed_dir {
                        allowed_dir
                    } else {
                        &cwd
                    };
                    if resolved != base && !resolved.starts_with(base) {
                        return Err(NanobotError::tool_execution(
                            "exec",
                            anyhow::anyhow!(
                                "command blocked by safety guard (path outside working dir)"
                            ),
                        ));
                    }
                }
            }
        }
    }

    Ok(())
}

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

fn resolve_working_dir(
    working_dir: Option<&str>,
    workspace: &Path,
    allowed_dir: Option<&Path>,
) -> Result<std::path::PathBuf> {
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
        NanobotError::tool_execution(
            "exec",
            anyhow::anyhow!("canonicalizing working_dir {}: {}", candidate.display(), e),
        )
    })?;

    if let Some(allowed_dir) = allowed_dir {
        let allowed_dir = allowed_dir.canonicalize().map_err(|e| {
            NanobotError::tool_execution(
                "exec",
                anyhow::anyhow!(
                    "canonicalizing workspace {}: {}",
                    allowed_dir.display(),
                    e
                ),
            )
        })?;
        if resolved != allowed_dir && !resolved.starts_with(&allowed_dir) {
            return Err(NanobotError::tool_execution(
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
        let args: ExecArgs = crate::tools::base::parse_args(json).expect("parse exec args");
        assert_eq!(args.command, "echo ok");
        assert_eq!(args.working_dir.as_deref(), Some("/tmp"));
    }

    #[test]
    fn guard_blocks_path_traversal_when_restricted() {
        let cwd = std::path::PathBuf::from("/tmp");
        let blocked = guard_command("cat ../secret.txt", &cwd, Some(&cwd), true);
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
        let blocked = guard_command("echo hello", &cwd, None, false);
        assert!(blocked.is_ok());
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
}
