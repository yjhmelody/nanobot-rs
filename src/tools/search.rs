use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::OnceLock;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::error::{NanobotError, Result};
use crate::tools::base::{JsonSchema, Tool, ToolContext, ToolDefinition, parse_args, schema_props};
use crate::tools::config::SharedToolConfig;

pub fn build_tools(config: SharedToolConfig) -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(SearchFilesTool::new(config.clone())),
        Arc::new(GrepCodeTool::new(config)),
    ]
}

pub fn definitions() -> Vec<ToolDefinition> {
    vec![
        SearchFilesTool::definition_static(),
        GrepCodeTool::definition_static(),
    ]
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchFilesArgs {
    query: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    case_sensitive: bool,
    #[serde(default)]
    regex: bool,
    #[serde(default)]
    file_pattern: Option<String>,
    #[serde(default = "default_limit")]
    limit: usize,
    #[serde(default = "default_context_lines")]
    context_lines: usize,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GrepCodeArgs {
    query: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    case_sensitive: bool,
    #[serde(default = "default_limit")]
    limit: usize,
    #[serde(default = "default_context_lines")]
    context_lines: usize,
}

fn default_limit() -> usize {
    50
}

fn default_context_lines() -> usize {
    2
}

#[derive(Debug, Serialize)]
struct SearchResult {
    file: String,
    line: usize,
    column: usize,
    #[serde(rename = "match")]
    match_text: String,
    context_before: Vec<String>,
    context_after: Vec<String>,
}

#[derive(Debug, Serialize)]
struct SearchResponse {
    results: Vec<SearchResult>,
    total: usize,
    truncated: bool,
}

pub struct SearchFilesTool {
    config: SharedToolConfig,
}

impl SearchFilesTool {
    fn new(config: SharedToolConfig) -> Self {
        Self { config }
    }

    fn definition_static() -> ToolDefinition {
        static DEF: OnceLock<ToolDefinition> = OnceLock::new();
        DEF.get_or_init(|| {
            ToolDefinition::function(
                "search_files",
                "Search for text in files using ripgrep. Fast full-text search across the codebase with regex support.",
                JsonSchema::object(
                    schema_props([
                        ("query", JsonSchema::string(Some("Search query (supports regex if regex=true)"))),
                        ("path", JsonSchema::string(Some("Directory or file to search (optional, defaults to workspace root)"))),
                        ("caseSensitive", JsonSchema::string(Some("Case sensitive search (default: false)"))),
                        ("regex", JsonSchema::string(Some("Treat query as regex (default: false)"))),
                        ("filePattern", JsonSchema::string(Some("File pattern to filter (e.g., '*.rs', '*.{js,ts}')"))),
                        ("limit", JsonSchema::integer(Some("Maximum number of results (default: 50)"))),
                        ("contextLines", JsonSchema::integer(Some("Number of context lines before/after match (default: 2)"))),
                    ]),
                    vec!["query"],
                ),
            )
        })
        .clone()
    }
}

#[async_trait]
impl Tool for SearchFilesTool {
    fn name(&self) -> &str {
        "search_files"
    }

    fn definition(&self) -> ToolDefinition {
        Self::definition_static()
    }

    async fn execute(&self, args_json: &str, _ctx: &ToolContext) -> Result<String> {
        let args: SearchFilesArgs = parse_args(args_json)?;
        let snapshot = self.config.snapshot().await;

        search_with_ripgrep(
            &args.query,
            args.path.as_deref(),
            args.file_pattern.as_deref(),
            None,
            args.case_sensitive,
            args.regex,
            args.limit,
            args.context_lines,
            snapshot.workspace.as_path(),
        )
        .await
    }
}

pub struct GrepCodeTool {
    config: SharedToolConfig,
}

impl GrepCodeTool {
    fn new(config: SharedToolConfig) -> Self {
        Self { config }
    }

    fn definition_static() -> ToolDefinition {
        static DEF: OnceLock<ToolDefinition> = OnceLock::new();
        DEF.get_or_init(|| {
            ToolDefinition::function(
                "grep_code",
                "Search for text in code files with language-specific filtering. Automatically excludes common non-code files.",
                JsonSchema::object(
                    schema_props([
                        ("query", JsonSchema::string(Some("Search query"))),
                        ("path", JsonSchema::string(Some("Directory to search (optional, defaults to workspace root)"))),
                        ("language", JsonSchema::string(Some("Filter by language (e.g., 'rust', 'python', 'javascript')"))),
                        ("caseSensitive", JsonSchema::string(Some("Case sensitive search (default: false)"))),
                        ("limit", JsonSchema::integer(Some("Maximum number of results (default: 50)"))),
                        ("contextLines", JsonSchema::integer(Some("Number of context lines before/after match (default: 2)"))),
                    ]),
                    vec!["query"],
                ),
            )
        })
        .clone()
    }
}

#[async_trait]
impl Tool for GrepCodeTool {
    fn name(&self) -> &str {
        "grep_code"
    }

    fn definition(&self) -> ToolDefinition {
        Self::definition_static()
    }

    async fn execute(&self, args_json: &str, _ctx: &ToolContext) -> Result<String> {
        let args: GrepCodeArgs = parse_args(args_json)?;
        let snapshot = self.config.snapshot().await;

        search_with_ripgrep(
            &args.query,
            args.path.as_deref(),
            None,
            args.language.as_deref(),
            args.case_sensitive,
            false, // grep_code uses literal search by default
            args.limit,
            args.context_lines,
            snapshot.workspace.as_path(),
        )
        .await
    }
}

async fn search_with_ripgrep(
    query: &str,
    path: Option<&str>,
    file_pattern: Option<&str>,
    language: Option<&str>,
    case_sensitive: bool,
    regex: bool,
    limit: usize,
    context_lines: usize,
    workspace: &Path,
) -> Result<String> {
    let search_path = if let Some(p) = path {
        workspace.join(p)
    } else {
        workspace.to_path_buf()
    };

    if !search_path.exists() {
        return Err(NanobotError::invalid_tool_args(
            "search_files",
            format!("Path does not exist: {}", search_path.display()),
        ));
    }

    let mut cmd = Command::new("rg");

    // Basic flags
    cmd.arg("--json")
        .arg("--max-count").arg(limit.to_string())
        .arg("--context").arg(context_lines.to_string());

    // Case sensitivity
    if !case_sensitive {
        cmd.arg("--ignore-case");
    }

    // Regex vs literal
    if !regex {
        cmd.arg("--fixed-strings");
    }

    // File pattern
    if let Some(pattern) = file_pattern {
        cmd.arg("--glob").arg(pattern);
    }

    // Language filter
    if let Some(lang) = language {
        cmd.arg("--type").arg(lang);
    }

    // Query and path
    cmd.arg(query).arg(&search_path);

    // Execute
    cmd.stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd.spawn().map_err(|e| {
        NanobotError::tool_execution(
            "search_files",
            anyhow::anyhow!("Failed to spawn ripgrep: {}. Make sure 'rg' is installed.", e),
        )
    })?;

    let stdout = child.stdout.take().ok_or_else(|| {
        NanobotError::tool_execution("search_files", anyhow::anyhow!("Failed to capture stdout"))
    })?;

    let stderr = child.stderr.take().ok_or_else(|| {
        NanobotError::tool_execution("search_files", anyhow::anyhow!("Failed to capture stderr"))
    })?;

    let mut stdout_data = Vec::new();
    let mut stderr_data = Vec::new();

    let stdout_task = tokio::spawn(async move {
        let mut reader = tokio::io::BufReader::new(stdout);
        reader.read_to_end(&mut stdout_data).await.map(|_| stdout_data)
    });

    let stderr_task = tokio::spawn(async move {
        let mut reader = tokio::io::BufReader::new(stderr);
        reader.read_to_end(&mut stderr_data).await.map(|_| stderr_data)
    });

    let status = child.wait().await.map_err(|e| {
        NanobotError::tool_execution("search_files", anyhow::anyhow!("Failed to wait for ripgrep: {}", e))
    })?;

    let stdout_data = stdout_task.await.map_err(|e| {
        NanobotError::tool_execution("search_files", anyhow::anyhow!("Failed to read stdout: {}", e))
    })??;

    let stderr_data = stderr_task.await.map_err(|e| {
        NanobotError::tool_execution("search_files", anyhow::anyhow!("Failed to read stderr: {}", e))
    })??;

    // ripgrep returns exit code 1 when no matches found, which is not an error
    if !status.success() && status.code() != Some(1) {
        let stderr_text = String::from_utf8_lossy(&stderr_data);
        return Err(NanobotError::tool_execution(
            "search_files",
            anyhow::anyhow!("ripgrep failed: {}", stderr_text),
        ));
    }

    let results = parse_ripgrep_json(&stdout_data, limit)?;
    let response = SearchResponse {
        total: results.len(),
        truncated: results.len() >= limit,
        results,
    };

    serde_json::to_string_pretty(&response).map_err(|e| {
        NanobotError::tool_execution("search_files", anyhow::anyhow!("Failed to serialize results: {}", e))
    })
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum RipgrepMessage {
    #[serde(rename = "match")]
    Match {
        data: RipgrepMatch,
    },
    #[serde(rename = "context")]
    Context {
        data: RipgrepContext,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
struct RipgrepMatch {
    path: RipgrepPath,
    lines: RipgrepLines,
    line_number: usize,
    submatches: Vec<RipgrepSubmatch>,
}

#[derive(Debug, Deserialize)]
struct RipgrepContext {
    path: RipgrepPath,
    lines: RipgrepLines,
    line_number: usize,
}

#[derive(Debug, Deserialize)]
struct RipgrepPath {
    text: String,
}

#[derive(Debug, Deserialize)]
struct RipgrepLines {
    text: String,
}

#[derive(Debug, Deserialize)]
struct RipgrepSubmatch {
    #[serde(rename = "match")]
    match_text: RipgrepMatchText,
    start: usize,
    end: usize,
}

#[derive(Debug, Deserialize)]
struct RipgrepMatchText {
    text: String,
}

fn parse_ripgrep_json(data: &[u8], limit: usize) -> Result<Vec<SearchResult>> {
    let text = String::from_utf8_lossy(data);
    let mut results = Vec::new();
    let mut current_match: Option<(String, usize, usize, String)> = None;
    let mut context_before: Vec<String> = Vec::new();
    let mut context_after: Vec<String> = Vec::new();

    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }

        let msg: RipgrepMessage = match serde_json::from_str(line) {
            Ok(m) => m,
            Err(_) => continue,
        };

        match msg {
            RipgrepMessage::Match { data } => {
                // Save previous match if exists
                if let Some((file, line_num, col, match_text)) = current_match.take() {
                    results.push(SearchResult {
                        file,
                        line: line_num,
                        column: col,
                        match_text,
                        context_before: std::mem::take(&mut context_before),
                        context_after: std::mem::take(&mut context_after),
                    });

                    if results.len() >= limit {
                        break;
                    }

                    // Reset for next match - previous context_after becomes next context_before
                    context_before = context_after.clone();
                    context_after.clear();
                }

                // Start new match
                let column = data.submatches.first().map(|s| s.start).unwrap_or(0);
                let match_text = data.submatches.first()
                    .map(|s| s.match_text.text.clone())
                    .unwrap_or_else(|| data.lines.text.clone());

                current_match = Some((
                    data.path.text,
                    data.line_number,
                    column,
                    match_text,
                ));
            }
            RipgrepMessage::Context { data } => {
                if current_match.is_some() {
                    // After a match, context goes to after
                    context_after.push(data.lines.text);
                } else {
                    // Before any match, context goes to before
                    context_before.push(data.lines.text);
                }
            }
            RipgrepMessage::Other => {}
        }
    }

    // Save last match
    if let Some((file, line_num, col, match_text)) = current_match {
        results.push(SearchResult {
            file,
            line: line_num,
            column: col,
            match_text,
            context_before,
            context_after,
        });
    }

    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::SessionKey;

    #[tokio::test]
    async fn search_files_tool_definition_is_valid() {
        let def = SearchFilesTool::definition_static();
        assert_eq!(def.function.name, "search_files");
        assert!(def.function.parameters.required.contains(&"query".to_string()));
        assert!(!def.function.parameters.required.contains(&"path".to_string()));
        assert!(!def.function.parameters.required.contains(&"limit".to_string()));
    }

    #[tokio::test]
    async fn grep_code_tool_definition_is_valid() {
        let def = GrepCodeTool::definition_static();
        assert_eq!(def.function.name, "grep_code");
        assert!(def.function.parameters.required.contains(&"query".to_string()));
        assert!(!def.function.parameters.required.contains(&"language".to_string()));
    }

    #[test]
    fn parse_empty_ripgrep_output() {
        let results = parse_ripgrep_json(b"", 50).unwrap();
        assert_eq!(results.len(), 0);
    }

    #[test]
    fn parse_ripgrep_json_with_single_match() {
        let json = r#"{"type":"begin","data":{"path":{"text":"test.rs"}}}
{"type":"match","data":{"path":{"text":"test.rs"},"lines":{"text":"fn main() {"},"line_number":1,"submatches":[{"match":{"text":"main"},"start":3,"end":7}]}}
{"type":"end","data":{"path":{"text":"test.rs"}}}
"#;
        let results = parse_ripgrep_json(json.as_bytes(), 50).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].file, "test.rs");
        assert_eq!(results[0].line, 1);
        assert_eq!(results[0].column, 3);
        assert_eq!(results[0].match_text, "main");
    }

    #[test]
    fn parse_ripgrep_json_with_multiple_matches() {
        let json = r#"{"type":"match","data":{"path":{"text":"file1.rs"},"lines":{"text":"test"},"line_number":1,"submatches":[{"match":{"text":"test"},"start":0,"end":4}]}}
{"type":"match","data":{"path":{"text":"file2.rs"},"lines":{"text":"test"},"line_number":5,"submatches":[{"match":{"text":"test"},"start":0,"end":4}]}}
"#;
        let results = parse_ripgrep_json(json.as_bytes(), 50).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].file, "file1.rs");
        assert_eq!(results[1].file, "file2.rs");
    }

    #[test]
    fn parse_ripgrep_json_respects_limit() {
        let json = r#"{"type":"match","data":{"path":{"text":"f1.rs"},"lines":{"text":"x"},"line_number":1,"submatches":[{"match":{"text":"x"},"start":0,"end":1}]}}
{"type":"match","data":{"path":{"text":"f2.rs"},"lines":{"text":"x"},"line_number":1,"submatches":[{"match":{"text":"x"},"start":0,"end":1}]}}
{"type":"match","data":{"path":{"text":"f3.rs"},"lines":{"text":"x"},"line_number":1,"submatches":[{"match":{"text":"x"},"start":0,"end":1}]}}
{"type":"match","data":{"path":{"text":"f4.rs"},"lines":{"text":"x"},"line_number":1,"submatches":[{"match":{"text":"x"},"start":0,"end":1}]}}
"#;
        let results = parse_ripgrep_json(json.as_bytes(), 2).unwrap();
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn parse_ripgrep_json_handles_context_lines() {
        let json = r#"{"type":"context","data":{"path":{"text":"test.rs"},"lines":{"text":"// before"},"line_number":1}}
{"type":"match","data":{"path":{"text":"test.rs"},"lines":{"text":"fn main()"},"line_number":2,"submatches":[{"match":{"text":"main"},"start":3,"end":7}]}}
{"type":"context","data":{"path":{"text":"test.rs"},"lines":{"text":"// after"},"line_number":3}}
"#;
        let results = parse_ripgrep_json(json.as_bytes(), 50).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].context_before.len(), 1);
        assert_eq!(results[0].context_before[0], "// before");
        assert_eq!(results[0].context_after.len(), 1);
        assert_eq!(results[0].context_after[0], "// after");
    }

    #[test]
    fn parse_ripgrep_json_ignores_unknown_message_types() {
        let json = r#"{"type":"unknown","data":{}}
{"type":"match","data":{"path":{"text":"test.rs"},"lines":{"text":"test"},"line_number":1,"submatches":[{"match":{"text":"test"},"start":0,"end":4}]}}
{"type":"summary","data":{}}
"#;
        let results = parse_ripgrep_json(json.as_bytes(), 50).unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn parse_ripgrep_json_handles_malformed_lines() {
        let json = r#"not valid json
{"type":"match","data":{"path":{"text":"test.rs"},"lines":{"text":"test"},"line_number":1,"submatches":[{"match":{"text":"test"},"start":0,"end":4}]}}
{incomplete
"#;
        let results = parse_ripgrep_json(json.as_bytes(), 50).unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn default_limit_is_50() {
        assert_eq!(default_limit(), 50);
    }

    #[test]
    fn default_context_lines_is_2() {
        assert_eq!(default_context_lines(), 2);
    }

    #[test]
    fn search_result_serialization() {
        let result = SearchResult {
            file: "test.rs".to_string(),
            line: 10,
            column: 5,
            match_text: "test".to_string(),
            context_before: vec!["line1".to_string()],
            context_after: vec!["line2".to_string()],
        };

        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("test.rs"));
        assert!(json.contains("\"line\":10"));
        assert!(json.contains("\"column\":5"));
        assert!(json.contains("\"match\":\"test\""));
    }

    #[test]
    fn search_response_serialization() {
        let response = SearchResponse {
            results: vec![],
            total: 0,
            truncated: false,
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"results\":[]"));
        assert!(json.contains("\"total\":0"));
        assert!(json.contains("\"truncated\":false"));
    }

    #[test]
    fn search_response_truncated_flag() {
        let response = SearchResponse {
            results: vec![],
            total: 50,
            truncated: true,
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"truncated\":true"));
    }
}
