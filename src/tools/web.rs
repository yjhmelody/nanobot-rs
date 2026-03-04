use std::sync::{Arc, OnceLock};

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::tools::base::{JsonSchema, Tool, ToolContext, ToolDefinition, parse_args, schema_props};
use crate::tools::shared_config::SharedToolConfig;

#[derive(Debug, Deserialize)]
struct WebSearchArgs {
    query: String,
    count: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct WebFetchArgs {
    url: String,
    #[serde(rename = "maxChars", alias = "max_chars")]
    max_chars: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct BraveSearchResponse {
    web: Option<BraveWebData>,
}

#[derive(Debug, Deserialize)]
struct BraveWebData {
    #[serde(default)]
    results: Vec<BraveResult>,
}

#[derive(Debug, Deserialize)]
struct BraveResult {
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Debug, Serialize)]
struct WebFetchResponse {
    url: String,
    #[serde(rename = "finalUrl")]
    final_url: String,
    status: u16,
    extractor: String,
    truncated: bool,
    length: usize,
    text: String,
}

pub fn definitions() -> Vec<ToolDefinition> {
    static DEFS: OnceLock<Vec<ToolDefinition>> = OnceLock::new();
    DEFS.get_or_init(|| {
        vec![
            ToolDefinition::function(
                "web_search",
                "Search the web. Returns titles, URLs, and snippets.",
                JsonSchema::object(
                    schema_props([
                        ("query", JsonSchema::string(Some("Search query"))),
                        (
                            "count",
                            JsonSchema::integer(Some("Results (1-10)"))
                                .with_minimum(1)
                                .with_maximum(10),
                        ),
                    ]),
                    vec!["query"],
                ),
            ),
            ToolDefinition::function(
                "web_fetch",
                "Fetch URL and extract readable content (HTML to text).",
                JsonSchema::object(
                    schema_props([
                        ("url", JsonSchema::string(Some("URL to fetch"))),
                        ("maxChars", JsonSchema::integer(None).with_minimum(100)),
                    ]),
                    vec!["url"],
                ),
            ),
        ]
    })
    .clone()
}

pub fn build_tools(config: SharedToolConfig) -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(WebSearchTool::new(config.clone())),
        Arc::new(WebFetchTool::new(config)),
    ]
}

pub struct WebSearchTool {
    config: SharedToolConfig,
}

impl WebSearchTool {
    pub fn new(config: SharedToolConfig) -> Self {
        Self { config }
    }

    fn definition_static() -> ToolDefinition {
        definitions()
            .into_iter()
            .find(|d| d.function.name == "web_search")
            .expect("web_search definition")
    }
}

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn definition(&self) -> ToolDefinition {
        Self::definition_static()
    }

    async fn execute(&self, args_json: &str, _ctx: &ToolContext) -> Result<String> {
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

    fn definition_static() -> ToolDefinition {
        definitions()
            .into_iter()
            .find(|d| d.function.name == "web_fetch")
            .expect("web_fetch definition")
    }
}

#[async_trait]
impl Tool for WebFetchTool {
    fn name(&self) -> &str {
        "web_fetch"
    }

    fn definition(&self) -> ToolDefinition {
        Self::definition_static()
    }

    async fn execute(&self, args_json: &str, _ctx: &ToolContext) -> Result<String> {
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
) -> Result<String> {
    let typed = parse_args::<WebSearchArgs>(args_json)?;
    let query = typed.query;

    if api_key.trim().is_empty() {
        bail!(
            "Brave Search API key not configured. Set tools.web.search.apiKey in ~/.nanobot/config.json"
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

    let resp = res.context("requesting Brave Search API")?;
    if !resp.status().is_success() {
        let code = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        bail!("HTTP {}: {}", code, text);
    }

    let parsed: BraveSearchResponse = resp
        .json()
        .await
        .context("failed to parse search response")?;

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
) -> Result<String> {
    let typed = parse_args::<WebFetchArgs>(args_json)?;
    let url = typed.url;
    let max_chars = typed
        .max_chars
        .map(|v| v.max(100) as usize)
        .unwrap_or(max_chars_default);

    let parsed = Url::parse(&url).with_context(|| format!("URL validation failed: {}", url))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        bail!("URL validation failed: only http/https allowed");
    }

    let client = build_client(proxy)?;

    let res = client
        .get(&parsed.to_string())
        .header("User-Agent", "Mozilla/5.0 nanobot-rs")
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await;

    let resp = res.context("fetching web content")?;

    let status = resp.status().as_u16();
    let final_url = resp.url().to_string();
    let ctype = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("")
        .to_string();

    let body = resp.text().await.context("reading web response body")?;

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

    let truncated = text.len() > max_chars;
    if truncated {
        text.truncate(max_chars);
    }

    serde_json::to_string(&WebFetchResponse {
        url: parsed.to_string(),
        final_url,
        status,
        extractor: extractor.to_string(),
        truncated,
        length: text.len(),
        text,
    })
    .map_err(|e| anyhow!("serializing web_fetch response: {}", e))
}

fn build_client(proxy: Option<&str>) -> Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder();
    if let Some(proxy_url) = proxy {
        if !proxy_url.trim().is_empty() {
            let proxy = reqwest::Proxy::all(proxy_url)
                .with_context(|| format!("invalid proxy: {}", proxy_url))?;
            builder = builder.proxy(proxy);
        }
    }
    builder.build().context("building web client")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn execute_search_requires_api_key() {
        let err = execute_search(r#"{"query":"rust"}"#, "", 5, None)
            .await
            .expect_err("missing api key should fail");
        assert!(
            err.to_string()
                .contains("Brave Search API key not configured")
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
}
