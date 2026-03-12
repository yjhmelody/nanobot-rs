use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use serde_json::json;
use tokio::fs as async_fs;

use crate::error::{NanobotError, Result};
use crate::tool_error;
use crate::tools::base::{
    Tool, ToolContext, ToolDefinition, parse_args, tool_definition_from_json,
};
use crate::tools::config::SharedToolConfig;
use crate::types::tools::{EditFileArgs, ListDirArgs, ReadFileArgs, WriteFileArgs};

// Tool descriptions
const READ_FILE_DESC: &str = "Read the contents of a file at the given path.";
const READ_FILE_PATH_DESC: &str = "The file path to read";

const WRITE_FILE_DESC: &str =
    "Write content to a file at the given path. Creates parent directories if needed.";
const WRITE_FILE_PATH_DESC: &str = "The file path to write to";
const WRITE_FILE_CONTENT_DESC: &str = "The content to write";

const EDIT_FILE_DESC: &str =
    "Edit a file by replacing old_text with new_text. The old_text must exist exactly in the file.";
const EDIT_FILE_PATH_DESC: &str = "The file path to edit";
const EDIT_FILE_OLD_TEXT_DESC: &str = "The exact text to find and replace";
const EDIT_FILE_NEW_TEXT_DESC: &str = "The text to replace with";

const LIST_DIR_DESC: &str = "List files and directories in the given directory path.";
const LIST_DIR_PATH_DESC: &str = "The directory path to list";

pub struct ReadFileTool {
    config: SharedToolConfig,
}

impl ReadFileTool {
    pub fn new(config: SharedToolConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn definition(&self) -> Arc<ToolDefinition> {
        static DEF: OnceLock<Arc<ToolDefinition>> = OnceLock::new();
        DEF.get_or_init(|| {
            Arc::new(tool_definition_from_json(json!({
                "type": "function",
                "function": {
                    "name": "read_file",
                    "description": READ_FILE_DESC,
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "path": {
                                "type": "string",
                                "description": READ_FILE_PATH_DESC
                            }
                        },
                        "required": ["path"]
                    }
                }
            })))
        })
        .clone()
    }

    async fn execute(&self, args_json: &str, _ctx: &ToolContext) -> Result<String> {
        let snapshot = self.config.snapshot().await;
        read_file(
            args_json,
            snapshot.workspace.as_path(),
            snapshot.allowed_dir.as_deref(),
        )
        .await
    }
}

pub struct WriteFileTool {
    config: SharedToolConfig,
}

impl WriteFileTool {
    pub fn new(config: SharedToolConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &str {
        "write_file"
    }

    fn definition(&self) -> Arc<ToolDefinition> {
        static DEF: OnceLock<Arc<ToolDefinition>> = OnceLock::new();
        DEF.get_or_init(|| {
            Arc::new(tool_definition_from_json(json!({
                "type": "function",
                "function": {
                    "name": "write_file",
                    "description": WRITE_FILE_DESC,
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "path": {
                                "type": "string",
                                "description": WRITE_FILE_PATH_DESC
                            },
                            "content": {
                                "type": "string",
                                "description": WRITE_FILE_CONTENT_DESC
                            }
                        },
                        "required": ["path", "content"]
                    }
                }
            })))
        })
        .clone()
    }

    async fn execute(&self, args_json: &str, _ctx: &ToolContext) -> Result<String> {
        let snapshot = self.config.snapshot().await;
        write_file(
            args_json,
            snapshot.workspace.as_path(),
            snapshot.allowed_dir.as_deref(),
        )
        .await
    }
}

pub struct EditFileTool {
    config: SharedToolConfig,
}

impl EditFileTool {
    pub fn new(config: SharedToolConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Tool for EditFileTool {
    fn name(&self) -> &str {
        "edit_file"
    }

    fn definition(&self) -> Arc<ToolDefinition> {
        static DEF: OnceLock<Arc<ToolDefinition>> = OnceLock::new();
        DEF.get_or_init(|| {
            Arc::new(tool_definition_from_json(json!({
                "type": "function",
                "function": {
                    "name": "edit_file",
                    "description": EDIT_FILE_DESC,
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "path": {
                                "type": "string",
                                "description": EDIT_FILE_PATH_DESC
                            },
                            "old_text": {
                                "type": "string",
                                "description": EDIT_FILE_OLD_TEXT_DESC
                            },
                            "new_text": {
                                "type": "string",
                                "description": EDIT_FILE_NEW_TEXT_DESC
                            }
                        },
                        "required": ["path", "old_text", "new_text"]
                    }
                }
            })))
        })
        .clone()
    }

