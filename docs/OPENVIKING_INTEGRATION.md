# OpenViking Integration Guide

## Overview

This guide provides detailed information on integrating OpenViking as a memory backend for nanobot-rs, including API specifications, implementation examples, and best practices.

## OpenViking API Reference

### Base Concepts

OpenViking is a vector database that stores points (vectors) with associated payloads (metadata). Each point has:
- **ID**: Unique identifier
- **Vector**: High-dimensional embedding
- **Payload**: JSON metadata

### HTTP API Endpoints

#### 1. Collection Management

**Create Collection**
```http
PUT /collections/{collection_name}
Content-Type: application/json

{
  "vectors": {
    "size": 1536,
    "distance": "Cosine"
  }
}
```

**List Collections**
```http
GET /collections
```

**Delete Collection**
```http
DELETE /collections/{collection_name}
```

#### 2. Point Operations

**Insert Point**
```http
PUT /collections/{collection_name}/points
Content-Type: application/json

{
  "points": [
    {
      "id": "uuid-or-string",
      "vector": [0.1, 0.2, ...],
      "payload": {
        "content": "Memory content",
        "session_key": "user:123",
        "importance": 0.8
      }
    }
  ]
}
```

**Search Points**
```http
POST /collections/{collection_name}/points/search
Content-Type: application/json

{
  "vector": [0.1, 0.2, ...],
  "limit": 10,
  "with_payload": true,
  "with_vector": false,
  "filter": {
    "must": [
      {
        "key": "session_key",
        "match": {
          "value": "user:123"
        }
      }
    ]
  }
}
```

**Get Point**
```http
GET /collections/{collection_name}/points/{point_id}
```

**Update Point**
```http
PUT /collections/{collection_name}/points/{point_id}
Content-Type: application/json

{
  "vector": [0.1, 0.2, ...],
  "payload": {...}
}
```

**Delete Point**
```http
DELETE /collections/{collection_name}/points/{point_id}
```

#### 3. Batch Operations

**Batch Insert**
```http
PUT /collections/{collection_name}/points/batch
Content-Type: application/json

{
  "points": [
    {"id": "1", "vector": [...], "payload": {...}},
    {"id": "2", "vector": [...], "payload": {...}}
  ]
}
```

### Filter Syntax

OpenViking supports complex filtering:

```json
{
  "filter": {
    "must": [
      {"key": "memory_type", "match": {"value": "semantic"}},
      {"key": "importance", "range": {"gte": 0.5}}
    ],
    "should": [
      {"key": "tags", "match": {"any": ["important", "user-preference"]}}
    ],
    "must_not": [
      {"key": "session_key", "match": {"value": "archived"}}
    ]
  }
}
```

## Implementation Details

### OpenViking Client Implementation

