use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use chrono::Utc;
use tokio::sync::RwLock;

pub use crate::types::session::{Session, SessionEntry, SessionSummary};
use crate::types::session::{SessionMetadata, SessionMetadataLine};
use crate::utils::helpers::{ensure_dir, safe_filename};

pub struct SessionManager {
    sessions_dir: PathBuf,
    cache: Arc<RwLock<HashMap<String, Session>>>,
}

impl SessionManager {
    /// Creates a new session manager with the specified workspace.
    ///
    /// The workspace directory will be used to store session data in JSON files
    /// under `{workspace}/sessions/`. If the directory doesn't exist, it will be created.
    ///
    /// # Arguments
    ///
    /// * `workspace` - Base directory for session storage
    ///
    /// # Errors
    ///
    /// Returns an error if the sessions directory cannot be created.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use std::path::Path;
    /// use nanobot_rs::session::manager::SessionManager;
    ///
    /// let manager = SessionManager::new(Path::new("/tmp/workspace"))?;
    /// # Ok::<(), anyhow::Error>(())
    /// ```
    pub fn new(workspace: &Path) -> Result<Self> {
        let sessions_dir = ensure_dir(&workspace.join("sessions"))?;
        Ok(Self {
            sessions_dir,
            cache: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    pub async fn get_or_create(&self, key: &str) -> Result<Session> {
        if let Some(hit) = self.cache.read().await.get(key).cloned() {
            return Ok(hit);
        }

        let loaded = self.load_session(key)?;
        let session = loaded.unwrap_or_else(|| Session::new(key));
        self.cache
            .write()
            .await
            .insert(key.to_string(), session.clone());
        Ok(session)
    }

    pub async fn save(&self, session: &Session) -> Result<()> {
        let path = self.session_path(&session.key);
        let mut file =
            File::create(&path).with_context(|| format!("failed to create {}", path.display()))?;

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

        self.cache
            .write()
            .await
            .insert(session.key.clone(), session.clone());
        Ok(())
    }

    pub async fn invalidate(&self, key: &str) {
        self.cache.write().await.remove(key);
    }

    /// Lists all available sessions in the workspace.
    ///
    /// Scans the sessions directory and returns metadata for each session file.
    ///
    /// # Returns
    ///
    /// Returns a vector of session summaries containing:
    /// - Session key
    /// - Last updated timestamp
    /// - File path
    ///
    /// # Errors
    ///
    /// Returns an error if the workspace directory cannot be read or if
    /// session files are corrupted.
    pub fn list_sessions(&self) -> Result<Vec<SessionSummary>> {
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

    fn session_path(&self, key: &str) -> PathBuf {
        let safe = safe_filename(&key.replace(':', "_"));
        self.sessions_dir.join(format!("{}.jsonl", safe))
    }

    fn load_session(&self, key: &str) -> Result<Option<Session>> {
        let path = self.session_path(key);
        if !path.exists() {
            return Ok(None);
        }

        let file =
            File::open(&path).with_context(|| format!("failed to open {}", path.display()))?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use chrono::Utc;

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

    #[test]
    fn get_history_drops_leading_non_user_messages() {
        let mut session = Session::new("cli:direct");
        session.messages = vec![
            entry(MessageRole::Assistant, "preface"),
            entry(MessageRole::User, "question"),
            entry(MessageRole::Assistant, "answer"),
        ];

        let history = session.get_history(10);
        assert_eq!(history.len(), 2);
        assert!(matches!(history[0].role, MessageRole::User));
        assert_eq!(history[0].content_as_text(), Some("question"));
        assert_eq!(history[1].content_as_text(), Some("answer"));
    }

    #[tokio::test]
    async fn save_then_load_roundtrip_works() {
        let workspace = temp_workspace("roundtrip");
        fs::create_dir_all(&workspace).expect("create workspace");
        let manager = SessionManager::new(&workspace).expect("new session manager");

        let mut session = Session::new("telegram:123");
        session.metadata.tags = vec!["prod".to_string()];
        session.last_consolidated = 1;
        session.messages.push(entry(MessageRole::User, "hello"));
        session
            .messages
            .push(entry(MessageRole::Assistant, "world"));

        manager.save(&session).await.expect("save session");
        manager.invalidate("telegram:123").await;

        let loaded = manager
            .get_or_create("telegram:123")
            .await
            .expect("load session");
        assert_eq!(loaded.key, "telegram:123");
        assert_eq!(loaded.messages.len(), 2);
        assert_eq!(loaded.metadata.tags, vec!["prod".to_string()]);
        assert_eq!(loaded.last_consolidated, 1);
        assert_eq!(
            loaded.messages[0]
                .content
                .as_ref()
                .and_then(|c| c.as_text()),
            Some("hello")
        );

        let _ = fs::remove_dir_all(workspace);
    }

    #[tokio::test]
    async fn list_sessions_is_sorted_by_updated_at_desc() {
        let workspace = temp_workspace("list");
        fs::create_dir_all(&workspace).expect("create workspace");
        let manager = SessionManager::new(&workspace).expect("new session manager");

        let mut old = Session::new("cli:old");
        old.updated_at = Utc::now() - Duration::minutes(10);
        manager.save(&old).await.expect("save old");

        let mut new = Session::new("cli:new");
        new.updated_at = Utc::now();
        manager.save(&new).await.expect("save new");

        let list = manager.list_sessions().expect("list sessions");
        assert!(list.len() >= 2);
        assert_eq!(list[0].key, "cli:new");
        assert_eq!(list[1].key, "cli:old");

        let _ = fs::remove_dir_all(workspace);
    }

    #[tokio::test]
    async fn get_or_create_returns_cached_session() {
        let workspace = temp_workspace("cache");
        fs::create_dir_all(&workspace).expect("create workspace");
        let manager = SessionManager::new(&workspace).expect("new session manager");

        let session1 = manager.get_or_create("test:1").await.expect("first get");
        let session2 = manager.get_or_create("test:1").await.expect("second get");

        assert_eq!(session1.key, session2.key);
        assert_eq!(session1.created_at, session2.created_at);

        let _ = fs::remove_dir_all(workspace);
    }

    #[tokio::test]
    async fn invalidate_removes_from_cache() {
        let workspace = temp_workspace("invalidate");
        fs::create_dir_all(&workspace).expect("create workspace");
        let manager = SessionManager::new(&workspace).expect("new session manager");

        let mut session = Session::new("test:invalidate");
        session.messages.push(entry(MessageRole::User, "original"));
        manager.save(&session).await.expect("save");

        session
            .messages
            .push(entry(MessageRole::Assistant, "reply"));
        manager.save(&session).await.expect("save again");

        manager.invalidate("test:invalidate").await;

        let loaded = manager
            .get_or_create("test:invalidate")
            .await
            .expect("reload");
        assert_eq!(loaded.messages.len(), 2);

        let _ = fs::remove_dir_all(workspace);
    }

    #[test]
    fn session_clear_resets_messages_and_consolidation() {
        let mut session = Session::new("test:clear");
        session.messages.push(entry(MessageRole::User, "msg1"));
        session.messages.push(entry(MessageRole::Assistant, "msg2"));
        session.last_consolidated = 2;

        session.clear();

        assert!(session.messages.is_empty());
        assert_eq!(session.last_consolidated, 0);
        assert_eq!(session.key, "test:clear");
    }

    #[test]
    fn get_history_respects_max_messages() {
        let mut session = Session::new("test:max");
        for i in 0..10 {
            session.messages.push(entry(
                if i % 2 == 0 {
                    MessageRole::User
                } else {
                    MessageRole::Assistant
                },
                &format!("msg{}", i),
            ));
        }

        let history = session.get_history(3);
        assert!(history.len() <= 3);
    }

    #[test]
    fn get_history_handles_empty_session() {
        let session = Session::new("test:empty");
        let history = session.get_history(10);
        assert!(history.is_empty());
    }

    #[test]
    fn get_history_respects_last_consolidated() {
        let mut session = Session::new("test:consolidated");
        session.messages.push(entry(MessageRole::User, "old1"));
        session.messages.push(entry(MessageRole::Assistant, "old2"));
        session.last_consolidated = 2;

        session.messages.push(entry(MessageRole::User, "new1"));
        session.messages.push(entry(MessageRole::Assistant, "new2"));

        let history = session.get_history(10);
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].content_as_text(), Some("new1"));
        assert_eq!(history[1].content_as_text(), Some("new2"));
    }

    #[tokio::test]
    async fn session_path_sanitizes_key() {
        let workspace = temp_workspace("sanitize");
        fs::create_dir_all(&workspace).expect("create workspace");
        let manager = SessionManager::new(&workspace).expect("new session manager");

        let path = manager.session_path("telegram:123/456");
        let filename = path.file_name().unwrap().to_str().unwrap();

        assert!(!filename.contains('/'));
        assert!(filename.ends_with(".jsonl"));

        let _ = fs::remove_dir_all(workspace);
    }

    #[tokio::test]
    async fn list_sessions_ignores_non_jsonl_files() {
        let workspace = temp_workspace("list-filter");
        fs::create_dir_all(&workspace).expect("create workspace");
        let manager = SessionManager::new(&workspace).expect("new session manager");

        let session = Session::new("test:valid");
        manager.save(&session).await.expect("save");

        let sessions_dir = workspace.join("sessions");
        fs::write(sessions_dir.join("invalid.txt"), "not a session").expect("write txt");

        let list = manager.list_sessions().expect("list");
        assert_eq!(list.iter().filter(|s| s.key == "test:valid").count(), 1);
        assert_eq!(list.iter().filter(|s| s.key.contains("invalid")).count(), 0);

        let _ = fs::remove_dir_all(workspace);
    }
}