    async fn execute(&self, args_json: &str, _ctx: &ToolContext) -> Result<String> {
        let snapshot = self.config.snapshot().await;
        edit_file(
            args_json,
            snapshot.workspace.as_path(),
            snapshot.allowed_dir.as_deref(),
        )
        .await
    }
}

pub struct ListDirTool {
    config: SharedToolConfig,
}

impl ListDirTool {
    pub fn new(config: SharedToolConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Tool for ListDirTool {
    fn name(&self) -> &str {
        "list_dir"
    }

    fn definition(&self) -> Arc<ToolDefinition> {
        static DEF: OnceLock<Arc<ToolDefinition>> = OnceLock::new();
        DEF.get_or_init(|| {
            Arc::new(tool_definition_from_json(json!({
                "type": "function",
                "function": {
                    "name": "list_dir",
                    "description": LIST_DIR_DESC,
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "path": {
                                "type": "string",
                                "description": LIST_DIR_PATH_DESC
                            }
                        },
                        "required": ["path"]
                    }
                }
            })))
        })
        .clone()
    }

    async fn execute(&self, args_json: &str, _ctx: &ToolContext) -> Result<String> {
        let snapshot = self.config.snapshot().await;
        list_dir(
            args_json,
            snapshot.workspace.as_path(),
            snapshot.allowed_dir.as_deref(),
        )
        .await
    }
}

async fn resolve_path(path: &str, workspace: &Path, allowed_dir: Option<&Path>) -> Result<PathBuf> {
    let raw = if let Some(rest) = path.strip_prefix("~/") {
        dirs::home_dir()
            .map(|h| h.join(rest))
            .unwrap_or_else(|| PathBuf::from(path))
    } else {
        PathBuf::from(path)
    };

    let full = if raw.is_absolute() {
        raw
    } else {
        workspace.join(raw)
    };

    // Canonicalize when possible; if target does not exist yet, keep original full path.
    let resolved = async_fs::canonicalize(&full)
        .await
        .or_else(|_| Ok::<PathBuf, io::Error>(full.clone()))
        .map_err(|e| tool_error!("filesystem", "resolving path {}: {}", full.display(), e))?;

    if let Some(allowed) = allowed_dir {
        let allowed = async_fs::canonicalize(allowed)
            .await
            .or_else(|_| Ok::<PathBuf, io::Error>(allowed.to_path_buf()))
            .map_err(|e| {
                tool_error!(
                    "filesystem",
                    "resolving allowed dir {}: {}",
                    allowed.display(),
                    e
                )
            })?;
        // Enforce workspace boundary for both read and write operations.
        if !resolved.starts_with(&allowed) {
            return Err(tool_error!(
                "filesystem",
                "path {} is outside allowed directory {}",
                path,
                allowed.display()
            ));
        }
    }
    Ok(resolved)
}

async fn read_file(
    args_json: &str,
    workspace: &Path,
    allowed_dir: Option<&Path>,
) -> Result<String> {
    let typed = parse_args::<ReadFileArgs>(args_json)?;
    let path = typed.path;

    let resolved = resolve_path(&path, workspace, allowed_dir).await?;
    let metadata = match async_fs::metadata(&resolved).await {
        Ok(meta) => meta,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return Err(NanobotError::tool_execution(
                "read_file",
                anyhow::anyhow!("file not found: {}", path),
            ));
        }
        Err(err) => {
            return Err(NanobotError::tool_execution(
                "read_file",
                anyhow::anyhow!("reading metadata {}: {}", resolved.display(), err),
            ));
        }
    };
    if !metadata.is_file() {
        return Err(NanobotError::tool_execution(
            "read_file",
            anyhow::anyhow!("not a file: {}", path),
        ));
    }

    async_fs::read_to_string(&resolved).await.map_err(|e| {
        NanobotError::tool_execution(
            "read_file",
            anyhow::anyhow!("reading file {}: {}", resolved.display(), e),
        )
    })
}

