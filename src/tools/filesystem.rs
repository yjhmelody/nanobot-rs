use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::OnceLock;

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde::Deserialize;
use tokio::fs as async_fs;

use crate::tools::base::{JsonSchema, Tool, ToolContext, ToolDefinition, parse_args, schema_props};
use crate::tools::shared_config::SharedToolConfig;

#[derive(Debug, Deserialize)]
struct ReadFileArgs {
    path: String,
}

#[derive(Debug, Deserialize)]
struct WriteFileArgs {
    path: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct EditFileArgs {
    path: String,
    old_text: String,
    new_text: String,
}

#[derive(Debug, Deserialize)]
struct ListDirArgs {
    path: String,
}

pub fn build_tools(config: SharedToolConfig) -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(ReadFileTool::new(config.clone())),
        Arc::new(WriteFileTool::new(config.clone())),
        Arc::new(EditFileTool::new(config.clone())),
        Arc::new(ListDirTool::new(config)),
    ]
}

pub fn definitions() -> Vec<ToolDefinition> {
    vec![
        ReadFileTool::definition_static(),
        WriteFileTool::definition_static(),
        EditFileTool::definition_static(),
        ListDirTool::definition_static(),
    ]
}

pub async fn execute(
    name: &str,
    args_json: &str,
    workspace: &Path,
    allowed_dir: Option<&Path>,
) -> Result<String> {
    match name {
        "read_file" => read_file(args_json, workspace, allowed_dir).await,
        "write_file" => write_file(args_json, workspace, allowed_dir).await,
        "edit_file" => edit_file(args_json, workspace, allowed_dir).await,
        "list_dir" => list_dir(args_json, workspace, allowed_dir).await,
        _ => bail!("unsupported filesystem tool {}", name),
    }
}

pub struct ReadFileTool {
    config: SharedToolConfig,
}

impl ReadFileTool {
    fn new(config: SharedToolConfig) -> Self {
        Self { config }
    }

    fn definition_static() -> ToolDefinition {
        static DEF: OnceLock<ToolDefinition> = OnceLock::new();
        DEF.get_or_init(|| {
            ToolDefinition::function(
                "read_file",
                "Read the contents of a file at the given path.",
                JsonSchema::object(
                    schema_props([("path", JsonSchema::string(Some("The file path to read")))]),
                    vec!["path"],
                ),
            )
        })
        .clone()
    }
}

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn definition(&self) -> ToolDefinition {
        Self::definition_static()
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
    fn new(config: SharedToolConfig) -> Self {
        Self { config }
    }

    fn definition_static() -> ToolDefinition {
        static DEF: OnceLock<ToolDefinition> = OnceLock::new();
        DEF.get_or_init(|| {
            ToolDefinition::function(
                "write_file",
                "Write content to a file at the given path. Creates parent directories if needed.",
                JsonSchema::object(
                    schema_props([
                        (
                            "path",
                            JsonSchema::string(Some("The file path to write to")),
                        ),
                        ("content", JsonSchema::string(Some("The content to write"))),
                    ]),
                    vec!["path", "content"],
                ),
            )
        })
        .clone()
    }
}

#[async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &str {
        "write_file"
    }

    fn definition(&self) -> ToolDefinition {
        Self::definition_static()
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
    fn new(config: SharedToolConfig) -> Self {
        Self { config }
    }

    fn definition_static() -> ToolDefinition {
        static DEF: OnceLock<ToolDefinition> = OnceLock::new();
        DEF.get_or_init(|| {
            ToolDefinition::function(
                "edit_file",
                "Edit a file by replacing old_text with new_text. The old_text must exist exactly in the file.",
                JsonSchema::object(
                    schema_props([
                        ("path", JsonSchema::string(Some("The file path to edit"))),
                        (
                            "old_text",
                            JsonSchema::string(Some("The exact text to find and replace")),
                        ),
                        (
                            "new_text",
                            JsonSchema::string(Some("The text to replace with")),
                        ),
                    ]),
                    vec!["path", "old_text", "new_text"],
                ),
            )
        })
        .clone()
    }
}

#[async_trait]
impl Tool for EditFileTool {
    fn name(&self) -> &str {
        "edit_file"
    }

    fn definition(&self) -> ToolDefinition {
        Self::definition_static()
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
    fn new(config: SharedToolConfig) -> Self {
        Self { config }
    }

    fn definition_static() -> ToolDefinition {
        static DEF: OnceLock<ToolDefinition> = OnceLock::new();
        DEF.get_or_init(|| {
            ToolDefinition::function(
                "list_dir",
                "List the contents of a directory.",
                JsonSchema::object(
                    schema_props([(
                        "path",
                        JsonSchema::string(Some("The directory path to list")),
                    )]),
                    vec!["path"],
                ),
            )
        })
        .clone()
    }
}

#[async_trait]
impl Tool for ListDirTool {
    fn name(&self) -> &str {
        "list_dir"
    }

