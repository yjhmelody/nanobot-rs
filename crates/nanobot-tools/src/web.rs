use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use serde_json::json;
use url::Url;

use crate::base::{Tool, ToolContext, ToolDefinition, parse_args, tool_definition_from_json};
use crate::config::SharedToolConfig;
use crate::error::{ToolError, ToolResult};
use nanobot_types::tools::{BraveSearchResponse, WebFetchArgs, WebFetchResponse, WebSearchArgs};

// Tool descriptions
const WEB_SEARCH_DESC: &str = "Search the web. Returns titles, URLs, and snippets.";
const WEB_SEARCH_QUERY_DESC: &str = "Search query";
const WEB_SEARCH_COUNT_DESC: &str = "Results (1-10)";

const WEB_FETCH_DESC: &str = "Fetch URL and extract readable content (HTML to text).";
const WEB_FETCH_URL_DESC: &str = "URL to fetch";

pub struct WebSearchTool {
    config: SharedToolConfig,
}

impl WebSearchTool {
    pub fn new(config: SharedToolConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn definition(&self) -> Arc<ToolDefinition> {
        static DEF: OnceLock<Arc<ToolDefinition>> = OnceLock::new();
        DEF.get_or_init(|| {
            Arc::new(tool_definition_from_json(json!({
                "type": "function",
                "function": {
                    "name": "web_search",
                    "description": WEB_SEARCH_DESC,
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "query": {
                                "type": "string",
                                "description": WEB_SEARCH_QUERY_DESC
                            },
                            "count": {
                                "type": "integer",
                                "description": WEB_SEARCH_COUNT_DESC,
                                "minimum": 1,
                                "maximum": 10
                            }
                        },
                        "required": ["query"]
                    }
                }
            })))
        })
        .clone()
    }

    async fn execute(&self, args_json: &str, _ctx: &ToolContext) -> ToolResult<String> {
        let snapshot = self.config.snapshot().await;
        execute_search(
            args_json,
            &snapshot.web.search_api_key,
            snapshot.web.search_max_results,
            snapshot.web.proxy.as_deref(),
        )
        .await
    }
}

pub struct WebFetchTool {
    config: SharedToolConfig,
}

impl WebFetchTool {
    pub fn new(config: SharedToolConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Tool for WebFetchTool {
    fn name(&self) -> &str {
        "web_fetch"
    }

    fn definition(&self) -> Arc<ToolDefinition> {
        static DEF: OnceLock<Arc<ToolDefinition>> = OnceLock::new();
        DEF.get_or_init(|| {
            Arc::new(tool_definition_from_json(json!({
                "type": "function",
                "function": {
                    "name": "web_fetch",
                    "description": WEB_FETCH_DESC,
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "url": {
                                "type": "string",
                                "description": WEB_FETCH_URL_DESC
                            },
                            "max_chars": {
                                "type": "integer",
                                "minimum": 100
                            }
                        },
                        "required": ["url"]
                    }
                }
            })))
        })
        .clone()
    }

    async fn execute(&self, args_json: &str, _ctx: &ToolContext) -> ToolResult<String> {
        let snapshot = self.config.snapshot().await;
        const DEFAULT_FETCH_MAX_CHARS: usize = 50_000;
        execute_fetch(
            args_json,
            DEFAULT_FETCH_MAX_CHARS,
            snapshot.web.proxy.as_deref(),
        )
        .await
    }
}

pub async fn execute_search(
    args_json: &str,
    api_key: &str,
    max_results: usize,
    proxy: Option<&str>,
) -> ToolResult<String> {
    let typed = parse_args::<WebSearchArgs>(args_json)?;
    let query = typed.query;

    if api_key.trim().is_empty() {
        return Ok(
            "web_search unavailable: Brave Search API key not configured. Set tools.web.search.apiKey in ~/.nanobot/config.json".to_string(),
        );
    }

    let count = typed.count.unwrap_or(max_results as i64).clamp(1, 10) as usize;

    let client = build_client(proxy)?;

    let res = client
        .get("https://api.search.brave.com/res/v1/web/search")
        .query(&[("q", query.as_str()), ("count", &count.to_string())])
        .header("Accept", "application/json")
        .header("X-Subscription-Token", api_key)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await;

    let resp = res.map_err(|e| {
        ToolError::execution(
            "web_search",
            anyhow::anyhow!("requesting Brave Search API: {}", e),
        )
    })?;
    if !resp.status().is_success() {
        let code = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        return Err(ToolError::execution(
            "web_search",
            anyhow::anyhow!("HTTP {}: {}", code, text),
        ));
    }

    let parsed: BraveSearchResponse = resp.json().await.map_err(|e| {
        ToolError::execution(
            "web_search",
            anyhow::anyhow!("failed to parse search response: {}", e),
        )
    })?;

    let results = parsed.web.map(|w| w.results).unwrap_or_default();

    if results.is_empty() {
        return Ok(format!("No results for: {}", query));
    }

    let mut lines = vec![format!("Results for: {}\n", query)];
    for (idx, item) in results.iter().take(count).enumerate() {
        lines.push(format!("{}. {}\n   {}", idx + 1, item.title, item.url));
        if let Some(desc) = &item.description {
            lines.push(format!("   {}", desc));
        }
    }
    Ok(lines.join("\n"))
}