async fn write_file(
    args_json: &str,
    workspace: &Path,
    allowed_dir: Option<&Path>,
) -> Result<String> {
    let typed = parse_args::<WriteFileArgs>(args_json)?;
    let path = typed.path;
    let content = typed.content;

    let resolved = resolve_path(&path, workspace, allowed_dir).await?;

    if let Some(parent) = resolved.parent() {
        async_fs::create_dir_all(parent).await.map_err(|e| {
            NanobotError::tool_execution(
                "write_file",
                anyhow::anyhow!("creating directory {}: {}", parent.display(), e),
            )
        })?;
    }

    async_fs::write(&resolved, &content).await.map_err(|e| {
        NanobotError::tool_execution(
            "write_file",
            anyhow::anyhow!("writing file {}: {}", resolved.display(), e),
        )
    })?;
    Ok(format!(
        "Successfully wrote {} bytes to {}",
        content.len(),
        resolved.display()
    ))
}

async fn edit_file(
    args_json: &str,
    workspace: &Path,
    allowed_dir: Option<&Path>,
) -> Result<String> {
    let typed = parse_args::<EditFileArgs>(args_json)?;
    let path = typed.path;
    let old_text = typed.old_text;
    let new_text = typed.new_text;

    let resolved = resolve_path(&path, workspace, allowed_dir).await?;

    let metadata = match async_fs::metadata(&resolved).await {
        Ok(meta) => meta,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return Err(NanobotError::tool_execution(
                "edit_file",
                anyhow::anyhow!("file not found: {}", path),
            ));
        }
        Err(err) => {
            return Err(NanobotError::tool_execution(
                "edit_file",
                anyhow::anyhow!("reading metadata {}: {}", resolved.display(), err),
            ));
        }
    };
    if !metadata.is_file() {
        return Err(NanobotError::tool_execution(
            "edit_file",
            anyhow::anyhow!("not a file: {}", path),
        ));
    }

    let content = async_fs::read_to_string(&resolved).await.map_err(|e| {
        NanobotError::tool_execution(
            "edit_file",
            anyhow::anyhow!("reading file {}: {}", resolved.display(), e),
        )
    })?;

    if !content.contains(&old_text) {
        return Err(NanobotError::tool_execution(
            "edit_file",
            anyhow::anyhow!("old_text not found in {}", path),
        ));
    }
    if content.matches(&old_text).count() > 1 {
        return Ok(format!(
            "Warning: old_text appears multiple times in {}. Please provide more context.",
            path
        ));
    }

    let new_content = content.replacen(&old_text, &new_text, 1);
    async_fs::write(&resolved, new_content).await.map_err(|e| {
        NanobotError::tool_execution(
            "edit_file",
            anyhow::anyhow!("writing file {}: {}", resolved.display(), e),
        )
    })?;
    Ok(format!("Successfully edited {}", resolved.display()))
}

