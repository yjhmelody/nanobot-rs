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