    fn definition(&self) -> ToolDefinition {
        Self::definition_static()
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
        .with_context(|| format!("resolving path {}", full.display()))?;

    if let Some(allowed) = allowed_dir {
        let allowed = async_fs::canonicalize(allowed)
            .await
            .or_else(|_| Ok::<PathBuf, io::Error>(allowed.to_path_buf()))
            .with_context(|| format!("resolving allowed dir {}", allowed.display()))?;
        // Enforce workspace boundary for both read and write operations.
        if !resolved.starts_with(&allowed) {
            bail!(
                "path {} is outside allowed directory {}",
                path,
                allowed.display()
            );
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
        Err(err) if err.kind() == io::ErrorKind::NotFound => bail!("file not found: {}", path),
        Err(err) => {
            return Err(err).with_context(|| format!("reading metadata {}", resolved.display()));
        }
    };
    if !metadata.is_file() {
        bail!("not a file: {}", path);
    }

    async_fs::read_to_string(&resolved)
        .await
        .with_context(|| format!("reading file {}", resolved.display()))
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
        async_fs::create_dir_all(parent)
            .await
            .with_context(|| format!("creating directory {}", parent.display()))?;
    }

    async_fs::write(&resolved, &content)
        .await
        .with_context(|| format!("writing file {}", resolved.display()))?;
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
        Err(err) if err.kind() == io::ErrorKind::NotFound => bail!("file not found: {}", path),
        Err(err) => {
            return Err(err).with_context(|| format!("reading metadata {}", resolved.display()));
        }
    };
    if !metadata.is_file() {
        bail!("not a file: {}", path);
    }

    let content = async_fs::read_to_string(&resolved)
        .await
        .with_context(|| format!("reading file {}", resolved.display()))?;

    if !content.contains(&old_text) {
        bail!("old_text not found in {}", path);
    }
    if content.matches(&old_text).count() > 1 {
        return Ok(format!(
            "Warning: old_text appears multiple times in {}. Please provide more context.",
            path
        ));
    }

    let new_content = content.replacen(&old_text, &new_text, 1);
    async_fs::write(&resolved, new_content)
        .await
        .with_context(|| format!("writing file {}", resolved.display()))?;
    Ok(format!("Successfully edited {}", resolved.display()))
}

async fn list_dir(args_json: &str, workspace: &Path, allowed_dir: Option<&Path>) -> Result<String> {
    let typed = parse_args::<ListDirArgs>(args_json)?;
    let path = typed.path;

    let resolved = resolve_path(&path, workspace, allowed_dir).await?;
    let metadata = match async_fs::metadata(&resolved).await {
        Ok(meta) => meta,
        Err(err) if err.kind() == io::ErrorKind::NotFound => bail!("directory not found: {}", path),
        Err(err) => {
            return Err(err).with_context(|| format!("reading metadata {}", resolved.display()));
        }
    };
    if !metadata.is_dir() {
        bail!("not a directory: {}", path);
    }

    let mut items = Vec::new();
    let mut read_dir = async_fs::read_dir(&resolved)
        .await
        .with_context(|| format!("listing directory {}", resolved.display()))?;
    while let Some(ent) = read_dir.next_entry().await? {
        let file_type = ent
            .file_type()
            .await
            .with_context(|| format!("reading entry type in {}", resolved.display()))?;
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

    fn temp_workspace(case: &str) -> PathBuf {
        std::env::temp_dir().join(format!("nanobot-rs-fs-{}-{}", case, uuid::Uuid::new_v4()))
    }

    #[tokio::test]
    async fn write_then_read_roundtrip_works() {
        let workspace_raw = temp_workspace("roundtrip");
        fs::create_dir_all(&workspace_raw).expect("create temp workspace");
        let workspace = workspace_raw
            .canonicalize()
            .expect("canonicalize temp workspace");

        let write = execute(
            "write_file",
            r#"{"path":"notes/todo.txt","content":"hello rust"}"#,
            workspace.as_path(),
            Some(workspace.as_path()),
        )
        .await
        .expect("write file should succeed");
        assert!(write.contains("Successfully wrote"));

        let read = execute(
            "read_file",
            r#"{"path":"notes/todo.txt"}"#,
            workspace.as_path(),
            Some(workspace.as_path()),
        )
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

        let out = execute(
            "edit_file",
            r#"{"path":"dup.txt","old_text":"foo","new_text":"bar"}"#,
            workspace.as_path(),
            Some(workspace.as_path()),
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

        let err = execute(
            "read_file",
            &args,
            workspace.as_path(),
            Some(workspace.as_path()),
        )
        .await
        .expect_err("outside path should be rejected");
        assert!(err.to_string().contains("outside allowed directory"));

        let _ = fs::remove_file(outside);
        let _ = fs::remove_dir_all(workspace_raw);
    }
}