async fn list_dir(args_json: &str, workspace: &Path, allowed_dir: Option<&Path>) -> Result<String> {
    let typed = parse_args::<ListDirArgs>(args_json)?;
    let path = typed.path;

    let resolved = resolve_path(&path, workspace, allowed_dir).await?;
    let metadata = match async_fs::metadata(&resolved).await {
        Ok(meta) => meta,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return Err(NanobotError::tool_execution(
                "list_dir",
                anyhow::anyhow!("directory not found: {}", path),
            ));
        }
        Err(err) => {
            return Err(NanobotError::tool_execution(
                "list_dir",
                anyhow::anyhow!("reading metadata {}: {}", resolved.display(), err),
            ));
        }
    };
    if !metadata.is_dir() {
        return Err(NanobotError::tool_execution(
            "list_dir",
            anyhow::anyhow!("not a directory: {}", path),
        ));
    }

    let mut items = Vec::new();
    let mut read_dir = async_fs::read_dir(&resolved).await.map_err(|e| {
        NanobotError::tool_execution(
            "list_dir",
            anyhow::anyhow!("listing directory {}: {}", resolved.display(), e),
        )
    })?;
    while let Some(ent) = read_dir.next_entry().await.map_err(|e| {
        NanobotError::tool_execution(
            "list_dir",
            anyhow::anyhow!("reading directory entry: {}", e),
        )
    })? {
        let file_type = ent.file_type().await.map_err(|e| {
            NanobotError::tool_execution(
                "list_dir",
                anyhow::anyhow!("reading entry type in {}: {}", resolved.display(), e),
            )
        })?;
        let prefix = if file_type.is_dir() { "📁" } else { "📄" };
        items.push(format!("{} {}", prefix, ent.file_name().to_string_lossy()));
    }

    items.sort();
    if items.is_empty() {
        Ok(format!("Directory {} is empty", path))
    } else {
        Ok(items.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use crate::config::schema::{ExecToolConfig, WebToolsConfig};
    use crate::tools::base::ToolContext;

    fn temp_workspace(case: &str) -> PathBuf {
        std::env::temp_dir().join(format!("nanobot-rs-fs-{}-{}", case, uuid::Uuid::new_v4()))
    }

    fn tool_config(workspace: &Path) -> SharedToolConfig {
        SharedToolConfig::new(
            workspace.to_path_buf(),
            true,
            ExecToolConfig::default(),
            WebToolsConfig::default(),
        )
    }

    #[tokio::test]
    async fn write_then_read_roundtrip_works() {
        let workspace_raw = temp_workspace("roundtrip");
        fs::create_dir_all(&workspace_raw).expect("create temp workspace");
        let workspace = workspace_raw
            .canonicalize()
            .expect("canonicalize temp workspace");
        let config = tool_config(workspace.as_path());
        let write_tool = WriteFileTool::new(config.clone());
        let read_tool = ReadFileTool::new(config);
        let ctx = ToolContext::default();

        let write = write_tool
            .execute(r#"{"path":"notes/todo.txt","content":"hello rust"}"#, &ctx)
            .await
            .expect("write file should succeed");
        assert!(write.contains("Successfully wrote"));

        let read = read_tool
            .execute(r#"{"path":"notes/todo.txt"}"#, &ctx)
            .await
            .expect("read file should succeed");
        assert_eq!(read, "hello rust");

        let _ = fs::remove_dir_all(workspace_raw);
    }

    #[tokio::test]
    async fn edit_file_warns_on_multiple_matches_without_modifying_file() {
        let workspace_raw = temp_workspace("edit-multi");
        fs::create_dir_all(&workspace_raw).expect("create temp workspace");
        let workspace = workspace_raw
            .canonicalize()
            .expect("canonicalize temp workspace");
        let file = workspace.join("dup.txt");
        fs::write(&file, "foo\nfoo\n").expect("seed file");
        let config = tool_config(workspace.as_path());
        let edit_tool = EditFileTool::new(config);
        let ctx = ToolContext::default();

        let out = edit_tool
            .execute(
                r#"{"path":"dup.txt","old_text":"foo","new_text":"bar"}"#,
                &ctx,
            )
            .await
            .expect("edit call should return warning");
        assert!(out.contains("Warning: old_text appears multiple times"));

        let current = fs::read_to_string(&file).expect("read back file");
        assert_eq!(current, "foo\nfoo\n");

        let _ = fs::remove_dir_all(workspace_raw);
    }

    #[tokio::test]
    async fn resolve_path_blocks_access_outside_allowed_directory() {
        let workspace_raw = temp_workspace("allowed");
        fs::create_dir_all(&workspace_raw).expect("create temp workspace");
        let workspace = workspace_raw
            .canonicalize()
            .expect("canonicalize temp workspace");

        let outside = std::env::temp_dir().join(format!(
            "nanobot-rs-fs-outside-{}.txt",
            uuid::Uuid::new_v4()
        ));
        fs::write(&outside, "outside").expect("seed outside file");

        let path_json =
            serde_json::to_string(&outside.to_string_lossy().to_string()).expect("serialize path");
        let args = format!(r#"{{"path":{}}}"#, path_json);
        let config = tool_config(workspace.as_path());
        let read_tool = ReadFileTool::new(config);
        let ctx = ToolContext::default();

        let err = read_tool
            .execute(&args, &ctx)
            .await
            .expect_err("outside path should be rejected");
        assert!(err.to_string().contains("outside allowed directory"));

        let _ = fs::remove_file(outside);
        let _ = fs::remove_dir_all(workspace_raw);
    }
}
