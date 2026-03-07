use std::path::{Path, PathBuf};

use anyhow::Result;
use tokio::fs;

use crate::utils::helpers::ensure_dir;

/// Agent memory storage system.
///
/// # Memory Design Philosophy
///
/// The memory system is designed with a dual-layer approach to balance context relevance
/// and storage efficiency:
///
/// ## 1. Long-term Memory (MEMORY.md)
///
/// - **Purpose**: Stores persistent, high-value information that should be available across
///   all conversations and sessions.
/// - **Content**: Key facts, user preferences, important decisions, learned patterns, and
///   domain knowledge that the agent should always remember.
/// - **Lifecycle**: Manually curated by the agent or user. Information is added when it's
///   deemed important enough to persist indefinitely.
/// - **Usage**: Loaded into every conversation's system prompt, providing consistent context.
/// - **Size**: Should be kept concise (typically < 2000 lines) to avoid context window bloat.
///
/// ## 2. History Log (HISTORY.md)
///
/// - **Purpose**: Append-only log of significant events, decisions, and outcomes over time.
/// - **Content**: Timestamped entries about completed tasks, important conversations,
///   system changes, and notable events.
/// - **Lifecycle**: Continuously appended. Old entries are kept for reference but may not
///   be loaded into active context.
/// - **Usage**: Can be queried when needed for historical context or debugging.
/// - **Size**: Can grow indefinitely as it's not loaded into every conversation.
///
/// ## Memory vs Session History
///
/// - **Session History**: Short-term conversation context (last N messages) that provides
///   immediate conversational continuity. Stored per-session and has a sliding window.
/// - **Long-term Memory**: Cross-session knowledge that should persist and be available
///   regardless of which conversation is active.
///
/// ## Design Rationale
///
/// 1. **Separation of Concerns**: Long-term memory focuses on "what to remember always",
///    while history focuses on "what happened when".
///
/// 2. **Context Window Management**: By keeping long-term memory concise and curated,
///    we ensure it doesn't consume too much of the LLM's context window.
///
/// 3. **File-based Storage**: Simple, human-readable, and easily inspectable. Users can
///    directly edit MEMORY.md to add or correct information.
///
/// ## Future Extensions
///
/// - Vector embeddings for semantic memory search
/// - Automatic memory consolidation and summarization
/// - Memory importance scoring and pruning
/// - Multi-agent shared memory spaces
#[derive(Debug, Clone)]
pub struct MemoryStore {
    memory_dir: PathBuf,
    memory_file: PathBuf,
    history_file: PathBuf,
}

impl MemoryStore {
    /// Creates a new memory store in the specified workspace.
    ///
    /// # Arguments
    ///
    /// * `workspace` - The workspace directory path
    ///
    /// # Returns
    ///
    /// Returns a `MemoryStore` instance with initialized memory directory structure.
    ///
    /// # Errors
    ///
    /// Returns an error if the memory directory cannot be created.
    pub fn new(workspace: &Path) -> Result<Self> {
        let memory_dir = ensure_dir(&workspace.join("memory"))?;
        let memory_file = memory_dir.join("MEMORY.md");
        let history_file = memory_dir.join("HISTORY.md");
        Ok(Self {
            memory_dir,
            memory_file,
            history_file,
        })
    }

    /// Reads the long-term memory content.
    ///
    /// This is the curated, persistent knowledge that should be available in every
    /// conversation. Returns empty string if the file doesn't exist.
    pub async fn read_long_term(&self) -> String {
        fs::read_to_string(&self.memory_file)
            .await
            .unwrap_or_default()
    }

    /// Writes new content to long-term memory, replacing existing content.
    ///
    /// Use this when the agent needs to update its persistent knowledge base.
    /// This should be done thoughtfully as it affects all future conversations.
    ///
    /// # Arguments
    ///
    /// * `content` - The new memory content to write
    pub async fn write_long_term(&self, content: &str) -> Result<()> {
        fs::write(&self.memory_file, content).await?;
        Ok(())
    }

    /// Appends a new entry to the history log.
    ///
    /// History entries are timestamped records of significant events. They are
    /// append-only and not automatically loaded into conversation context.
    ///
    /// # Arguments
    ///
    /// * `entry` - The history entry to append (will be trimmed and formatted)
    pub async fn append_history(&self, entry: &str) -> Result<()> {
        let mut current = fs::read_to_string(&self.history_file)
            .await
            .unwrap_or_default();
        if !current.is_empty() && !current.ends_with("\n\n") {
            current.push_str("\n\n");
        }
        current.push_str(entry.trim_end());
        current.push_str("\n\n");
        fs::write(&self.history_file, current).await?;
        Ok(())
    }

    /// Gets the formatted memory context for inclusion in system prompts.
    ///
    /// Returns the long-term memory wrapped in a markdown section header,
    /// or an empty string if there's no memory content.
    pub async fn get_memory_context(&self) -> String {
        let long_term = self.read_long_term().await;
        if long_term.trim().is_empty() {
            String::new()
        } else {
            format!("## Long-term Memory\n{}", long_term)
        }
    }

    /// Returns the path to the history log file.
    pub fn history_file(&self) -> &Path {
        &self.history_file
    }

