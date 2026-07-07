use std::path::{Path, PathBuf};

use crate::SessionResult;
use tokio::fs;

use crate::helpers::ensure_dir;

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
    /// Path to the `memory/` directory under the workspace.
    memory_dir: PathBuf,
    /// Path to `MEMORY.md` -- curated long-term memory.
    memory_file: PathBuf,
    /// Path to `HISTORY.md` -- append-only event log.
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
    pub fn new(workspace: &Path) -> SessionResult<Self> {
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
    pub async fn write_long_term(&self, content: &str) -> SessionResult<()> {
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
    pub async fn append_history(&self, entry: &str) -> SessionResult<()> {
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

    /// Gets query-aware memory context for inclusion in prompts.
    ///
    /// The implementation is intentionally lightweight: split markdown-ish memory
    /// into small blocks, score them by query term overlap, and keep the best
    /// matches. Empty queries preserve the historical behavior of returning the
    /// full long-term memory.
    pub async fn get_memory_context_for_query(&self, query: &str, max_blocks: usize) -> String {
        let long_term = self.read_long_term().await;
        if long_term.trim().is_empty() {
            return String::new();
        }

        let query_terms = tokenize(query);
        if query_terms.is_empty() {
            return format!("## Long-term Memory\n{}", long_term);
        }

        let mut scored = split_memory_blocks(&long_term)
            .into_iter()
            .filter_map(|block| {
                let score = score_block(&block, &query_terms);
                (score > 0).then_some((score, block))
            })
            .collect::<Vec<_>>();

        scored.sort_by(|a, b| b.0.cmp(&a.0));
        let blocks = scored
            .into_iter()
            .take(max_blocks.max(1))
            .map(|(_, block)| block)
            .collect::<Vec<_>>();

        if blocks.is_empty() {
            String::new()
        } else {
            format!("## Relevant Long-term Memory\n{}", blocks.join("\n\n"))
        }
    }

    /// Returns the path to the history log file (`HISTORY.md`).
    pub fn history_file(&self) -> &Path {
        &self.history_file
    }

    /// Returns the path to the memory directory (`{workspace}/memory/`).
    pub fn memory_dir(&self) -> &Path {
        &self.memory_dir
    }
}

/// Splits memory content into blocks separated by markdown headings or blank lines.
///
/// Each heading (`#`, `##`, etc.) starts a new block. Contiguous non-heading
/// lines separated by a blank line also form separate blocks. This allows the
/// query-scoring logic to operate at a fine granularity rather than on the
/// entire memory file.
///
/// # Returns
///
/// A list of non-empty string blocks.
fn split_memory_blocks(content: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut current = Vec::new();

    for line in content.lines() {
        let starts_heading = line.trim_start().starts_with('#');
        if starts_heading && !current.is_empty() {
            blocks.push(current.join("\n").trim().to_string());
            current.clear();
        }

        if line.trim().is_empty() && !current.is_empty() {
            blocks.push(current.join("\n").trim().to_string());
            current.clear();
            continue;
        }

        current.push(line.to_string());
    }

    if !current.is_empty() {
        blocks.push(current.join("\n").trim().to_string());
    }

    blocks.into_iter().filter(|b| !b.is_empty()).collect()
}

/// Tokenizes a query string into lowercase terms for relevance scoring.
///
/// Splits on any non-alphanumeric character (except `_` and `-`), filters
/// out single-character tokens, and converts to lowercase.
///
/// # Returns
///
/// A list of terms (each at least 2 characters long).
fn tokenize(input: &str) -> Vec<String> {
    input
        .split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
        .filter_map(|part| {
            let term = part.trim().to_ascii_lowercase();
            (term.len() >= 2).then_some(term)
        })
        .collect()
}

/// Scores a memory block by counting occurrences of each query term.
///
/// This is a simple bag-of-words overlap score. Each occurrence of a query
/// term in the block (case-insensitive) increments the score by 1.
///
/// # Returns
///
/// The total number of query-term occurrences in the block.
fn score_block(block: &str, query_terms: &[String]) -> usize {
    let lower = block.to_ascii_lowercase();
    query_terms
        .iter()
        .map(|term| lower.matches(term).count())
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_workspace(case: &str) -> PathBuf {
        std::env::temp_dir().join(format!("nanobot-memory-{}-{}", case, uuid::Uuid::new_v4()))
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
        let input = "Important memory content";

        store.write_long_term(input).await.expect("write memory");
        let output = store.read_long_term().await;

        assert_eq!(output, input);

        let _ = std::fs::remove_dir_all(workspace);
    }

    #[tokio::test]
    async fn append_history_adds_entry() {
        let workspace = temp_workspace("history");
        std::fs::create_dir_all(&workspace).expect("create workspace");

        let store = MemoryStore::new(&workspace).expect("new memory store");
        store
            .append_history("Completed: test task")
            .await
            .expect("append history");
        let history = fs::read_to_string(store.history_file())
            .await
            .expect("read history");

        assert!(history.contains("Completed: test task"));

        let _ = std::fs::remove_dir_all(workspace);
    }

    #[tokio::test]
    async fn append_history_adds_spacing_between_entries() {
        let workspace = temp_workspace("history-spacing");
        std::fs::create_dir_all(&workspace).expect("create workspace");

        let store = MemoryStore::new(&workspace).expect("new memory store");
        store
            .append_history("First entry")
            .await
            .expect("append history");
        store
            .append_history("Second entry")
            .await
            .expect("append history");
        let history = fs::read_to_string(store.history_file())
            .await
            .expect("read history");

        assert!(history.contains("First entry"));
        assert!(history.contains("Second entry"));
        assert!(history.contains("First entry\n\nSecond entry"));

        let _ = std::fs::remove_dir_all(workspace);
    }

    #[tokio::test]
    async fn get_memory_context_empty_when_no_content() {
        let workspace = temp_workspace("context-empty");
        std::fs::create_dir_all(&workspace).expect("create workspace");

        let store = MemoryStore::new(&workspace).expect("new memory store");
        let context = store.get_memory_context().await;

        assert!(context.is_empty());

        let _ = std::fs::remove_dir_all(workspace);
    }

    #[tokio::test]
    async fn get_memory_context_includes_header() {
        let workspace = temp_workspace("context-header");
        std::fs::create_dir_all(&workspace).expect("create workspace");

        let store = MemoryStore::new(&workspace).expect("new memory store");
        store
            .write_long_term("Remember me")
            .await
            .expect("write memory");
        let context = store.get_memory_context().await;

        assert!(context.contains("## Long-term Memory"));
        assert!(context.contains("Remember me"));

        let _ = std::fs::remove_dir_all(workspace);
    }

    #[tokio::test]
    async fn get_memory_context_for_query_returns_relevant_blocks() {
        let workspace = temp_workspace("context-query");
        std::fs::create_dir_all(&workspace).expect("create workspace");

        let store = MemoryStore::new(&workspace).expect("new memory store");
        store
            .write_long_term(
                "# Alpha\nThe deploy owner is ReleaseOps.\n\n# Beta\nThe design owner is Research.",
            )
            .await
            .expect("write memory");
        let context = store
            .get_memory_context_for_query("Who owns deploy?", 2)
            .await;

        assert!(context.contains("ReleaseOps"));
        assert!(!context.contains("Research"));

        let _ = std::fs::remove_dir_all(workspace);
    }
}
