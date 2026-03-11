//! Session storage implementations.
//!
//! This module provides concrete implementations of the SessionStore trait:
//! - JsonlSessionStore: File-based storage using JSONL format
//! - InMemorySessionStore: In-memory storage for testing

use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use dashmap::DashMap;

use crate::types::session::{Session, SessionEntry, SessionMetadata, SessionMetadataLine, SessionSummary};
use crate::utils::helpers::{ensure_dir, safe_filename};

use super::traits::SessionStore;

/// JSONL-based session storage with file system persistence and in-memory caching.
///
/// Sessions are stored as JSONL files in `{workspace}/sessions/` directory.
/// Each file contains:
/// 1. First line: metadata (session key, timestamps, etc.)
/// 2. Following lines: session messages
///
/// Uses DashMap for high-performance concurrent caching.
pub struct JsonlSessionStore {
    sessions_dir: PathBuf,
    cache: DashMap<String, Session>,
}

impl JsonlSessionStore {
    /// Creates a new JSONL session store.
    ///
    /// # Arguments
    ///
    /// * `workspace` - Base directory for session storage
    ///
    /// # Errors
    ///
    /// Returns an error if the sessions directory cannot be created.
    pub fn new(workspace: &Path) -> Result<Self> {
        let sessions_dir = ensure_dir(&workspace.join("sessions"))?;
        Ok(Self {
            sessions_dir,
            cache: DashMap::new(),
        })
    }

    fn session_path(&self, key: &str) -> PathBuf {
        let safe = safe_filename(&key.replace(':', "_"));
        self.sessions_dir.join(format!("{}.jsonl", safe))
    }

    fn load_session(&self, key: &str) -> Result<Option<Session>> {
        let path = self.session_path(key);
        if !path.exists() {
            return Ok(None);
        }

        let file = File::open(&path)
            .with_context(|| format!("failed to open {}", path.display()))?;
        let reader = BufReader::new(file);

        let mut messages = Vec::new();
        let mut created_at = Utc::now();
        let mut updated_at = Utc::now();
        let mut metadata = SessionMetadata::default();
        let mut last_consolidated = 0usize;

        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }

            if let Ok(meta) = serde_json::from_str::<SessionMetadataLine>(&line) {
                if meta.line_type == "metadata" {
                    created_at = meta.created_at;
                    updated_at = meta.updated_at;
                    metadata = meta.metadata;
                    last_consolidated = meta.last_consolidated;
                    continue;
                }
            }

            if let Ok(msg) = serde_json::from_str::<SessionEntry>(&line) {
                messages.push(msg);
            }
        }

        Ok(Some(Session {
            key: key.to_string(),
            messages,
            created_at,
            updated_at,
            metadata,
            last_consolidated,
        }))
    }
}

#[async_trait]
impl SessionStore for JsonlSessionStore {
    async fn get_or_create(&self, key: &str) -> Result<Session> {
        if let Some(hit) = self.cache.get(key) {
            return Ok(hit.value().clone());
        }

        let loaded = self.load_session(key)?;
        let session = loaded.unwrap_or_else(|| Session::new(key));
        self.cache.insert(key.to_string(), session.clone());
        Ok(session)
    }

    async fn save(&self, session: &Session) -> Result<()> {
        let path = self.session_path(&session.key);
        let mut file = File::create(&path)
            .with_context(|| format!("failed to create {}", path.display()))?;

        let metadata_line = SessionMetadataLine {
            line_type: "metadata".to_string(),
            key: session.key.clone(),
            created_at: session.created_at,
            updated_at: session.updated_at,
            metadata: session.metadata.clone(),
            last_consolidated: session.last_consolidated,
        };

        writeln!(file, "{}", serde_json::to_string(&metadata_line)?)?;
        for msg in &session.messages {
            writeln!(file, "{}", serde_json::to_string(msg)?)?;
        }

        self.cache.insert(session.key.clone(), session.clone());
        Ok(())
    }

    async fn invalidate(&self, key: &str) {
        self.cache.remove(key);
    }

    async fn list_sessions(&self) -> Result<Vec<SessionSummary>> {
        let mut items = Vec::new();
        for entry in fs::read_dir(&self.sessions_dir)
            .with_context(|| format!("failed to read {}", self.sessions_dir.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
                continue;
            }

            let file = File::open(&path)?;
            let mut reader = BufReader::new(file);
            let mut first_line = String::new();
            if reader.read_line(&mut first_line)? == 0 {
                continue;
            }

            let data = match serde_json::from_str::<SessionMetadataLine>(first_line.trim()) {
                Ok(v) if v.line_type == "metadata" => v,
                _ => continue,
            };

            items.push(SessionSummary {
                key: data.key,
                updated_at: Some(data.updated_at.to_rfc3339()),
                path: path.display().to_string(),
            });
        }

        items.sort_by(|a, b| {
            b.updated_at
                .as_deref()
                .unwrap_or("")
                .cmp(a.updated_at.as_deref().unwrap_or(""))
        });

        Ok(items)
    }