```rust
use reqwest::{Client, Response};
use serde::{Deserialize, Serialize};
use anyhow::{Result, Context};

/// OpenViking HTTP client
pub struct OpenVikingClient {
    base_url: String,
    api_key: Option<String>,
    client: Client,
}

impl OpenVikingClient {
    pub fn new(base_url: &str, api_key: Option<&str>) -> Result<Self> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()?;

        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: api_key.map(String::from),
            client,
        })
    }

    /// Creates a collection if it doesn't exist
    pub async fn create_collection_if_not_exists(
        &self,
        name: &str,
        dimension: usize,
        distance: DistanceMetric,
    ) -> Result<()> {
        // Check if collection exists
        let exists = self.collection_exists(name).await?;
        if exists {
            return Ok(());
        }

        // Create collection
        let url = format!("{}/collections/{}", self.base_url, name);
        let body = json!({
            "vectors": {
                "size": dimension,
                "distance": distance.to_string()
            }
        });

        let mut req = self.client.put(&url).json(&body);
        if let Some(key) = &self.api_key {
            req = req.header("api-key", key);
        }

        let response = req.send().await?;
        if !response.status().is_success() {
            let error_text = response.text().await?;
            anyhow::bail!("Failed to create collection: {}", error_text);
        }

        Ok(())
    }

    /// Checks if a collection exists
    async fn collection_exists(&self, name: &str) -> Result<bool> {
        let url = format!("{}/collections/{}", self.base_url, name);
        let mut req = self.client.get(&url);
        if let Some(key) = &self.api_key {
            req = req.header("api-key", key);
        }

        let response = req.send().await?;
        Ok(response.status().is_success())
    }

    /// Inserts a single point
    pub async fn insert_point(
        &self,
        collection: &str,
        id: &str,
        vector: Vec<f32>,
        payload: serde_json::Value,
    ) -> Result<()> {
        let url = format!("{}/collections/{}/points", self.base_url, collection);
        let body = json!({
            "points": [{
                "id": id,
                "vector": vector,
                "payload": payload
            }]
        });

        let mut req = self.client.put(&url).json(&body);
        if let Some(key) = &self.api_key {
            req = req.header("api-key", key);
        }

        let response = req.send().await?;
        if !response.status().is_success() {
            let error_text = response.text().await?;
            anyhow::bail!("Failed to insert point: {}", error_text);
        }

        Ok(())
    }

    /// Batch inserts multiple points
    pub async fn insert_points_batch(
        &self,
        collection: &str,
        points: Vec<PointInsert>,
    ) -> Result<()> {
        let url = format!("{}/collections/{}/points/batch", self.base_url, collection);
        let body = json!({
            "points": points
        });

        let mut req = self.client.put(&url).json(&body);
        if let Some(key) = &self.api_key {
            req = req.header("api-key", key);
        }

        let response = req.send().await?;
        if !response.status().is_success() {
            let error_text = response.text().await?;
            anyhow::bail!("Failed to batch insert points: {}", error_text);
        }

        Ok(())
    }

    /// Searches for similar vectors
    pub async fn search(
        &self,
        collection: &str,
        vector: Vec<f32>,
        limit: usize,
        filter: Option<Filter>,
    ) -> Result<Vec<SearchResult>> {
        let url = format!("{}/collections/{}/points/search", self.base_url, collection);
        let mut body = json!({
            "vector": vector,
            "limit": limit,
            "with_payload": true,
            "with_vector": false
        });

        if let Some(f) = filter {
            body["filter"] = serde_json::to_value(f)?;
        }

        let mut req = self.client.post(&url).json(&body);
        if let Some(key) = &self.api_key {
            req = req.header("api-key", key);
        }

        let response = req.send().await?;
        if !response.status().is_success() {
            let error_text = response.text().await?;
            anyhow::bail!("Failed to search: {}", error_text);
        }

        let result: SearchResponse = response.json().await?;
        Ok(result.result)
    }

    /// Gets a point by ID
    pub async fn get_point(
        &self,
        collection: &str,
        id: &str,
    ) -> Result<Option<Point>> {
        let url = format!("{}/collections/{}/points/{}", self.base_url, collection, id);
        let mut req = self.client.get(&url);
        if let Some(key) = &self.api_key {
            req = req.header("api-key", key);
        }

        let response = req.send().await?;
        if response.status().as_u16() == 404 {
            return Ok(None);
        }

        if !response.status().is_success() {
            let error_text = response.text().await?;
            anyhow::bail!("Failed to get point: {}", error_text);
        }

        let result: GetPointResponse = response.json().await?;
        Ok(Some(result.result))
    }

    /// Updates a point
    pub async fn update_point(
        &self,
        collection: &str,
        id: &str,
        vector: Vec<f32>,
        payload: serde_json::Value,
    ) -> Result<()> {
        let url = format!("{}/collections/{}/points/{}", self.base_url, collection, id);
        let body = json!({
            "vector": vector,
            "payload": payload
        });

        let mut req = self.client.put(&url).json(&body);
        if let Some(key) = &self.api_key {
            req = req.header("api-key", key);
        }

        let response = req.send().await?;
        if !response.status().is_success() {
            let error_text = response.text().await?;
            anyhow::bail!("Failed to update point: {}", error_text);
        }

        Ok(())
    }

    /// Deletes a point
    pub async fn delete_point(
        &self,
        collection: &str,
        id: &str,
    ) -> Result<()> {
        let url = format!("{}/collections/{}/points/{}", self.base_url, collection, id);
        let mut req = self.client.delete(&url);
        if let Some(key) = &self.api_key {
            req = req.header("api-key", key);
        }

        let response = req.send().await?;
        if !response.status().is_success() {
            let error_text = response.text().await?;
            anyhow::bail!("Failed to delete point: {}", error_text);
        }

        Ok(())
    }
}

/// Point data for insertion
#[derive(Debug, Clone, Serialize)]
pub struct PointInsert {
    pub id: String,
    pub vector: Vec<f32>,
    pub payload: serde_json::Value,
}

/// Search result from OpenViking
#[derive(Debug, Clone, Deserialize)]
pub struct SearchResult {
    pub id: String,
    pub score: f32,
    pub payload: serde_json::Value,
    #[serde(default)]
    pub vector: Option<Vec<f32>>,
}

/// Response from search endpoint
#[derive(Debug, Deserialize)]
struct SearchResponse {
    result: Vec<SearchResult>,
}

/// Point data from get endpoint
#[derive(Debug, Clone, Deserialize)]
pub struct Point {
    pub id: String,
    pub vector: Vec<f32>,
    pub payload: serde_json::Value,
}

/// Response from get point endpoint
#[derive(Debug, Deserialize)]
struct GetPointResponse {
    result: Point,
}

/// Filter for search queries
#[derive(Debug, Clone, Serialize)]
pub struct Filter {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub must: Vec<Condition>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub should: Vec<Condition>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub must_not: Vec<Condition>,
}

impl Filter {
    pub fn new() -> Self {
        Self {
            must: Vec::new(),
            should: Vec::new(),
            must_not: Vec::new(),
        }
    }

    pub fn must(mut self, condition: Condition) -> Self {
        self.must.push(condition);
        self
    }

    pub fn should(mut self, condition: Condition) -> Self {
        self.should.push(condition);
        self
    }

    pub fn must_not(mut self, condition: Condition) -> Self {
        self.must_not.push(condition);
        self
    }
}

/// Filter condition
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum Condition {
    Match {
        key: String,
        #[serde(rename = "match")]
        match_value: MatchValue,
    },
    Range {
        key: String,
        range: RangeValue,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum MatchValue {
    Value { value: serde_json::Value },
    Any { any: Vec<serde_json::Value> },
}

#[derive(Debug, Clone, Serialize)]
pub struct RangeValue {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gt: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gte: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lt: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lte: Option<f64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum DistanceMetric {
    Cosine,
    Euclidean,
    Dot,
}

impl std::fmt::Display for DistanceMetric {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DistanceMetric::Cosine => write!(f, "Cosine"),
            DistanceMetric::Euclidean => write!(f, "Euclid"),
            DistanceMetric::Dot => write!(f, "Dot"),
        }
    }
}
```

