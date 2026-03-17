# Memory System Design: Enhanced Architecture with OpenViking Integration

## Executive Summary

This document describes the enhanced memory system architecture for nanobot-rs, featuring:
- Improved trait-based design for extensibility
- Plugin architecture for memory backends
- Native OpenViking integration support
- Semantic search and vector storage capabilities
- Backward compatibility with existing file-based memory

## Table of Contents

1. [Current Architecture Analysis](#current-architecture-analysis)
2. [Design Goals](#design-goals)
3. [Enhanced Trait Design](#enhanced-trait-design)
4. [OpenViking Integration](#openviking-integration)
5. [Plugin Architecture](#plugin-architecture)
6. [Implementation Plan](#implementation-plan)
7. [Migration Strategy](#migration-strategy)
8. [Performance Considerations](#performance-considerations)

---

## Current Architecture Analysis

### Existing Components

#### 1. MemoryProvider Trait (Current)

```rust
#[async_trait]
pub trait MemoryProvider: Send + Sync {
    async fn get_context(&self, query: &str, session_key: &str) -> Result<String>;
    async fn store(&self, content: &str, session_key: &str, metadata: Option<&serde_json::Value>) -> Result<()>;
    async fn append_history(&self, entry: &str) -> Result<()>;
}
```

**Limitations:**
- Simple string-based interface lacks semantic capabilities
- No support for vector embeddings or similarity search
- Limited metadata handling
- No batch operations
- No query filtering or ranking

#### 2. MemoryStore (File-based)

**Current Structure:**
```
workspace/
  memory/
    MEMORY.md      # Long-term memory (curated)
    HISTORY.md     # Append-only event log
```

**Limitations:**
- No semantic search
- Linear scan for retrieval
- No relevance ranking
- Limited scalability

#### 3. CompositeMemoryProvider

**Current Design:**
- Combines multiple providers
- Simple concatenation of results
- No deduplication or ranking

---

## Design Goals

### Primary Objectives

1. **Extensibility**: Support multiple memory backends through plugins
2. **Semantic Search**: Enable vector-based similarity search
3. **OpenViking Integration**: Native support for OpenViking as a memory backend
4. **Backward Compatibility**: Maintain existing file-based memory
5. **Performance**: Efficient retrieval and storage operations
6. **Flexibility**: Support various memory types (episodic, semantic, procedural)

### Non-Goals

- Real-time streaming memory updates
- Distributed memory synchronization (future consideration)
- Built-in memory compression (handled by consolidation strategy)

---

## Enhanced Trait Design

### 1. Core Memory Trait

```rust
use serde::{Deserialize, Serialize};
use chrono::{DateTime, Utc};

/// Represents a memory entry with rich metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    /// Unique identifier for this memory
    pub id: String,

    /// The actual content of the memory
    pub content: String,

    /// Optional vector embedding for semantic search
    pub embedding: Option<Vec<f32>>,

    /// Session key this memory belongs to (None for global memories)
    pub session_key: Option<String>,

    /// Memory type classification
    pub memory_type: MemoryType,

    /// Importance score (0.0 to 1.0)
    pub importance: f32,

    /// When this memory was created
    pub created_at: DateTime<Utc>,

    /// When this memory was last accessed
    pub last_accessed_at: DateTime<Utc>,

    /// Number of times this memory has been retrieved
    pub access_count: u32,

    /// Custom metadata for extensibility
    pub metadata: serde_json::Value,

    /// Tags for categorization
    pub tags: Vec<String>,
}

/// Types of memory
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MemoryType {
    /// Episodic: specific events and experiences
    Episodic,

    /// Semantic: facts and knowledge
    Semantic,

    /// Procedural: how-to knowledge and patterns
    Procedural,

    /// Working: temporary context (not persisted long-term)
    Working,
}

/// Query parameters for memory retrieval
#[derive(Debug, Clone)]
pub struct MemoryQuery {
    /// The query text
    pub text: String,

    /// Optional query embedding for semantic search
    pub embedding: Option<Vec<f32>>,

    /// Filter by session key
    pub session_key: Option<String>,

    /// Filter by memory type
    pub memory_type: Option<MemoryType>,

    /// Filter by tags
    pub tags: Option<Vec<String>>,

    /// Minimum importance threshold
    pub min_importance: Option<f32>,

    /// Maximum number of results
    pub limit: usize,

    /// Time range filter
    pub time_range: Option<TimeRange>,
}

#[derive(Debug, Clone)]
pub struct TimeRange {
    pub start: Option<DateTime<Utc>>,
    pub end: Option<DateTime<Utc>>,
}

/// Result of a memory query with relevance scoring
#[derive(Debug, Clone)]
pub struct MemoryResult {
    /// The memory entry
    pub entry: MemoryEntry,

    /// Relevance score (0.0 to 1.0)
    pub score: f32,

    /// Explanation of why this memory was retrieved (for debugging)
    pub reason: Option<String>,
}

/// Enhanced memory provider trait
#[async_trait]
pub trait MemoryProvider: Send + Sync {
    /// Retrieves memories matching the query
    async fn query(&self, query: &MemoryQuery) -> Result<Vec<MemoryResult>>;

    /// Stores a new memory entry
    async fn store(&self, entry: MemoryEntry) -> Result<String>;

    /// Updates an existing memory entry
    async fn update(&self, id: &str, entry: MemoryEntry) -> Result<()>;

    /// Deletes a memory entry
    async fn delete(&self, id: &str) -> Result<()>;

    /// Retrieves a specific memory by ID
    async fn get(&self, id: &str) -> Result<Option<MemoryEntry>>;

    /// Batch store multiple memories (for efficiency)
    async fn store_batch(&self, entries: Vec<MemoryEntry>) -> Result<Vec<String>>;

    /// Returns provider capabilities
    fn capabilities(&self) -> MemoryCapabilities;

    /// Returns provider metadata
    fn metadata(&self) -> MemoryProviderMetadata;
}

/// Capabilities supported by a memory provider
#[derive(Debug, Clone)]
pub struct MemoryCapabilities {
    /// Supports vector embeddings and semantic search
    pub semantic_search: bool,

    /// Supports full-text search
    pub full_text_search: bool,

    /// Supports filtering by metadata
    pub metadata_filtering: bool,

    /// Supports batch operations
    pub batch_operations: bool,

    /// Supports transactions
    pub transactions: bool,

    /// Maximum embedding dimension supported
    pub max_embedding_dim: Option<usize>,
}

/// Metadata about a memory provider
#[derive(Debug, Clone)]
pub struct MemoryProviderMetadata {
    /// Provider name
    pub name: String,

    /// Provider version
    pub version: String,

    /// Provider description
    pub description: String,

    /// Provider author/maintainer
    pub author: Option<String>,
}
```

### 2. Embedding Provider Trait

```rust
/// Trait for generating embeddings from text
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Generates an embedding vector for the given text
    async fn embed(&self, text: &str) -> Result<Vec<f32>>;

    /// Batch embed multiple texts (more efficient)
    async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>>;

    /// Returns the dimension of embeddings produced
    fn dimension(&self) -> usize;

    /// Returns the model name/identifier
    fn model_name(&self) -> &str;
}
```

### 3. Memory Consolidation Trait (Enhanced)

```rust
/// Enhanced consolidation strategy with memory awareness
#[async_trait]
pub trait MemoryConsolidationStrategy: Send + Sync {
    /// Analyzes session and determines what should be stored in long-term memory
    async fn extract_memories(&self, session: &Session) -> Result<Vec<MemoryEntry>>;

    /// Consolidates existing memories (merge, deduplicate, summarize)
    async fn consolidate_memories(&self, memories: Vec<MemoryEntry>) -> Result<Vec<MemoryEntry>>;

    /// Determines if a memory should be pruned based on importance and age
    async fn should_prune(&self, entry: &MemoryEntry) -> bool;
}
```

---

## OpenViking Integration

### What is OpenViking?

OpenViking is a vector database and semantic search engine optimized for:
- High-dimensional vector storage
- Fast similarity search (ANN - Approximate Nearest Neighbor)
- Metadata filtering
- Hybrid search (vector + keyword)

### Integration Architecture

```rust
/// OpenViking-based memory provider
pub struct OpenVikingMemoryProvider {
    /// OpenViking client
    client: OpenVikingClient,

    /// Embedding provider for generating vectors
    embedding_provider: Arc<dyn EmbeddingProvider>,

    /// Collection name in OpenViking
    collection: String,

    /// Configuration
    config: OpenVikingConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenVikingConfig {
    /// OpenViking server URL
    pub url: String,

    /// API key for authentication
    pub api_key: Option<String>,

    /// Collection name
    pub collection: String,

    /// Vector dimension
    pub dimension: usize,

    /// Distance metric (cosine, euclidean, dot_product)
    pub distance_metric: DistanceMetric,

    /// Number of results to retrieve
    pub default_limit: usize,

    /// Minimum similarity threshold
    pub min_similarity: f32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum DistanceMetric {
    Cosine,
    Euclidean,
    DotProduct,
}

impl OpenVikingMemoryProvider {
    pub async fn new(
        config: OpenVikingConfig,
        embedding_provider: Arc<dyn EmbeddingProvider>,
    ) -> Result<Self> {
        let client = OpenVikingClient::new(&config.url, config.api_key.as_deref())?;

        // Ensure collection exists
        client.create_collection_if_not_exists(
            &config.collection,
            config.dimension,
            config.distance_metric,
        ).await?;

        Ok(Self {
            client,
            embedding_provider,
            collection: config.collection.clone(),
            config,
        })
    }
}

#[async_trait]
impl MemoryProvider for OpenVikingMemoryProvider {
    async fn query(&self, query: &MemoryQuery) -> Result<Vec<MemoryResult>> {
        // Generate embedding if not provided
        let embedding = if let Some(emb) = &query.embedding {
            emb.clone()
        } else {
            self.embedding_provider.embed(&query.text).await?
        };

        // Build OpenViking query with filters
        let mut ov_query = self.client
            .query(&self.collection)
            .vector(embedding)
            .limit(query.limit);

        // Apply filters
        if let Some(session_key) = &query.session_key {
            ov_query = ov_query.filter("session_key", session_key);
        }

        if let Some(memory_type) = query.memory_type {
            ov_query = ov_query.filter("memory_type", memory_type.to_string());
        }

        if let Some(min_importance) = query.min_importance {
            ov_query = ov_query.filter_gte("importance", min_importance);
        }

        if let Some(tags) = &query.tags {
            ov_query = ov_query.filter_in("tags", tags);
        }

        // Execute query
        let results = ov_query.execute().await?;

        // Convert to MemoryResult
        let memory_results = results
            .into_iter()
            .map(|r| MemoryResult {
                entry: serde_json::from_value(r.payload)?,
                score: r.score,
                reason: Some(format!("Semantic similarity: {:.3}", r.score)),
            })
            .collect::<Result<Vec<_>>>()?;

        Ok(memory_results)
    }

    async fn store(&self, entry: MemoryEntry) -> Result<String> {
        // Generate embedding if not present
        let embedding = if let Some(emb) = entry.embedding {
            emb
        } else {
            self.embedding_provider.embed(&entry.content).await?
        };

        // Store in OpenViking
        let id = self.client
            .insert(&self.collection)
            .id(&entry.id)
            .vector(embedding)
            .payload(serde_json::to_value(&entry)?)
            .execute()
            .await?;

        Ok(id)
    }

    async fn update(&self, id: &str, entry: MemoryEntry) -> Result<()> {
        // Generate embedding if changed
        let embedding = if let Some(emb) = entry.embedding {
            emb
        } else {
            self.embedding_provider.embed(&entry.content).await?
        };

        self.client
            .update(&self.collection)
            .id(id)
            .vector(embedding)
            .payload(serde_json::to_value(&entry)?)
            .execute()
            .await?;

        Ok(())
    }

    async fn delete(&self, id: &str) -> Result<()> {
        self.client
            .delete(&self.collection)
            .id(id)
            .execute()
            .await?;

        Ok(())
    }

    async fn get(&self, id: &str) -> Result<Option<MemoryEntry>> {
        let result = self.client
            .get(&self.collection)
            .id(id)
            .execute()
            .await?;

        match result {
            Some(point) => {
                let entry: MemoryEntry = serde_json::from_value(point.payload)?;
                Ok(Some(entry))
            }
            None => Ok(None),
        }
    }

    async fn store_batch(&self, entries: Vec<MemoryEntry>) -> Result<Vec<String>> {
        // Generate embeddings for all entries
        let texts: Vec<String> = entries
            .iter()
            .filter(|e| e.embedding.is_none())
            .map(|e| e.content.clone())
            .collect();

        let embeddings = if !texts.is_empty() {
            self.embedding_provider.embed_batch(&texts).await?
        } else {
            Vec::new()
        };

        // Prepare batch insert
        let mut batch = self.client.batch_insert(&self.collection);

        let mut emb_idx = 0;
        for entry in entries {
            let embedding = if let Some(emb) = entry.embedding {
                emb
            } else {
                let emb = embeddings[emb_idx].clone();
                emb_idx += 1;
                emb
            };

            batch = batch.add(
                &entry.id,
                embedding,
                serde_json::to_value(&entry)?,
            );
        }

        let ids = batch.execute().await?;
        Ok(ids)
    }

    fn capabilities(&self) -> MemoryCapabilities {
        MemoryCapabilities {
            semantic_search: true,
            full_text_search: true,
            metadata_filtering: true,
            batch_operations: true,
            transactions: false,
            max_embedding_dim: Some(self.config.dimension),
        }
    }

    fn metadata(&self) -> MemoryProviderMetadata {
        MemoryProviderMetadata {
            name: "OpenViking".to_string(),
            version: "1.0.0".to_string(),
            description: "Vector-based semantic memory using OpenViking".to_string(),
            author: Some("nanobot-rs".to_string()),
        }
    }
}
```

### OpenViking Client Wrapper

```rust
/// Wrapper around OpenViking HTTP API
pub struct OpenVikingClient {
    base_url: String,
    api_key: Option<String>,
    client: reqwest::Client,
}

impl OpenVikingClient {
    pub fn new(base_url: &str, api_key: Option<&str>) -> Result<Self> {
        Ok(Self {
            base_url: base_url.to_string(),
            api_key: api_key.map(String::from),
            client: reqwest::Client::new(),
        })
    }

    pub async fn create_collection_if_not_exists(
        &self,
        name: &str,
        dimension: usize,
        distance: DistanceMetric,
    ) -> Result<()> {
        // Implementation details...
        Ok(())
    }

    pub fn query(&self, collection: &str) -> QueryBuilder {
        QueryBuilder::new(self, collection)
    }

    pub fn insert(&self, collection: &str) -> InsertBuilder {
        InsertBuilder::new(self, collection)
    }

    // ... other methods
}

/// Builder for constructing queries
pub struct QueryBuilder<'a> {
    client: &'a OpenVikingClient,
    collection: String,
    vector: Option<Vec<f32>>,
    limit: usize,
    filters: Vec<Filter>,
}

impl<'a> QueryBuilder<'a> {
    pub fn vector(mut self, vec: Vec<f32>) -> Self {
        self.vector = Some(vec);
        self
    }

    pub fn limit(mut self, limit: usize) -> Self {
        self.limit = limit;
        self
    }

    pub fn filter(mut self, field: &str, value: impl Into<FilterValue>) -> Self {
        self.filters.push(Filter::Eq(field.to_string(), value.into()));
        self
    }

    pub async fn execute(self) -> Result<Vec<SearchResult>> {
        // Execute HTTP request to OpenViking
        // ...
        Ok(Vec::new())
    }
}
```

---

## Plugin Architecture

### Memory Plugin System

```rust
/// Plugin trait for memory providers
pub trait MemoryPlugin: Send + Sync {
    /// Plugin name
    fn name(&self) -> &str;

    /// Plugin version
    fn version(&self) -> &str;

    /// Creates a memory provider instance from configuration
    fn create_provider(
        &self,
        config: &serde_json::Value,
    ) -> Result<Box<dyn MemoryProvider>>;

    /// Returns the configuration schema for this plugin
    fn config_schema(&self) -> serde_json::Value;
}

/// Registry for memory plugins
pub struct MemoryPluginRegistry {
    plugins: HashMap<String, Box<dyn MemoryPlugin>>,
}

impl MemoryPluginRegistry {
    pub fn new() -> Self {
        Self {
            plugins: HashMap::new(),
        }
    }

    pub fn register(&mut self, plugin: Box<dyn MemoryPlugin>) {
        self.plugins.insert(plugin.name().to_string(), plugin);
    }

    pub fn create_provider(
        &self,
        plugin_name: &str,
        config: &serde_json::Value,
    ) -> Result<Box<dyn MemoryProvider>> {
        let plugin = self.plugins
            .get(plugin_name)
            .ok_or_else(|| anyhow::anyhow!("Plugin not found: {}", plugin_name))?;

        plugin.create_provider(config)
    }

    pub fn list_plugins(&self) -> Vec<&str> {
        self.plugins.keys().map(|s| s.as_str()).collect()
    }
}

/// OpenViking plugin implementation
pub struct OpenVikingPlugin {
    embedding_provider: Arc<dyn EmbeddingProvider>,
}

impl MemoryPlugin for OpenVikingPlugin {
    fn name(&self) -> &str {
        "openviking"
    }

    fn version(&self) -> &str {
        "1.0.0"
    }

    fn create_provider(
        &self,
        config: &serde_json::Value,
    ) -> Result<Box<dyn MemoryProvider>> {
        let config: OpenVikingConfig = serde_json::from_value(config.clone())?;
        let provider = OpenVikingMemoryProvider::new(
            config,
            self.embedding_provider.clone(),
        ).await?;
        Ok(Box::new(provider))
    }

    fn config_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "OpenViking server URL"
                },
                "api_key": {
                    "type": "string",
                    "description": "API key for authentication"
                },
                "collection": {
                    "type": "string",
                    "description": "Collection name"
                },
                "dimension": {
                    "type": "integer",
                    "description": "Vector dimension"
                },
                "distance_metric": {
                    "type": "string",
                    "enum": ["cosine", "euclidean", "dot_product"]
                }
            },
            "required": ["url", "collection", "dimension"]
        })
    }
}
```

### Configuration Format

```json
{
  "memory": {
    "providers": [
      {
        "name": "file",
        "plugin": "file",
        "enabled": true,
        "config": {
          "workspace": "~/.nanobot/workspace"
        }
      },
      {
        "name": "openviking",
        "plugin": "openviking",
        "enabled": true,
        "config": {
          "url": "http://localhost:6333",
          "api_key": "${OPENVIKING_API_KEY}",
          "collection": "nanobot_memories",
          "dimension": 1536,
          "distance_metric": "cosine",
          "default_limit": 10,
          "min_similarity": 0.7
        }
      }
    ],
    "embedding": {
      "provider": "openai",
      "model": "text-embedding-3-small",
      "dimension": 1536
    },
    "consolidation": {
      "enabled": true,
      "strategy": "llm_based",
      "min_importance": 0.5
    }
  }
}
```

---

## Implementation Plan

### Phase 1: Core Trait Refactoring (Week 1-2)

1. **Define new traits** (`src/session/memory/traits.rs`)
   - `MemoryProvider` (enhanced)
   - `EmbeddingProvider`
   - `MemoryConsolidationStrategy` (enhanced)

2. **Implement data structures**
   - `MemoryEntry`
   - `MemoryQuery`
   - `MemoryResult`
   - `MemoryCapabilities`

3. **Update existing FileMemoryProvider**
   - Implement new trait interface
   - Maintain backward compatibility
   - Add migration utilities

### Phase 2: Plugin System (Week 3)

1. **Implement plugin registry**
   - `MemoryPlugin` trait
   - `MemoryPluginRegistry`
   - Plugin loading mechanism

2. **Create file-based plugin**
   - Wrap existing `FileMemoryProvider`
   - Add configuration schema

3. **Update SessionManager**
   - Support multiple memory providers
   - Implement provider selection logic

### Phase 3: OpenViking Integration (Week 4-5)

1. **Implement OpenViking client**
   - HTTP API wrapper
   - Query builder
   - Batch operations

2. **Create OpenVikingMemoryProvider**
   - Implement `MemoryProvider` trait
   - Add embedding generation
   - Implement semantic search

3. **Create OpenVikingPlugin**
   - Plugin registration
   - Configuration handling

### Phase 4: Embedding Support (Week 6)

1. **Implement EmbeddingProvider trait**
   - OpenAI embeddings
   - Local model support (optional)

2. **Add embedding generation**
   - Automatic embedding on store
   - Batch embedding optimization

3. **Implement hybrid search**
   - Combine semantic and keyword search
   - Result ranking and deduplication

### Phase 5: Testing & Documentation (Week 7-8)

1. **Unit tests**
   - Test each provider independently
   - Test plugin system

2. **Integration tests**
   - Test with real OpenViking instance
   - Test memory consolidation

3. **Documentation**
   - API documentation
   - Configuration guide
   - Migration guide

---

## Migration Strategy

### Backward Compatibility

1. **Default to file-based memory**
   - Existing installations continue to work
   - No configuration changes required

2. **Gradual migration**
   - Users can enable OpenViking alongside file-based
   - Migrate memories incrementally

3. **Migration tool**
   ```bash
   nanobot migrate-memory --from file --to openviking
   ```

### Migration Process

```rust
pub struct MemoryMigrator {
    source: Box<dyn MemoryProvider>,
    target: Box<dyn MemoryProvider>,
}

impl MemoryMigrator {
    pub async fn migrate(&self) -> Result<MigrationReport> {
        // 1. Query all memories from source
        let query = MemoryQuery {
            text: String::new(),
            limit: usize::MAX,
            ..Default::default()
        };

        let memories = self.source.query(&query).await?;

        // 2. Convert to MemoryEntry format
        let entries: Vec<MemoryEntry> = memories
            .into_iter()
            .map(|r| r.entry)
            .collect();

        // 3. Batch store to target
        let ids = self.target.store_batch(entries).await?;

        Ok(MigrationReport {
            total: ids.len(),
            succeeded: ids.len(),
            failed: 0,
        })
    }
}
```

---

## Performance Considerations

### Caching Strategy

```rust
pub struct CachedMemoryProvider {
    inner: Box<dyn MemoryProvider>,
    cache: Arc<DashMap<String, MemoryEntry>>,
    query_cache: Arc<DashMap<String, Vec<MemoryResult>>>,
    ttl: Duration,
}

impl CachedMemoryProvider {
    pub fn new(inner: Box<dyn MemoryProvider>, ttl: Duration) -> Self {
        Self {
            inner,
            cache: Arc::new(DashMap::new()),
            query_cache: Arc::new(DashMap::new()),
            ttl,
        }
    }
}

#[async_trait]
impl MemoryProvider for CachedMemoryProvider {
    async fn query(&self, query: &MemoryQuery) -> Result<Vec<MemoryResult>> {
        let cache_key = format!("{:?}", query);

        if let Some(cached) = self.query_cache.get(&cache_key) {
            return Ok(cached.clone());
        }

        let results = self.inner.query(query).await?;
        self.query_cache.insert(cache_key, results.clone());

        Ok(results)
    }

    // ... other methods with caching
}
```

### Batch Operations

- Use `store_batch()` for bulk inserts
- Batch embedding generation
- Connection pooling for OpenViking

### Indexing

- Create indexes on frequently queried fields
- Use OpenViking's built-in indexing
- Periodic index optimization

---

## Example Usage

### Basic Usage

```rust
// Create OpenViking provider
let embedding_provider = Arc::new(OpenAIEmbeddingProvider::new(api_key));
let config = OpenVikingConfig {
    url: "http://localhost:6333".to_string(),
    collection: "memories".to_string(),
    dimension: 1536,
    distance_metric: DistanceMetric::Cosine,
    ..Default::default()
};

let provider = OpenVikingMemoryProvider::new(config, embedding_provider).await?;

// Store a memory
let entry = MemoryEntry {
    id: Uuid::new_v4().to_string(),
    content: "User prefers dark mode".to_string(),
    memory_type: MemoryType::Semantic,
    importance: 0.8,
    session_key: Some("user:123".to_string()),
    tags: vec!["preference".to_string()],
    ..Default::default()
};

let id = provider.store(entry).await?;

// Query memories
let query = MemoryQuery {
    text: "What are the user's preferences?".to_string(),
    session_key: Some("user:123".to_string()),
    limit: 5,
    ..Default::default()
};

let results = provider.query(&query).await?;
for result in results {
    println!("Memory: {} (score: {:.3})", result.entry.content, result.score);
}
```

### Plugin Registration

```rust
// Register plugins
let mut registry = MemoryPluginRegistry::new();
registry.register(Box::new(FileMemoryPlugin::new()));
registry.register(Box::new(OpenVikingPlugin::new(embedding_provider)));

// Create provider from config
let config = json!({
    "url": "http://localhost:6333",
    "collection": "memories",
    "dimension": 1536
});

let provider = registry.create_provider("openviking", &config)?;
```

---

## Future Enhancements

1. **Multi-modal memories**: Support images, audio, video
2. **Memory graphs**: Relationships between memories
3. **Federated search**: Query across multiple providers
4. **Memory compression**: Automatic summarization of old memories
5. **Privacy controls**: Encryption, access control
6. **Memory sharing**: Share memories between agents
7. **Temporal reasoning**: Time-aware memory retrieval

---

## Conclusion

This design provides:
- ✅ Extensible trait-based architecture
- ✅ Native OpenViking integration
- ✅ Plugin system for custom backends
- ✅ Semantic search capabilities
- ✅ Backward compatibility
- ✅ Performance optimizations

The implementation can be done incrementally, maintaining backward compatibility while adding powerful new capabilities.
