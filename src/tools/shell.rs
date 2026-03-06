use std::path::Path;
use std::sync::Arc;
use std::sync::OnceLock;

use async_trait::async_trait;
use regex::Regex;
use tokio::process::Command;

use crate::error::{NanobotError, Result};
use crate::tools::base::{JsonSchema, Tool, ToolContext, ToolDefinition, parse_args, schema_props};
use crate::tools::config::SharedToolConfig;
use crate::types::tools::ExecArgs;

pub struct ShellTool {
    config: SharedToolConfig,
}

impl ShellTool {
    pub fn new(config: SharedToolConfig) -> Self {
        Self { config }
    }

    fn definition_static() -> ToolDefinition {
        static DEF: OnceLock<ToolDefinition> = OnceLock::new();
        DEF.get_or_init(|| {
            ToolDefinition::function(
                "exec",
                "Execute a shell command and return its output. Use with caution.",
                JsonSchema::object(
                    schema_props([
                        (
                            "command",
                            JsonSchema::string(Some("The shell command to execute")),
                        ),
                        (
                            "working_dir",
                            JsonSchema::string(Some("Optional working directory for the command")),
                        ),
                    ]),
                    vec!["command"],
                ),
            )
        })
        .clone()
    }
}

pub fn build_tool(config: SharedToolConfig) -> Arc<dyn Tool> {
    Arc::new(ShellTool::new(config))
}

pub fn definition() -> ToolDefinition {
    ShellTool::definition_static()
}

#[async_trait]
impl Tool for ShellTool {
    fn name(&self) -> &str {
        "exec"
    }

    fn definition(&self) -> ToolDefinition {
        Self::definition_static()
    }

    async fn execute(&self, args_json: &str, _ctx: &ToolContext) -> Result<String> {
        let snapshot = self.config.snapshot().await;
        execute(
            args_json,
            snapshot.workspace.as_path(),
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
    timeout_secs: u64,
    restrict_to_workspace: bool,
    path_append: &str,
) -> Result<String> {
    let typed = parse_args::<ExecArgs>(args_json)?;
    let command = typed.command;

    let cwd = typed
        .working_dir
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| default_working_dir.to_path_buf());

    guard_command(&command, &cwd, restrict_to_workspace)?;

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
            return Err(NanobotError::tool_execution(
                "exec",
                anyhow::anyhow!("executing command: {}", e),
            ));
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

fn guard_command(command: &str, cwd: &Path, restrict_to_workspace: bool) -> Result<()> {
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

        let cwd = cwd.canonicalize().map_err(|e| {
            NanobotError::tool_execution(
                "exec",
                anyhow::anyhow!("canonicalizing cwd {}: {}", cwd.display(), e),
            )
        })?;
        // Best-effort scan for absolute paths referenced in the shell string.
        for abs in extract_absolute_paths(command) {
            let p = std::path::PathBuf::from(abs);
            if p.is_absolute() {
                if let Ok(resolved) = p.canonicalize() {
                    if resolved != cwd && !resolved.starts_with(&cwd) {
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
        let blocked = guard_command("cat ../secret.txt", &cwd, true);
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
        let blocked = guard_command("echo hello", &cwd, false);
        assert!(blocked.is_ok());
    }
}