### Embedding Provider Implementation

```rust
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};

/// OpenAI embedding provider
pub struct OpenAIEmbeddingProvider {
    api_key: String,
    model: String,
    dimension: usize,
    client: Client,
}

impl OpenAIEmbeddingProvider {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            model: "text-embedding-3-small".to_string(),
            dimension: 1536,
            client: Client::new(),
        }
    }

    pub fn with_model(mut self, model: String, dimension: usize) -> Self {
        self.model = model;
        self.dimension = dimension;
        self
    }
}

#[async_trait]
impl EmbeddingProvider for OpenAIEmbeddingProvider {
    async fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let response = self.client
            .post("https://api.openai.com/v1/embeddings")
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&json!({
                "input": text,
                "model": self.model
            }))
            .send()
            .await?;

        if !response.status().is_success() {
            let error_text = response.text().await?;
            anyhow::bail!("OpenAI API error: {}", error_text);
        }

        let result: EmbeddingResponse = response.json().await?;
        Ok(result.data[0].embedding.clone())
    }

    async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let response = self.client
            .post("https://api.openai.com/v1/embeddings")
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&json!({
                "input": texts,
                "model": self.model
            }))
            .send()
            .await?;

        if !response.status().is_success() {
            let error_text = response.text().await?;
            anyhow::bail!("OpenAI API error: {}", error_text);
        }

        let result: EmbeddingResponse = response.json().await?;
        Ok(result.data.into_iter().map(|d| d.embedding).collect())
    }

    fn dimension(&self) -> usize {
        self.dimension
    }

    fn model_name(&self) -> &str {
        &self.model
    }
}

#[derive(Debug, Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingData>,
}

#[derive(Debug, Deserialize)]
struct EmbeddingData {
    embedding: Vec<f32>,
}
```

## Configuration Examples

