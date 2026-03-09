use std::path::PathBuf;
use tempfile::TempDir;
use tokio::fs;

use nanobot_rs::tools::base::{Tool, ToolContext};
use nanobot_rs::tools::config::SharedToolConfig;
use nanobot_rs::tools::search::build_tools;
use nanobot_rs::types::SessionKey;

#[tokio::test]
async fn search_files_finds_matches_in_test_directory() {
    let temp = TempDir::new().unwrap();
    let workspace = temp.path().to_path_buf();

    // Create test files
    fs::write(
        workspace.join("test1.rs"),
        "fn main() {\n    println!(\"Hello\");\n}\n",
    )
    .await
    .unwrap();

    fs::write(
        workspace.join("test2.rs"),
        "fn helper() {\n    println!(\"World\");\n}\n",
    )
    .await
    .unwrap();

    let config = SharedToolConfig::new(
        workspace,
        false,
        Default::default(),
        Default::default(),
    );

    let tools = build_tools(config);
    let tool = tools.iter().find(|t| t.name() == "search_files").unwrap();

    let ctx = ToolContext {
        channel: "test".to_string(),
        chat_id: "test".to_string(),
        session_key: SessionKey::from("test:test"),
        message_id: None,
    };

    let result = tool
        .execute(r#"{"query": "println"}"#, &ctx)
        .await
        .unwrap();

    assert!(result.contains("test1.rs") || result.contains("test2.rs"));
    assert!(result.contains("println"));
}

#[tokio::test]
async fn grep_code_filters_by_language() {
    let temp = TempDir::new().unwrap();
    let workspace = temp.path().to_path_buf();

    // Create Rust file
    fs::write(
        workspace.join("code.rs"),
        "fn rust_function() {}\n",
    )
    .await
    .unwrap();

    // Create Python file
    fs::write(
        workspace.join("code.py"),
        "def python_function():\n    pass\n",
    )
    .await
    .unwrap();

    let config = SharedToolConfig::new(
        workspace,
        false,
        Default::default(),
        Default::default(),
    );

    let tools = build_tools(config);
    let tool = tools.iter().find(|t| t.name() == "grep_code").unwrap();

    let ctx = ToolContext {
        channel: "test".to_string(),
        chat_id: "test".to_string(),
        session_key: SessionKey::from("test:test"),
        message_id: None,
    };

    let result = tool
        .execute(r#"{"query": "function", "language": "rust"}"#, &ctx)
        .await
        .unwrap();

    assert!(result.contains("code.rs"));
    assert!(!result.contains("code.py"));
}

#[tokio::test]
async fn search_files_respects_limit() {
    let temp = TempDir::new().unwrap();
    let workspace = temp.path().to_path_buf();

    // Create multiple files with same content
    for i in 0..10 {
        fs::write(
            workspace.join(format!("file{}.txt", i)),
            "target_word\n",
        )
        .await
        .unwrap();
    }

    let config = SharedToolConfig::new(
        workspace,
        false,
        Default::default(),
        Default::default(),
    );

    let tools = build_tools(config);
    let tool = tools.iter().find(|t| t.name() == "search_files").unwrap();

    let ctx = ToolContext {
        channel: "test".to_string(),
        chat_id: "test".to_string(),
        session_key: SessionKey::from("test:test"),
        message_id: None,
    };

    let result = tool
        .execute(r#"{"query": "target_word", "limit": 3}"#, &ctx)
        .await
        .unwrap();

    let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
    let total = parsed["total"].as_u64().unwrap();
    let truncated = parsed["truncated"].as_bool().unwrap();

    assert_eq!(total, 3);
    assert_eq!(truncated, true);
}

#[tokio::test]
async fn search_files_handles_no_matches() {
    let temp = TempDir::new().unwrap();
    let workspace = temp.path().to_path_buf();

    fs::write(workspace.join("test.txt"), "some content\n")
        .await
        .unwrap();

    let config = SharedToolConfig::new(
        workspace,
        false,
        Default::default(),
        Default::default(),
    );

    let tools = build_tools(config);
    let tool = tools.iter().find(|t| t.name() == "search_files").unwrap();

    let ctx = ToolContext {
        channel: "test".to_string(),
        chat_id: "test".to_string(),
        session_key: SessionKey::from("test:test"),
        message_id: None,
    };

    let result = tool
        .execute(r#"{"query": "nonexistent_pattern"}"#, &ctx)
        .await
        .unwrap();

    let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
    let total = parsed["total"].as_u64().unwrap();
    let results = parsed["results"].as_array().unwrap();

    assert_eq!(total, 0);
    assert_eq!(results.len(), 0);
}

#[tokio::test]
async fn search_files_supports_regex() {
    let temp = TempDir::new().unwrap();
    let workspace = temp.path().to_path_buf();

    fs::write(
        workspace.join("test.rs"),
        "fn test_one() {}\nfn test_two() {}\nfn helper() {}\n",
    )
    .await
    .unwrap();

    let config = SharedToolConfig::new(
        workspace,
        false,
        Default::default(),
        Default::default(),
    );

    let tools = build_tools(config);
    let tool = tools.iter().find(|t| t.name() == "search_files").unwrap();

    let ctx = ToolContext {
        channel: "test".to_string(),
        chat_id: "test".to_string(),
        session_key: SessionKey::from("test:test"),
        message_id: None,
    };

    let result = tool
        .execute(r#"{"query": "fn test_\\w+", "regex": true}"#, &ctx)
        .await
        .unwrap();

    let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
    let total = parsed["total"].as_u64().unwrap();

    assert_eq!(total, 2); // Should match test_one and test_two, not helper
}

#[tokio::test]
async fn search_files_case_sensitive() {
    let temp = TempDir::new().unwrap();
    let workspace = temp.path().to_path_buf();

    fs::write(
        workspace.join("test.txt"),
        "Error\nerror\nERROR\n",
    )
    .await
    .unwrap();

    let config = SharedToolConfig::new(
        workspace,
        false,
        Default::default(),
        Default::default(),
    );

    let tools = build_tools(config);
    let tool = tools.iter().find(|t| t.name() == "search_files").unwrap();

    let ctx = ToolContext {
        channel: "test".to_string(),
        chat_id: "test".to_string(),
        session_key: SessionKey::from("test:test"),
        message_id: None,
    };

    // Case insensitive (default)
    let result = tool
        .execute(r#"{"query": "error"}"#, &ctx)
        .await
        .unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(parsed["total"].as_u64().unwrap(), 3);

    // Case sensitive
    let result = tool
        .execute(r#"{"query": "error", "caseSensitive": true}"#, &ctx)
        .await
        .unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(parsed["total"].as_u64().unwrap(), 1);
}

#[tokio::test]
async fn search_files_with_file_pattern() {
    let temp = TempDir::new().unwrap();
    let workspace = temp.path().to_path_buf();

    fs::write(workspace.join("test.rs"), "rust code\n")
        .await
        .unwrap();
    fs::write(workspace.join("test.py"), "python code\n")
        .await
        .unwrap();
    fs::write(workspace.join("test.txt"), "text code\n")
        .await
        .unwrap();

    let config = SharedToolConfig::new(
        workspace,
        false,
        Default::default(),
        Default::default(),
    );

    let tools = build_tools(config);
    let tool = tools.iter().find(|t| t.name() == "search_files").unwrap();

    let ctx = ToolContext {
        channel: "test".to_string(),
        chat_id: "test".to_string(),
        session_key: SessionKey::from("test:test"),
        message_id: None,
    };

    let result = tool
        .execute(r#"{"query": "code", "filePattern": "*.rs"}"#, &ctx)
        .await
        .unwrap();

    assert!(result.contains("test.rs"));
    assert!(!result.contains("test.py"));
    assert!(!result.contains("test.txt"));
}

#[tokio::test]
async fn search_files_with_subdirectory() {
    let temp = TempDir::new().unwrap();
    let workspace = temp.path().to_path_buf();

    let subdir = workspace.join("subdir");
    fs::create_dir(&subdir).await.unwrap();

    fs::write(workspace.join("root.txt"), "root content\n")
        .await
        .unwrap();
    fs::write(subdir.join("sub.txt"), "sub content\n")
        .await
        .unwrap();

    let config = SharedToolConfig::new(
        workspace,
        false,
        Default::default(),
        Default::default(),
    );

    let tools = build_tools(config);
    let tool = tools.iter().find(|t| t.name() == "search_files").unwrap();

    let ctx = ToolContext {
        channel: "test".to_string(),
        chat_id: "test".to_string(),
        session_key: SessionKey::from("test:test"),
        message_id: None,
    };

    let result = tool
        .execute(r#"{"query": "content", "path": "subdir"}"#, &ctx)
        .await
        .unwrap();

    assert!(result.contains("sub.txt"));
    assert!(!result.contains("root.txt"));
}

#[tokio::test]
async fn search_files_returns_error_for_nonexistent_path() {
    let temp = TempDir::new().unwrap();
    let workspace = temp.path().to_path_buf();

    let config = SharedToolConfig::new(
        workspace,
        false,
        Default::default(),
        Default::default(),
    );

    let tools = build_tools(config);
    let tool = tools.iter().find(|t| t.name() == "search_files").unwrap();

    let ctx = ToolContext {
        channel: "test".to_string(),
        chat_id: "test".to_string(),
        session_key: SessionKey::from("test:test"),
        message_id: None,
    };

    let result = tool
        .execute(r#"{"query": "test", "path": "nonexistent"}"#, &ctx)
        .await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.to_string().contains("does not exist"));
}

#[tokio::test]
async fn search_files_with_context_lines() {
    let temp = TempDir::new().unwrap();
    let workspace = temp.path().to_path_buf();

    fs::write(
        workspace.join("test.txt"),
        "line1\nline2\ntarget\nline4\nline5\n",
    )
    .await
    .unwrap();

    let config = SharedToolConfig::new(
        workspace,
        false,
        Default::default(),
        Default::default(),
    );

    let tools = build_tools(config);
    let tool = tools.iter().find(|t| t.name() == "search_files").unwrap();

    let ctx = ToolContext {
        channel: "test".to_string(),
        chat_id: "test".to_string(),
        session_key: SessionKey::from("test:test"),
        message_id: None,
    };

    let result = tool
        .execute(r#"{"query": "target", "contextLines": 1}"#, &ctx)
        .await
        .unwrap();

    let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
    let results = parsed["results"].as_array().unwrap();

    assert_eq!(results.len(), 1);
    let first = &results[0];

    // Should have context before and after
    assert!(first["context_before"].as_array().is_some());
    assert!(first["context_after"].as_array().is_some());
}

#[tokio::test]
async fn grep_code_uses_literal_search_by_default() {
    let temp = TempDir::new().unwrap();
    let workspace = temp.path().to_path_buf();

    fs::write(
        workspace.join("test.rs"),
        "let x = 5;\nlet y = x + 1;\n",
    )
    .await
    .unwrap();

    let config = SharedToolConfig::new(
        workspace,
        false,
        Default::default(),
        Default::default(),
    );

    let tools = build_tools(config);
    let tool = tools.iter().find(|t| t.name() == "grep_code").unwrap();

    let ctx = ToolContext {
        channel: "test".to_string(),
        chat_id: "test".to_string(),
        session_key: SessionKey::from("test:test"),
        message_id: None,
    };

    // Search for literal "x + 1" (not regex)
    let result = tool
        .execute(r#"{"query": "x + 1"}"#, &ctx)
        .await
        .unwrap();

    let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(parsed["total"].as_u64().unwrap(), 1);
}

#[tokio::test]
async fn search_files_handles_empty_workspace() {
    let temp = TempDir::new().unwrap();
    let workspace = temp.path().to_path_buf();

    let config = SharedToolConfig::new(
        workspace,
        false,
        Default::default(),
        Default::default(),
    );

    let tools = build_tools(config);
    let tool = tools.iter().find(|t| t.name() == "search_files").unwrap();

    let ctx = ToolContext {
        channel: "test".to_string(),
        chat_id: "test".to_string(),
        session_key: SessionKey::from("test:test"),
        message_id: None,
    };

    let result = tool
        .execute(r#"{"query": "anything"}"#, &ctx)
        .await
        .unwrap();

    let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(parsed["total"].as_u64().unwrap(), 0);
}