    /// Returns the path to the memory directory.
    pub fn memory_dir(&self) -> &Path {
        &self.memory_dir
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_workspace(case: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "nanobot-rs-memory-{}-{}",
            case,
            uuid::Uuid::new_v4()
        ))
    }

    #[tokio::test]
    async fn new_creates_memory_directory() {
        let workspace = temp_workspace("new");
        std::fs::create_dir_all(&workspace).expect("create workspace");

        let store = MemoryStore::new(&workspace).expect("new memory store");

        assert!(store.memory_dir().exists());
        assert!(store.memory_dir().is_dir());

        let _ = std::fs::remove_dir_all(workspace);
    }

    #[tokio::test]
    async fn read_long_term_returns_empty_when_file_missing() {
        let workspace = temp_workspace("read-empty");
        std::fs::create_dir_all(&workspace).expect("create workspace");

        let store = MemoryStore::new(&workspace).expect("new memory store");
        let content = store.read_long_term().await;

        assert_eq!(content, "");

        let _ = std::fs::remove_dir_all(workspace);
    }

    #[tokio::test]
    async fn write_then_read_long_term_roundtrip() {
        let workspace = temp_workspace("roundtrip");
        std::fs::create_dir_all(&workspace).expect("create workspace");

        let store = MemoryStore::new(&workspace).expect("new memory store");

        store
            .write_long_term("# Important Facts\n\n- User prefers Rust\n- Timezone: UTC+8")
            .await
            .expect("write");

        let content = store.read_long_term().await;
        assert!(content.contains("Important Facts"));
        assert!(content.contains("User prefers Rust"));
        assert!(content.contains("Timezone: UTC+8"));

        let _ = std::fs::remove_dir_all(workspace);
    }

    #[tokio::test]
    async fn write_long_term_replaces_existing_content() {
        let workspace = temp_workspace("replace");
        std::fs::create_dir_all(&workspace).expect("create workspace");

        let store = MemoryStore::new(&workspace).expect("new memory store");

        store
            .write_long_term("Old content")
            .await
            .expect("write old");
        store
            .write_long_term("New content")
            .await
            .expect("write new");

        let content = store.read_long_term().await;
        assert_eq!(content, "New content");
        assert!(!content.contains("Old content"));

        let _ = std::fs::remove_dir_all(workspace);
    }

    #[tokio::test]
    async fn append_history_creates_file_if_missing() {
        let workspace = temp_workspace("history-new");
        std::fs::create_dir_all(&workspace).expect("create workspace");

        let store = MemoryStore::new(&workspace).expect("new memory store");

        store
            .append_history("2024-03-01: First entry")
            .await
            .expect("append");

        let content = fs::read_to_string(store.history_file())
            .await
            .expect("read history");
        assert!(content.contains("2024-03-01: First entry"));

        let _ = std::fs::remove_dir_all(workspace);
    }

    #[tokio::test]
    async fn append_history_adds_to_existing_content() {
        let workspace = temp_workspace("history-append");
        std::fs::create_dir_all(&workspace).expect("create workspace");

        let store = MemoryStore::new(&workspace).expect("new memory store");

        store.append_history("Entry 1").await.expect("append 1");
        store.append_history("Entry 2").await.expect("append 2");
        store.append_history("Entry 3").await.expect("append 3");

        let content = fs::read_to_string(store.history_file())
            .await
            .expect("read history");
        assert!(content.contains("Entry 1"));
        assert!(content.contains("Entry 2"));
        assert!(content.contains("Entry 3"));

        // Check order
        let pos1 = content.find("Entry 1").unwrap();
        let pos2 = content.find("Entry 2").unwrap();
        let pos3 = content.find("Entry 3").unwrap();
        assert!(pos1 < pos2);
        assert!(pos2 < pos3);

        let _ = std::fs::remove_dir_all(workspace);
    }

    #[tokio::test]
    async fn append_history_trims_and_formats_entries() {
        let workspace = temp_workspace("history-format");
        std::fs::create_dir_all(&workspace).expect("create workspace");

        let store = MemoryStore::new(&workspace).expect("new memory store");

        store
            .append_history("  Entry with spaces  \n\n")
            .await
            .expect("append");

        let content = fs::read_to_string(store.history_file())
            .await
            .expect("read history");
        assert!(content.contains("Entry with spaces"));
        assert!(!content.contains("  Entry with spaces  "));

        let _ = std::fs::remove_dir_all(workspace);
    }

    #[tokio::test]
    async fn get_memory_context_returns_empty_when_no_memory() {
        let workspace = temp_workspace("context-empty");
        std::fs::create_dir_all(&workspace).expect("create workspace");

        let store = MemoryStore::new(&workspace).expect("new memory store");
        let context = store.get_memory_context().await;

        assert_eq!(context, "");

        let _ = std::fs::remove_dir_all(workspace);
    }

    #[tokio::test]
    async fn get_memory_context_formats_with_header() {
        let workspace = temp_workspace("context-format");
        std::fs::create_dir_all(&workspace).expect("create workspace");

        let store = MemoryStore::new(&workspace).expect("new memory store");
        store
            .write_long_term("Important info")
            .await
            .expect("write");

        let context = store.get_memory_context().await;
        assert!(context.contains("## Long-term Memory"));
        assert!(context.contains("Important info"));

        let _ = std::fs::remove_dir_all(workspace);
    }

    #[tokio::test]
    async fn history_file_returns_correct_path() {
        let workspace = temp_workspace("path");
        std::fs::create_dir_all(&workspace).expect("create workspace");

        let store = MemoryStore::new(&workspace).expect("new memory store");
        let path = store.history_file();

        assert!(path.ends_with("HISTORY.md"));
        assert!(path.starts_with(&workspace));

        let _ = std::fs::remove_dir_all(workspace);
    }

    #[tokio::test]
    async fn memory_dir_returns_correct_path() {
        let workspace = temp_workspace("dir");
        std::fs::create_dir_all(&workspace).expect("create workspace");

        let store = MemoryStore::new(&workspace).expect("new memory store");
        let dir = store.memory_dir();

        assert!(dir.ends_with("memory"));
        assert!(dir.starts_with(&workspace));

        let _ = std::fs::remove_dir_all(workspace);
    }
}