### Basic Configuration

```json
{
  "memory": {
    "providers": [
      {
        "name": "openviking",
        "plugin": "openviking",
        "enabled": true,
        "config": {
          "url": "http://localhost:6333",
          "collection": "nanobot_memories",
          "dimension": 1536,
          "distance_metric": "cosine"
        }
      }
    ],
    "embedding": {
      "provider": "openai",
      "api_key": "${OPENAI_API_KEY}",
      "model": "text-embedding-3-small",
      "dimension": 1536
    }
  }
}
```

### Multi-Provider Configuration

```json
{
  "memory": {
    "providers": [
      {
        "name": "file",
        "plugin": "file",
        "enabled": true,
        "priority": 1,
        "config": {
          "workspace": "~/.nanobot/workspace"
        }
      },
      {
        "name": "openviking",
        "plugin": "openviking",
        "enabled": true,
        "priority": 2,
        "config": {
          "url": "http://localhost:6333",
          "collection": "nanobot_memories",
          "dimension": 1536,
          "distance_metric": "cosine",
          "min_similarity": 0.7
        }
      }
    ],
    "strategy": "hybrid",
    "embedding": {
      "provider": "openai",
      "api_key": "${OPENAI_API_KEY}",
      "model": "text-embedding-3-small"
    }
  }
}
```

## Usage Examples

### Storing Memories

```rust
use chrono::Utc;
use uuid::Uuid;

// Create memory entry
let entry = MemoryEntry {
    id: Uuid::new_v4().to_string(),
    content: "User prefers dark mode and uses vim keybindings".to_string(),
    embedding: None, // Will be generated automatically
    session_key: Some("user:alice".to_string()),
    memory_type: MemoryType::Semantic,
    importance: 0.9,
    created_at: Utc::now(),
    last_accessed_at: Utc::now(),
    access_count: 0,
    metadata: json!({
        "source": "conversation",
        "confidence": 0.95
    }),
    tags: vec!["preference".to_string(), "ui".to_string()],
};

// Store in OpenViking
let id = memory_provider.store(entry).await?;
println!("Stored memory with ID: {}", id);
```

### Querying Memories

```rust
// Simple semantic search
let query = MemoryQuery {
    text: "What are the user's UI preferences?".to_string(),
    embedding: None, // Will be generated automatically
    session_key: Some("user:alice".to_string()),
    memory_type: None,
    tags: None,
    min_importance: Some(0.5),
    limit: 5,
    time_range: None,
};

let results = memory_provider.query(&query).await?;

for result in results {
    println!(
        "Memory: {} (score: {:.3}, importance: {:.2})",
        result.entry.content,
        result.score,
        result.entry.importance
    );
}
```

### Filtered Search

```rust
use chrono::Duration;

// Search with filters
let query = MemoryQuery {
    text: "coding preferences".to_string(),
    embedding: None,
    session_key: Some("user:alice".to_string()),
    memory_type: Some(MemoryType::Semantic),
    tags: Some(vec!["preference".to_string()]),
    min_importance: Some(0.7),
    limit: 10,
    time_range: Some(TimeRange {
        start: Some(Utc::now() - Duration::days(30)),
        end: None,
    }),
};

let results = memory_provider.query(&query).await?;
```

### Batch Operations

```rust
// Batch store multiple memories
let entries = vec![
    MemoryEntry {
        id: Uuid::new_v4().to_string(),
        content: "User completed project X".to_string(),
        memory_type: MemoryType::Episodic,
        importance: 0.8,
        tags: vec!["achievement".to_string()],
        ..Default::default()
    },
    MemoryEntry {
        id: Uuid::new_v4().to_string(),
        content: "User learned Rust async programming".to_string(),
        memory_type: MemoryType::Procedural,
        importance: 0.9,
        tags: vec!["skill".to_string()],
        ..Default::default()
    },
];

let ids = memory_provider.store_batch(entries).await?;
println!("Stored {} memories", ids.len());
```

## Best Practices

### 1. Embedding Generation

- **Cache embeddings**: Store generated embeddings to avoid regeneration
- **Batch operations**: Use `embed_batch()` for multiple texts
- **Error handling**: Implement retry logic for API failures

### 2. Query Optimization