    async fn delete(&self, key: &str) -> Result<()> {
        let path = self.session_path(key);
        if path.exists() {
            tokio::fs::remove_file(path).await?;
        }
        self.invalidate(key).await;
        Ok(())
    }
}

/// In-memory session store for testing and ephemeral sessions.
///
/// All sessions are stored in memory and will be lost when the process exits.
/// Uses DashMap for thread-safe concurrent access.
pub struct InMemorySessionStore {
    sessions: DashMap<String, Session>,
}

impl InMemorySessionStore {
    pub fn new() -> Self {
        Self {
            sessions: DashMap::new(),
        }
    }
}

impl Default for InMemorySessionStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SessionStore for InMemorySessionStore {
    async fn get_or_create(&self, key: &str) -> Result<Session> {
        Ok(self
            .sessions
            .entry(key.to_string())
            .or_insert_with(|| Session::new(key))
            .clone())
    }

    async fn save(&self, session: &Session) -> Result<()> {
        self.sessions.insert(session.key.clone(), session.clone());
        Ok(())
    }

    async fn invalidate(&self, key: &str) {
        self.sessions.remove(key);
    }

    async fn list_sessions(&self) -> Result<Vec<SessionSummary>> {
        Ok(self
            .sessions
            .iter()
            .map(|entry| SessionSummary {
                key: entry.key().clone(),
                updated_at: Some(entry.value().updated_at.to_rfc3339()),
                path: format!("memory://{}", entry.key()),
            })
            .collect())
    }