pub async fn execute_fetch(
    args_json: &str,
    max_chars_default: usize,
    proxy: Option<&str>,
) -> ToolResult<String> {
    let typed = parse_args::<WebFetchArgs>(args_json)?;
    let url = typed.url;
    let max_chars = typed
        .max_chars
        .map(|v| v.max(100) as usize)
        .unwrap_or(max_chars_default);

    let parsed = Url::parse(&url).map_err(|e| {
        ToolError::execution(
            "web_fetch",
            anyhow::anyhow!("URL validation failed: {}: {}", url, e),
        )
    })?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(ToolError::execution(
            "web_fetch",
            anyhow::anyhow!("URL validation failed: only http/https allowed"),
        ));
    }

    let client = build_client(proxy)?;

    let res = client
        .get(parsed.to_string())
        .header("User-Agent", "Mozilla/5.0")
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await;

    let resp = res.map_err(|e| {
        ToolError::execution("web_fetch", anyhow::anyhow!("fetching web content: {}", e))
    })?;

    let status = resp.status().as_u16();
    let final_url = resp.url().to_string();
    let ctype = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("")
        .to_string();

    let body = resp.text().await.map_err(|e| {
        ToolError::execution(
            "web_fetch",
            anyhow::anyhow!("reading web response body: {}", e),
        )
    })?;

    let (extractor, mut text) = if ctype.contains("application/json") {
        ("json", body)
    } else if ctype.contains("text/html")
        || body.trim_start().to_lowercase().starts_with("<html")
        || body.trim_start().to_lowercase().starts_with("<!doctype")
    {
        let rendered = html2text::from_read(body.as_bytes(), 100).unwrap_or_else(|_| body.clone());
        ("html2text", rendered)
    } else {
        ("raw", body)
    };

    let truncated = truncate_utf8_to_max_bytes(&mut text, max_chars);

    serde_json::to_string(&WebFetchResponse {
        url: parsed.to_string(),
        final_url,
        status,
        extractor: extractor.to_string(),
        truncated,
        length: text.len(),
        text,
    })
    .map_err(|e| {
        ToolError::execution(
            "web_fetch",
            anyhow::anyhow!("serializing web_fetch response: {}", e),
        )
    })
}

fn truncate_utf8_to_max_bytes(text: &mut String, max_bytes: usize) -> bool {
    if text.len() <= max_bytes {
        return false;
    }

    let mut new_len = max_bytes.min(text.len());
    while new_len > 0 && !text.is_char_boundary(new_len) {
        new_len -= 1;
    }

    text.truncate(new_len);
    true
}

fn build_client(proxy: Option<&str>) -> ToolResult<reqwest::Client> {
    let mut builder = reqwest::Client::builder();
    if let Some(proxy_url) = proxy
        && !proxy_url.trim().is_empty()
    {
        let proxy = reqwest::Proxy::all(proxy_url).map_err(|e| {
            ToolError::execution(
                "web",
                anyhow::anyhow!("invalid proxy: {}: {}", proxy_url, e),
            )
        })?;
        builder = builder.proxy(proxy);
    }
    builder
        .build()
        .map_err(|e| ToolError::execution("web", anyhow::anyhow!("building web client: {}", e)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn execute_search_without_api_key_returns_guidance() {
        let output = execute_search(r#"{"query":"rust"}"#, "", 5, None)
            .await
            .expect("missing api key should return guidance");
        assert!(
            output.contains("Brave Search API key not configured")
        );
    }

    #[tokio::test]
    async fn execute_fetch_rejects_non_http_scheme() {
        let err = execute_fetch(r#"{"url":"ftp://example.com"}"#, 10_000, None)
            .await
            .expect_err("non-http scheme should fail");
        assert!(
            err.to_string()
                .contains("URL validation failed: only http/https allowed")
        );
    }

    #[tokio::test]
    async fn execute_fetch_rejects_invalid_url() {
        let err = execute_fetch(r#"{"url":"://bad-url"}"#, 10_000, None)
            .await
            .expect_err("invalid url should fail");
        assert!(err.to_string().contains("URL validation failed"));
    }

    #[test]
    fn build_client_rejects_invalid_proxy() {
        let err = build_client(Some("://bad proxy")).expect_err("invalid proxy should fail");
        assert!(err.to_string().contains("invalid proxy"));
    }

    #[test]
    fn truncate_utf8_to_max_bytes_respects_char_boundaries() {
        let mut value = "中文ab".to_string();
        let truncated = truncate_utf8_to_max_bytes(&mut value, 5);
        assert!(truncated);
        assert_eq!(value, "中");
    }

    #[test]
    fn truncate_utf8_to_max_bytes_noop_when_within_limit() {
        let mut value = "hello".to_string();
        let truncated = truncate_utf8_to_max_bytes(&mut value, 5);
        assert!(!truncated);
        assert_eq!(value, "hello");
    }
}