- **Use filters**: Reduce search space with metadata filters
- **Set appropriate limits**: Don't retrieve more than needed
- **Importance threshold**: Filter low-importance memories

### 3. Memory Management

- **Regular cleanup**: Remove old, low-importance memories
- **Deduplication**: Check for similar memories before storing
- **Importance scoring**: Update importance based on access patterns

### 4. Performance

- **Connection pooling**: Reuse HTTP connections
- **Batch operations**: Group multiple operations
- **Caching**: Cache frequently accessed memories

### 5. Error Handling

```rust
use backoff::{ExponentialBackoff, retry};

async fn store_with_retry(
    provider: &OpenVikingMemoryProvider,
    entry: MemoryEntry,
) -> Result<String> {
    let operation = || async {
        provider.store(entry.clone()).await
            .map_err(|e| {
                if e.to_string().contains("timeout") {
                    backoff::Error::Transient(e)
                } else {
                    backoff::Error::Permanent(e)
                }
            })
    };

    retry(ExponentialBackoff::default(), operation).await
}
```

## Testing

### Unit Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_openviking_store_and_query() {
        let config = OpenVikingConfig {
            url: "http://localhost:6333".to_string(),
            collection: "test_memories".to_string(),
            dimension: 1536,
            distance_metric: DistanceMetric::Cosine,
            ..Default::default()
        };

        let embedding_provider = Arc::new(MockEmbeddingProvider::new());
        let provider = OpenVikingMemoryProvider::new(config, embedding_provider)
            .await
            .unwrap();

        // Store a memory
        let entry = MemoryEntry {
            id: "test-1".to_string(),
            content: "Test memory".to_string(),
            memory_type: MemoryType::Semantic,
            importance: 0.8,
            ..Default::default()
        };

        let id = provider.store(entry).await.unwrap();
        assert_eq!(id, "test-1");

        // Query the memory
        let query = MemoryQuery {
            text: "Test".to_string(),
            limit: 1,
            ..Default::default()
        };

        let results = provider.query(&query).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].entry.content, "Test memory");
    }
}
```

### Integration Tests

```rust
#[tokio::test]
#[ignore] // Requires running OpenViking instance
async fn test_openviking_integration() {
    // Start OpenViking with Docker
    // docker run -p 6333:6333 qdrant/qdrant

    let config = OpenVikingConfig {
        url: "http://localhost:6333".to_string(),
        collection: "integration_test".to_string(),
        dimension: 1536,
        distance_metric: DistanceMetric::Cosine,
        ..Default::default()
    };

    let embedding_provider = Arc::new(
        OpenAIEmbeddingProvider::new(std::env::var("OPENAI_API_KEY").unwrap())
    );

    let provider = OpenVikingMemoryProvider::new(config, embedding_provider)
        .await
        .unwrap();

    // Run integration tests...
}
```

## Troubleshooting

### Common Issues

1. **Connection refused**
   - Ensure OpenViking is running
   - Check URL and port configuration

2. **Dimension mismatch**
   - Verify embedding dimension matches collection configuration
   - Recreate collection if needed

3. **Slow queries**
   - Add appropriate filters
   - Reduce limit
   - Check OpenViking resource usage

4. **Memory not found**
   - Verify collection name
   - Check if memory was actually stored
   - Inspect OpenViking logs

## Monitoring

### Metrics to Track

- Query latency
- Embedding generation time
- Storage operations per second
- Memory retrieval accuracy
- Cache hit rate

### Logging

```rust
use tracing::{info, warn, error, debug};

impl OpenVikingMemoryProvider {
    async fn query(&self, query: &MemoryQuery) -> Result<Vec<MemoryResult>> {
        let start = std::time::Instant::now();

        debug!(
            query_text = %query.text,
            limit = query.limit,
            "Executing memory query"
        );

        let results = self.execute_query(query).await?;

        info!(
            query_text = %query.text,
            results_count = results.len(),
            duration_ms = start.elapsed().as_millis(),
            "Memory query completed"
        );

        Ok(results)
    }
}
```

## Conclusion

This guide provides everything needed to integrate OpenViking as a memory backend for nanobot-rs, including:
- Complete API reference
- Implementation examples
- Configuration options
- Best practices
- Testing strategies

For more information, refer to:
- [Memory System Design](./MEMORY_SYSTEM_DESIGN.md)