    async fn delete(&self, key: &str) -> Result<()> {
        self.sessions.remove(key);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use crate::provider::{MessageContent, MessageRole};

    fn temp_workspace(case: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "nanobot-rs-session-{}-{}",
            case,
            uuid::Uuid::new_v4()
        ))
    }

    fn entry(role: MessageRole, text: &str) -> SessionEntry {
        SessionEntry {
            role,
            content: Some(MessageContent::Text(text.to_string())),
            timestamp: Utc::now().to_rfc3339(),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            reasoning_content: None,
            thinking_blocks: None,
        }
    }

    // JsonlSessionStore tests
    #[tokio::test]
    async fn jsonl_save_then_load_roundtrip_works() {
        let workspace = temp_workspace("roundtrip");
        fs::create_dir_all(&workspace).expect("create workspace");
        let store = JsonlSessionStore::new(&workspace).expect("new store");

        let mut session = Session::new("telegram:123");
        session.metadata.tags = vec!["prod".to_string()];
        session.last_consolidated = 1;
        session.messages.push(entry(MessageRole::User, "hello"));
        session.messages.push(entry(MessageRole::Assistant, "world"));

        store.save(&session).await.expect("save session");
        store.invalidate("telegram:123").await;

        let loaded = store.get_or_create("telegram:123").await.expect("load session");
        assert_eq!(loaded.key, "telegram:123");
        assert_eq!(loaded.messages.len(), 2);
        assert_eq!(loaded.metadata.tags, vec!["prod".to_string()]);
        assert_eq!(loaded.last_consolidated, 1);
        assert_eq!(
            loaded.messages[0].content.as_ref().and_then(|c| c.as_text()),
            Some("hello")
        );

        let _ = fs::remove_dir_all(workspace);
    }

    #[tokio::test]
    async fn jsonl_list_sessions_is_sorted_by_updated_at_desc() {
        let workspace = temp_workspace("list");
        fs::create_dir_all(&workspace).expect("create workspace");
        let store = JsonlSessionStore::new(&workspace).expect("new store");

        let mut old = Session::new("cli:old");
        old.updated_at = Utc::now() - Duration::minutes(10);
        store.save(&old).await.expect("save old");

        let mut new = Session::new("cli:new");
        new.updated_at = Utc::now();
        store.save(&new).await.expect("save new");

        let list = store.list_sessions().await.expect("list sessions");
        assert!(list.len() >= 2);
        assert_eq!(list[0].key, "cli:new");
        assert_eq!(list[1].key, "cli:old");

        let _ = fs::remove_dir_all(workspace);
    }

    #[tokio::test]
    async fn jsonl_get_or_create_returns_cached_session() {
        let workspace = temp_workspace("cache");
        fs::create_dir_all(&workspace).expect("create workspace");
        let store = JsonlSessionStore::new(&workspace).expect("new store");

        let session1 = store.get_or_create("test:1").await.expect("first get");
        let session2 = store.get_or_create("test:1").await.expect("second get");

        assert_eq!(session1.key, session2.key);
        assert_eq!(session1.created_at, session2.created_at);

        let _ = fs::remove_dir_all(workspace);
    }

    #[tokio::test]
    async fn jsonl_invalidate_removes_from_cache() {
        let workspace = temp_workspace("invalidate");
        fs::create_dir_all(&workspace).expect("create workspace");
        let store = JsonlSessionStore::new(&workspace).expect("new store");

        let mut session = Session::new("test:invalidate");
        session.messages.push(entry(MessageRole::User, "original"));
        store.save(&session).await.expect("save");

        session.messages.push(entry(MessageRole::Assistant, "reply"));
        store.save(&session).await.expect("save again");

        store.invalidate("test:invalidate").await;

        let loaded = store.get_or_create("test:invalidate").await.expect("reload");
        assert_eq!(loaded.messages.len(), 2);

        let _ = fs::remove_dir_all(workspace);
    }

    #[tokio::test]
    async fn jsonl_session_path_sanitizes_key() {
        let workspace = temp_workspace("sanitize");
        fs::create_dir_all(&workspace).expect("create workspace");
        let store = JsonlSessionStore::new(&workspace).expect("new store");

        let path = store.session_path("telegram:123/456");
        let filename = path.file_name().unwrap().to_str().unwrap();

        assert!(!filename.contains('/'));
        assert!(filename.ends_with(".jsonl"));

        let _ = fs::remove_dir_all(workspace);
    }

    #[tokio::test]
    async fn jsonl_list_sessions_ignores_non_jsonl_files() {
        let workspace = temp_workspace("list-filter");
        fs::create_dir_all(&workspace).expect("create workspace");
        let store = JsonlSessionStore::new(&workspace).expect("new store");

        let session = Session::new("test:valid");
        store.save(&session).await.expect("save");

        let sessions_dir = workspace.join("sessions");
        fs::write(sessions_dir.join("invalid.txt"), "not a session").expect("write txt");

        let list = store.list_sessions().await.expect("list");
        assert_eq!(list.iter().filter(|s| s.key == "test:valid").count(), 1);
        assert_eq!(list.iter().filter(|s| s.key.contains("invalid")).count(), 0);

        let _ = fs::remove_dir_all(workspace);
    }

    #[tokio::test]
    async fn jsonl_delete_removes_file_and_cache() {
        let workspace = temp_workspace("delete");
        fs::create_dir_all(&workspace).expect("create workspace");
        let store = JsonlSessionStore::new(&workspace).expect("new store");

        let session = Session::new("test:delete");
        store.save(&session).await.expect("save");

        let path = store.session_path("test:delete");
        assert!(path.exists());

        store.delete("test:delete").await.expect("delete");
        assert!(!path.exists());

        let _ = fs::remove_dir_all(workspace);
    }

    // InMemorySessionStore tests
    #[tokio::test]
    async fn in_memory_store_get_or_create_works() {
        let store = InMemorySessionStore::new();

        let session1 = store.get_or_create("test:1").await.unwrap();
        assert_eq!(session1.key, "test:1");
        assert!(session1.messages.is_empty());

        let session2 = store.get_or_create("test:1").await.unwrap();
        assert_eq!(session1.key, session2.key);
    }

    #[tokio::test]
    async fn in_memory_store_save_and_retrieve() {
        let store = InMemorySessionStore::new();

        let mut session = Session::new("test:save");
        session.messages.push(entry(MessageRole::User, "hello"));

        store.save(&session).await.unwrap();

        let loaded = store.get_or_create("test:save").await.unwrap();
        assert_eq!(loaded.messages.len(), 1);
    }

    #[tokio::test]
    async fn in_memory_store_delete_works() {
        let store = InMemorySessionStore::new();

        let session = Session::new("test:delete");
        store.save(&session).await.unwrap();

        store.delete("test:delete").await.unwrap();

        let new_session = store.get_or_create("test:delete").await.unwrap();
        assert!(new_session.messages.is_empty());
    }

    #[tokio::test]
    async fn in_memory_store_list_sessions() {
        let store = InMemorySessionStore::new();

        store.save(&Session::new("test:1")).await.unwrap();
        store.save(&Session::new("test:2")).await.unwrap();

        let list = store.list_sessions().await.unwrap();
        assert_eq!(list.len(), 2);
    }
}
