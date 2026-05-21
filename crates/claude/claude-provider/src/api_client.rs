//! Full API client for Anthropic-compatible providers.
//!
//! Provides [`ApiClient`] with high-level methods for streaming and
//! non-streaming queries, API key verification, usage tracking, and
//! metadata retrieval.
//!
//! Based on upstream Claude Code's `services/api/claude.ts` (3,420 lines).

use anyhow::{Context, Result, anyhow};
use claude_config::ProviderConfig;
use claude_core::{ProviderProtocol, ProviderResponse, UsageSummary};
use reqwest::Client;
use reqwest::header::{
    ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue, USER_AGENT,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::Duration;
use tracing::{debug, info, warn};

use crate::attribution::build_attribution_header;
use crate::beta_headers::{self, get_beta_headers};
use crate::cache_headers::{add_cache_breakpoints, is_prompt_caching_enabled, should_1h_cache_ttl};
use crate::effort_params::configure_effort_params;
use crate::fingerprint::compute_message_fingerprint;
use crate::max_tokens::{adjust_params_for_non_streaming, get_max_output_tokens_for_model};
use crate::retry::{RetryConfig, with_retry};

// ---------------------------------------------------------------------------
// Core types
// ---------------------------------------------------------------------------

/// Content block in an API response.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlock {
    /// Text content.
    #[serde(rename = "text")]
    Text {
        /// The text content.
        text: String,
    },
    /// Tool use request.
    #[serde(rename = "tool_use")]
    ToolUse {
        /// Tool call identifier.
        id: String,
        /// Tool name.
        name: String,
        /// Tool input as JSON.
        input: Value,
    },
    /// Extended thinking content.
    #[serde(rename = "thinking")]
    Thinking {
        /// The thinking content.
        thinking: String,
    },
    /// Server-side tool use (e.g. web search, web fetch).
    #[serde(rename = "server_tool_use")]
    ServerToolUse {
        /// Tool call identifier.
        id: String,
        /// Tool name.
        name: String,
        /// Tool input as JSON.
        input: Value,
    },
}

/// Usage statistics from an API response.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct UsageStats {
    /// Input (prompt) tokens.
    #[serde(default)]
    pub input_tokens: u64,
    /// Output (completion) tokens.
    #[serde(default)]
    pub output_tokens: u64,
    /// Cache creation input tokens.
    #[serde(default)]
    pub cache_creation_input_tokens: u64,
    /// Cache read input tokens.
    #[serde(default)]
    pub cache_read_input_tokens: u64,
    /// Server-side web search requests (Anthropic server tool use).
    #[serde(default)]
    pub server_tool_use_web_search_requests: u64,
    /// Server-side web fetch requests (Anthropic server tool use).
    #[serde(default)]
    pub server_tool_use_web_fetch_requests: u64,
    /// Cache creation ephemeral 5-minute TTL input tokens.
    #[serde(default)]
    pub cache_creation_ephemeral_5m_input_tokens: u64,
    /// Cache creation ephemeral 1-hour TTL input tokens.
    #[serde(default)]
    pub cache_creation_ephemeral_1h_input_tokens: u64,
}

/// Result of an API query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryResult {
    /// Content blocks in the response.
    pub content: Vec<ContentBlock>,
    /// Usage statistics.
    pub usage: UsageStats,
    /// Model that produced the response.
    pub model: String,
    /// Stop reason (e.g. "end_turn", "tool_use").
    pub stop_reason: Option<String>,
    /// Request ID from the API.
    pub request_id: Option<String>,
}

/// Options for an API query.
#[derive(Debug, Clone)]
pub struct QueryOptions {
    /// Model identifier.
    pub model: String,
    /// System prompt blocks.
    pub system_prompt: Vec<Value>,
    /// Conversation messages.
    pub messages: Vec<Value>,
    /// Tool definitions.
    pub tools: Vec<Value>,
    /// Maximum output tokens.
    pub max_tokens: u32,
    /// Temperature (0.0–1.0).
    pub temperature: Option<f64>,
    /// Effort level ("low", "medium", "high").
    pub effort_level: Option<String>,
    /// Whether to use streaming.
    pub stream: bool,
    /// Stop sequences.
    pub stop_sequences: Vec<String>,
    /// Thinking budget (Anthropic extended thinking).
    pub thinking_budget: Option<u32>,
}

// ---------------------------------------------------------------------------
// Usage tracking
// ---------------------------------------------------------------------------

/// Update usage statistics with new partial data from streaming events.
///
/// Anthropic's streaming API provides cumulative usage totals, not
/// incremental deltas.  Each event contains the complete usage up to that
/// point.  Input-related tokens are only updated if the new value is
/// non-zero (to avoid overwriting real values with zeros from
/// `message_delta` events).
pub fn update_usage(current: &UsageStats, partial: &UsageStats) -> UsageStats {
    UsageStats {
        input_tokens: if partial.input_tokens > 0 {
            partial.input_tokens
        } else {
            current.input_tokens
        },
        output_tokens: if partial.output_tokens > 0 {
            partial.output_tokens
        } else {
            current.output_tokens
        },
        cache_creation_input_tokens: if partial.cache_creation_input_tokens > 0 {
            partial.cache_creation_input_tokens
        } else {
            current.cache_creation_input_tokens
        },
        cache_read_input_tokens: if partial.cache_read_input_tokens > 0 {
            partial.cache_read_input_tokens
        } else {
            current.cache_read_input_tokens
        },
        server_tool_use_web_search_requests: if partial.server_tool_use_web_search_requests > 0 {
            partial.server_tool_use_web_search_requests
        } else {
            current.server_tool_use_web_search_requests
        },
        server_tool_use_web_fetch_requests: if partial.server_tool_use_web_fetch_requests > 0 {
            partial.server_tool_use_web_fetch_requests
        } else {
            current.server_tool_use_web_fetch_requests
        },
        cache_creation_ephemeral_5m_input_tokens: if partial
            .cache_creation_ephemeral_5m_input_tokens
            > 0
        {
            partial.cache_creation_ephemeral_5m_input_tokens
        } else {
            current.cache_creation_ephemeral_5m_input_tokens
        },
        cache_creation_ephemeral_1h_input_tokens: if partial
            .cache_creation_ephemeral_1h_input_tokens
            > 0
        {
            partial.cache_creation_ephemeral_1h_input_tokens
        } else {
            current.cache_creation_ephemeral_1h_input_tokens
        },
    }
}

/// Accumulate usage from one message into a running total.
///
/// Used to track cumulative usage across multiple assistant turns.
pub fn accumulate_usage(total: &UsageStats, message: &UsageStats) -> UsageStats {
    UsageStats {
        input_tokens: total.input_tokens + message.input_tokens,
        output_tokens: total.output_tokens + message.output_tokens,
        cache_creation_input_tokens: total.cache_creation_input_tokens
            + message.cache_creation_input_tokens,
        cache_read_input_tokens: total.cache_read_input_tokens + message.cache_read_input_tokens,
        server_tool_use_web_search_requests: total.server_tool_use_web_search_requests
            + message.server_tool_use_web_search_requests,
        server_tool_use_web_fetch_requests: total.server_tool_use_web_fetch_requests
            + message.server_tool_use_web_fetch_requests,
        cache_creation_ephemeral_5m_input_tokens: total.cache_creation_ephemeral_5m_input_tokens
            + message.cache_creation_ephemeral_5m_input_tokens,
        cache_creation_ephemeral_1h_input_tokens: total.cache_creation_ephemeral_1h_input_tokens
            + message.cache_creation_ephemeral_1h_input_tokens,
    }
}

// ---------------------------------------------------------------------------
// ApiClient
// ---------------------------------------------------------------------------

/// High-level API client for Anthropic-compatible providers.
///
/// Wraps [`reqwest::Client`] with provider-aware request building,
/// retry logic, and response parsing.
pub struct ApiClient {
    http: Client,
}

impl ApiClient {
    /// Create a new API client.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying HTTP client cannot be constructed.
    pub fn new() -> Result<Self> {
        let timeout_secs = std::env::var("CLAUDE_CODE_API_TIMEOUT_SECS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(600);
        let http = Client::builder()
            .timeout(Duration::from_secs(timeout_secs))
            .build()
            .context("failed to build API HTTP client")?;
        Ok(Self { http })
    }

    // -----------------------------------------------------------------------
    // Streaming query
    // -----------------------------------------------------------------------

    /// Send a streaming query to the API.
    ///
    /// Returns the raw response for the caller to process SSE events.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails after all retries.
    pub async fn query_model_streaming(
        &self,
        provider: &ProviderConfig,
        options: &QueryOptions,
    ) -> Result<reqwest::Response> {
        let retry_config = RetryConfig::from_provider(
            provider.max_retries,
            provider.retry_initial_backoff_ms,
            provider.retry_max_backoff_ms,
        );

        let body = self.build_request_body(provider, options, true)?;
        let url = self.resolve_url(provider)?;
        let headers = self.build_request_headers(provider, options)?;

        debug!(model = %options.model, "sending streaming query");

        let http = &self.http;
        let url_clone = url.clone();
        let headers_clone = headers.clone();
        let body_clone = body.clone();

        with_retry(&retry_config, &options.model, |_ctx| {
            let http = http.clone();
            let url = url_clone.clone();
            let headers = headers_clone.clone();
            let body = body_clone.clone();
            async move {
                let response = http
                    .post(&url)
                    .headers(headers)
                    .json(&body)
                    .send()
                    .await
                    .context("streaming request failed")?;

                let status = response.status().as_u16();
                if status >= 400 {
                    let text = response.text().await.unwrap_or_default();
                    return Err(anyhow!(
                        "streaming request failed ({status}): {}",
                        truncate(&text)
                    ));
                }

                Ok(response)
            }
        })
        .await
    }

    // -----------------------------------------------------------------------
    // Non-streaming query
    // -----------------------------------------------------------------------

    /// Send a non-streaming query to the API.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails after all retries.
    pub async fn query_model_without_streaming(
        &self,
        provider: &ProviderConfig,
        options: &QueryOptions,
    ) -> Result<QueryResult> {
        let retry_config = RetryConfig::from_provider(
            provider.max_retries,
            provider.retry_initial_backoff_ms,
            provider.retry_max_backoff_ms,
        );

        let mut body = self.build_request_body(provider, options, false)?;
        adjust_params_for_non_streaming(&mut body);
        let url = self.resolve_url(provider)?;
        let headers = self.build_request_headers(provider, options)?;

        let fingerprint = compute_message_fingerprint(
            body.get("messages")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
                .as_slice(),
        );
        info!(model = %options.model, %fingerprint, "sending non-streaming query");

        let http = &self.http;
        let url_clone = url.clone();
        let headers_clone = headers.clone();

        with_retry(&retry_config, &options.model, move |_ctx| {
            let http = http.clone();
            let url = url_clone.clone();
            let headers = headers_clone.clone();
            let body = body.clone();
            async move {
                let response = http
                    .post(&url)
                    .headers(headers)
                    .json(&body)
                    .send()
                    .await
                    .context("non-streaming request failed")?;

                let status = response.status().as_u16();
                let text = response
                    .text()
                    .await
                    .context("failed to read response body")?;

                if status >= 400 {
                    return Err(anyhow!("request failed ({status}): {}", truncate(&text)));
                }

                parse_query_result(&text)
            }
        })
        .await
    }

    // -----------------------------------------------------------------------
    // Haiku shortcut
    // -----------------------------------------------------------------------

    /// Send a quick query using a small/fast model (Haiku-equivalent).
    ///
    /// Convenience method for lightweight tasks like classification,
    /// summarization, or title generation.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    pub async fn query_haiku(
        &self,
        provider: &ProviderConfig,
        system_prompt: &str,
        user_prompt: &str,
    ) -> Result<QueryResult> {
        let small_model = resolve_small_model(provider);
        let max_tokens = get_max_output_tokens_for_model(&small_model).min(4096);

        let options = QueryOptions {
            model: small_model,
            system_prompt: vec![json!({"type": "text", "text": system_prompt})],
            messages: vec![json!({"role": "user", "content": user_prompt})],
            tools: vec![],
            max_tokens,
            temperature: Some(0.0),
            effort_level: None,
            stream: false,
            stop_sequences: vec![],
            thinking_budget: None,
        };

        self.query_model_without_streaming(provider, &options).await
    }

    // -----------------------------------------------------------------------
    // API key verification
    // -----------------------------------------------------------------------

    /// Verify that the API key is valid by making a lightweight request.
    ///
    /// # Errors
    ///
    /// Returns an error if the API key is invalid or the request fails.
    pub async fn verify_api_key(&self, provider: &ProviderConfig) -> Result<bool> {
        let url = self.resolve_url(provider)?;
        let headers = build_auth_headers(provider)?;

        let body = json!({
            "model": provider.model.as_deref().unwrap_or("claude-sonnet-4-20250514"),
            "max_tokens": 1,
            "messages": [{"role": "user", "content": "ping"}],
        });

        let response = self
            .http
            .post(&url)
            .headers(headers)
            .json(&body)
            .timeout(Duration::from_secs(10))
            .send()
            .await;

        match response {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    Ok(true)
                } else if status.is_client_error() {
                    // 4xx = key invalid or bad request
                    Ok(false)
                } else {
                    // 5xx = server error, cannot verify key validity
                    Ok(false)
                }
            }
            Err(_) => Ok(false),
        }
    }

    // -----------------------------------------------------------------------
    // API metadata
    // -----------------------------------------------------------------------

    /// Get API metadata (model info, rate limits, etc.).
    ///
    /// # Errors
    ///
    /// Returns an error if the metadata request fails.
    pub async fn get_api_metadata(&self, provider: &ProviderConfig) -> Result<Value> {
        let base_url = provider
            .base_url
            .as_deref()
            .ok_or_else(|| anyhow!("provider missing base URL"))?;

        // For Anthropic, try the /v1/models endpoint.
        let metadata_url = if base_url.contains("/v1/messages") {
            base_url.replace("/v1/messages", "/v1/models")
        } else if base_url.ends_with("/messages") {
            format!("{base_url}/../models")
        } else {
            format!("{base_url}/models")
        };

        let headers = build_auth_headers(provider)?;

        let response = self
            .http
            .get(&metadata_url)
            .headers(headers)
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .context("metadata request failed")?;

        let status = response.status().as_u16();
        let text = response
            .text()
            .await
            .context("failed to read metadata response")?;

        if status >= 400 {
            // Return empty metadata rather than failing.
            warn!(status, "metadata request failed, returning empty");
            return Ok(json!({}));
        }

        serde_json::from_str(&text).context("failed to parse metadata response")
    }

    // -----------------------------------------------------------------------
    // Request building helpers
    // -----------------------------------------------------------------------

    /// Build the full request body for an API query.
    fn build_request_body(
        &self,
        provider: &ProviderConfig,
        options: &QueryOptions,
        stream: bool,
    ) -> Result<Value> {
        let mut body = json!({
            "model": options.model,
            "max_tokens": options.max_tokens,
            "messages": options.messages,
            "stream": stream,
        });

        // System prompt.
        if !options.system_prompt.is_empty() {
            let mut system = options.system_prompt.clone();

            // Prepend billing attribution as first block for Anthropic providers.
            if matches!(provider.protocol, ProviderProtocol::Anthropic) {
                let messages = body
                    .get("messages")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let fp = crate::fingerprint::compute_attribution_fingerprint(
                    &messages,
                    claude_config::runtime_version(),
                );
                let attr_text = crate::attribution::build_billing_attribution_text(&fp);
                system.insert(0, json!({"type": "text", "text": attr_text}));
            }

            body["system"] = json!(system);
        }

        // Tools.
        if !options.tools.is_empty() {
            body["tools"] = json!(options.tools);
        }

        // Temperature.
        if let Some(temp) = options.temperature {
            body["temperature"] = json!(temp);
        }

        // Stop sequences.
        if !options.stop_sequences.is_empty() {
            body["stop_sequences"] = json!(options.stop_sequences);
        }

        // Extended thinking.
        if let Some(budget) = options.thinking_budget {
            body["thinking"] = json!({
                "type": "enabled",
                "budget_tokens": budget,
            });
            // Ensure max_tokens > budget_tokens.
            let current_max = body.get("max_tokens").and_then(Value::as_u64).unwrap_or(0);
            if current_max <= u64::from(budget) {
                body["max_tokens"] = json!(u64::from(budget) + 4096);
            }
        }

        // Effort level.
        let mut betas = Vec::new();
        configure_effort_params(
            &mut body,
            &mut betas,
            &options.model,
            options.effort_level.as_deref(),
        );

        // Cache breakpoints.
        let enable_caching = is_prompt_caching_enabled(&options.model);
        if enable_caching {
            let use_1h = should_1h_cache_ttl();
            add_cache_breakpoints(&mut body, use_1h);
        }

        // Extra body params from environment.
        let extra_params = beta_headers::get_extra_body_params(None);
        if let Some(extra_obj) = extra_params.as_object() {
            for (key, value) in extra_obj {
                if body.get(key).is_none() {
                    body[key.clone()] = value.clone();
                }
            }
        }

        // Apply provider-specific metadata.
        crate::apply_anthropic_request_metadata(&mut body, provider, None);

        Ok(body)
    }

    /// Build the request headers.
    fn build_request_headers(
        &self,
        provider: &ProviderConfig,
        options: &QueryOptions,
    ) -> Result<HeaderMap> {
        let mut headers = build_auth_headers(provider)?;

        // Beta headers.
        let is_bedrock = provider.protocol == ProviderProtocol::Bedrock;
        let is_vertex = provider.protocol == ProviderProtocol::Vertex;
        let enable_caching = is_prompt_caching_enabled(&options.model);
        let enable_thinking = options.thinking_budget.is_some();

        let betas = get_beta_headers(
            &options.model,
            is_bedrock,
            is_vertex,
            enable_caching,
            enable_thinking,
        );

        if !betas.is_empty()
            && let Ok((name, value)) = beta_headers::build_beta_header_pair(&betas)
        {
            headers.insert(name, value);
        }

        // Attribution header.
        if let Ok(value) = build_attribution_header() {
            headers.insert(HeaderName::from_static("x-attribution"), value);
        }

        Ok(headers)
    }

    /// Resolve the API URL from the provider config.
    fn resolve_url(&self, provider: &ProviderConfig) -> Result<String> {
        provider
            .base_url
            .clone()
            .ok_or_else(|| anyhow!("provider is missing a base URL"))
    }
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Build authentication headers from the provider config.
fn build_auth_headers(provider: &ProviderConfig) -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert(ACCEPT, HeaderValue::from_static("application/json"));

    if matches!(provider.protocol, ProviderProtocol::Anthropic) {
        headers.insert(
            HeaderName::from_static("x-app"),
            HeaderValue::from_static("cli"),
        );
        headers.insert(
            USER_AGENT,
            HeaderValue::from_str(&super::claude_code_user_agent())?,
        );
        let session_id = provider
            .request_metadata
            .get("session_id")
            .filter(|v| !v.trim().is_empty())
            .cloned()
            .unwrap_or_else(|| "unknown".to_owned());
        headers.insert(
            HeaderName::from_static("x-claude-code-session-id"),
            HeaderValue::from_str(&session_id)?,
        );
        headers.insert(
            HeaderName::from_static("anthropic-version"),
            HeaderValue::from_static("2023-06-01"),
        );
    } else {
        headers.insert(
            USER_AGENT,
            HeaderValue::from_str(&format!("remote-code-rust/{}", env!("CARGO_PKG_VERSION")))?,
        );
    }

    if let Some(api_key) = &provider.api_key {
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {api_key}"))?,
        );
        headers.insert(
            HeaderName::from_static("x-api-key"),
            HeaderValue::from_str(api_key)?,
        );
    }

    // Apply user-supplied header overrides.
    for (name, value) in &provider.request_header_overrides {
        let header_name = HeaderName::from_bytes(name.as_bytes())
            .with_context(|| format!("invalid header name {name}"))?;
        let header_value = HeaderValue::from_str(value)
            .with_context(|| format!("invalid header value for {name}"))?;
        headers.insert(header_name, header_value);
    }

    // Parse ANTHROPIC_CUSTOM_HEADERS env var (newline-separated "Name: Value" pairs).
    if let Ok(custom_headers) = std::env::var("ANTHROPIC_CUSTOM_HEADERS") {
        for line in custom_headers.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Some((name, value)) = line.split_once(':') {
                let name = name.trim();
                let value = value.trim();
                if let Ok(header_name) = HeaderName::from_bytes(name.as_bytes())
                    && let Ok(header_value) = HeaderValue::from_str(value)
                {
                    headers.insert(header_name, header_value);
                }
            }
        }
    }

    // CCR (Claude Code Remote) headers.
    if let Ok(container_id) = std::env::var("CLAUDE_CODE_REMOTE_CONTAINER_ID")
        && let Ok(value) = HeaderValue::from_str(&container_id)
    {
        headers.insert(
            HeaderName::from_static("x-claude-remote-container-id"),
            value,
        );
    }
    if let Ok(session_id) = std::env::var("CLAUDE_CODE_REMOTE_SESSION_ID")
        && let Ok(value) = HeaderValue::from_str(&session_id)
    {
        headers.insert(HeaderName::from_static("x-claude-remote-session-id"), value);
    }

    Ok(headers)
}

/// Parse a non-streaming API response into a [`QueryResult`].
fn parse_query_result(raw: &str) -> Result<QueryResult> {
    let payload: Value = serde_json::from_str(raw)
        .with_context(|| format!("response is not valid JSON: {}", truncate(raw)))?;

    let content_blocks = payload
        .get("content")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let content: Vec<ContentBlock> = content_blocks
        .iter()
        .filter_map(|block| {
            let block_type = block.get("type").and_then(Value::as_str)?;
            match block_type {
                "text" => Some(ContentBlock::Text {
                    text: block.get("text").and_then(Value::as_str)?.to_owned(),
                }),
                "tool_use" => Some(ContentBlock::ToolUse {
                    id: block.get("id").and_then(Value::as_str)?.to_owned(),
                    name: block.get("name").and_then(Value::as_str)?.to_owned(),
                    input: block.get("input").cloned().unwrap_or(json!({})),
                }),
                "thinking" => Some(ContentBlock::Thinking {
                    thinking: block.get("thinking").and_then(Value::as_str)?.to_owned(),
                }),
                "server_tool_use" => Some(ContentBlock::ServerToolUse {
                    id: block.get("id").and_then(Value::as_str)?.to_owned(),
                    name: block.get("name").and_then(Value::as_str)?.to_owned(),
                    input: block.get("input").cloned().unwrap_or(json!({})),
                }),
                _ => None,
            }
        })
        .collect();

    let usage_value = payload.get("usage").cloned().unwrap_or_default();
    let usage = UsageStats {
        input_tokens: usage_value
            .get("input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or_default(),
        output_tokens: usage_value
            .get("output_tokens")
            .and_then(Value::as_u64)
            .unwrap_or_default(),
        cache_creation_input_tokens: usage_value
            .get("cache_creation_input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or_default(),
        cache_read_input_tokens: usage_value
            .get("cache_read_input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or_default(),
        server_tool_use_web_search_requests: usage_value
            .get("server_tool_use")
            .and_then(|stu| stu.get("web_search_requests"))
            .and_then(Value::as_u64)
            .unwrap_or_default(),
        server_tool_use_web_fetch_requests: usage_value
            .get("server_tool_use")
            .and_then(|stu| stu.get("web_fetch_requests"))
            .and_then(Value::as_u64)
            .unwrap_or_default(),
        cache_creation_ephemeral_5m_input_tokens: usage_value
            .get("cache_creation")
            .and_then(|cc| cc.get("ephemeral_5m_input_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or_default(),
        cache_creation_ephemeral_1h_input_tokens: usage_value
            .get("cache_creation")
            .and_then(|cc| cc.get("ephemeral_1h_input_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or_default(),
    };

    Ok(QueryResult {
        content,
        usage,
        model: payload
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned(),
        stop_reason: payload
            .get("stop_reason")
            .and_then(Value::as_str)
            .map(String::from),
        request_id: payload.get("id").and_then(Value::as_str).map(String::from),
    })
}

/// Resolve the small/fast model name from the provider config.
fn resolve_small_model(provider: &ProviderConfig) -> String {
    let current = provider.model.as_deref().unwrap_or("");
    let model_lower = current.to_ascii_lowercase();

    // Map known model families to their small/fast variant.
    if model_lower.contains("claude") {
        if model_lower.contains("sonnet-4") {
            return "claude-haiku-4-20250514".to_owned();
        }
        return "claude-3-5-haiku-20241022".to_owned();
    }
    if model_lower.contains("glm") {
        return "glm-4-flash".to_owned();
    }
    if model_lower.contains("gpt") {
        return "gpt-4o-mini".to_owned();
    }
    if model_lower.contains("qwen") {
        return "qwen-plus".to_owned();
    }
    if model_lower.contains("deepseek") {
        return "deepseek-chat".to_owned();
    }

    // Fallback: use the current model.
    current.to_owned()
}

// ---------------------------------------------------------------------------

/// Truncate a string for display in error messages.
///
/// Delegates to [`crate::truncate`] to avoid duplication.
fn truncate(value: &str) -> String {
    crate::truncate(value)
}

// ---------------------------------------------------------------------------
// Conversion helpers
// ---------------------------------------------------------------------------

impl From<QueryResult> for ProviderResponse {
    fn from(result: QueryResult) -> Self {
        let text = result
            .content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");

        let thinking = result
            .content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Thinking { thinking } => Some(thinking.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");

        let tool_calls = result
            .content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::ToolUse { id, name, input } => Some(claude_core::ToolCall {
                    id: id.clone(),
                    name: name.clone(),
                    input: input.clone(),
                }),
                ContentBlock::ServerToolUse { id, name, input } => Some(claude_core::ToolCall {
                    id: id.clone(),
                    name: name.clone(),
                    input: input.clone(),
                }),
                _ => None,
            })
            .collect();

        let content_blocks: Vec<Value> = result
            .content
            .iter()
            .map(|block| serde_json::to_value(block).unwrap_or_default())
            .collect();

        ProviderResponse {
            text,
            history_text: None,
            thinking: if thinking.is_empty() {
                None
            } else {
                Some(thinking)
            },
            content_blocks,
            tool_calls,
            request_id: result.request_id,
            usage: UsageSummary {
                input_tokens: result.usage.input_tokens,
                output_tokens: result.usage.output_tokens,
                cache_read_input_tokens: result.usage.cache_read_input_tokens,
                cache_creation_input_tokens: result.usage.cache_creation_input_tokens,
                server_tool_use_web_search_requests: result
                    .usage
                    .server_tool_use_web_search_requests,
                server_tool_use_web_fetch_requests: result.usage.server_tool_use_web_fetch_requests,
                cache_creation_ephemeral_5m_input_tokens: result
                    .usage
                    .cache_creation_ephemeral_5m_input_tokens,
                cache_creation_ephemeral_1h_input_tokens: result
                    .usage
                    .cache_creation_ephemeral_1h_input_tokens,
            },
            stop_reason: result.stop_reason.unwrap_or_else(|| "end_turn".to_owned()),
            research: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_usage_preserves_nonzero_input() {
        let current = UsageStats {
            input_tokens: 100,
            output_tokens: 50,
            cache_creation_input_tokens: 10,
            cache_read_input_tokens: 20,
            ..Default::default()
        };
        let partial = UsageStats {
            input_tokens: 0, // Should NOT overwrite.
            output_tokens: 60,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
            ..Default::default()
        };
        let updated = update_usage(&current, &partial);
        assert_eq!(updated.input_tokens, 100); // Preserved.
        assert_eq!(updated.output_tokens, 60); // Updated.
    }

    #[test]
    fn accumulate_usage_sums_correctly() {
        let total = UsageStats {
            input_tokens: 100,
            output_tokens: 50,
            cache_creation_input_tokens: 10,
            cache_read_input_tokens: 20,
            ..Default::default()
        };
        let message = UsageStats {
            input_tokens: 200,
            output_tokens: 30,
            cache_creation_input_tokens: 5,
            cache_read_input_tokens: 15,
            ..Default::default()
        };
        let accumulated = accumulate_usage(&total, &message);
        assert_eq!(accumulated.input_tokens, 300);
        assert_eq!(accumulated.output_tokens, 80);
        assert_eq!(accumulated.cache_creation_input_tokens, 15);
        assert_eq!(accumulated.cache_read_input_tokens, 35);
    }

    #[test]
    fn parse_query_result_handles_anthropic_response() {
        let raw = r#"{
            "id": "msg_test123",
            "type": "message",
            "role": "assistant",
            "content": [
                {"type": "text", "text": "Hello!"},
                {"type": "tool_use", "id": "tool_1", "name": "read_file", "input": {"path": "/tmp/test.rs"}}
            ],
            "model": "claude-sonnet-4-20250514",
            "stop_reason": "tool_use",
            "usage": {
                "input_tokens": 100,
                "output_tokens": 50,
                "cache_creation_input_tokens": 10,
                "cache_read_input_tokens": 20
            }
        }"#;

        let result = parse_query_result(raw).expect("should parse");
        assert_eq!(result.content.len(), 2);
        assert_eq!(result.model, "claude-sonnet-4-20250514");
        assert_eq!(result.stop_reason.as_deref(), Some("tool_use"));
        assert_eq!(result.request_id.as_deref(), Some("msg_test123"));
        assert_eq!(result.usage.input_tokens, 100);
        assert_eq!(result.usage.output_tokens, 50);
    }

    #[test]
    fn parse_query_result_handles_thinking() {
        let raw = r#"{
            "id": "msg_456",
            "content": [
                {"type": "thinking", "thinking": "Let me analyze this..."},
                {"type": "text", "text": "Here is my answer."}
            ],
            "model": "claude-sonnet-4",
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 50, "output_tokens": 25}
        }"#;

        let result = parse_query_result(raw).expect("should parse");
        assert_eq!(result.content.len(), 2);
        assert!(matches!(result.content[0], ContentBlock::Thinking { .. }));
        assert!(matches!(result.content[1], ContentBlock::Text { .. }));
    }

    #[test]
    fn query_result_converts_to_provider_response() {
        let query_result = QueryResult {
            content: vec![
                ContentBlock::Text {
                    text: "Hello".to_owned(),
                },
                ContentBlock::ToolUse {
                    id: "tool_1".to_owned(),
                    name: "bash".to_owned(),
                    input: json!({"command": "ls"}),
                },
            ],
            usage: UsageStats {
                input_tokens: 100,
                output_tokens: 50,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 0,
                ..Default::default()
            },
            model: "test-model".to_owned(),
            stop_reason: Some("tool_use".to_owned()),
            request_id: Some("req_123".to_owned()),
        };

        let provider_response: ProviderResponse = query_result.into();
        assert_eq!(provider_response.text, "Hello");
        assert_eq!(provider_response.tool_calls.len(), 1);
        assert_eq!(provider_response.tool_calls[0].name, "bash");
        assert_eq!(provider_response.usage.input_tokens, 100);
        assert_eq!(provider_response.request_id.as_deref(), Some("req_123"));
    }

    #[test]
    fn resolve_small_model_maps_correctly() {
        let provider = claude_config::ProviderConfig {
            model: Some("claude-sonnet-4-20250514".to_owned()),
            ..test_provider_config()
        };
        assert_eq!(resolve_small_model(&provider), "claude-haiku-4-20250514");

        let provider = claude_config::ProviderConfig {
            model: Some("glm-5".to_owned()),
            ..test_provider_config()
        };
        assert_eq!(resolve_small_model(&provider), "glm-4-flash");

        let provider = claude_config::ProviderConfig {
            model: Some("gpt-4o".to_owned()),
            ..test_provider_config()
        };
        assert_eq!(resolve_small_model(&provider), "gpt-4o-mini");
    }

    fn test_provider_config() -> claude_config::ProviderConfig {
        claude_config::ProviderConfig {
            name: "test".to_owned(),
            base_url: Some("https://api.anthropic.com/v1/messages".to_owned()),
            api_key: Some("test-key".to_owned()),
            model: Some("test-model".to_owned()),
            protocol: ProviderProtocol::Anthropic,
            timeout_ms: 30_000,
            max_output_tokens: 4096,
            max_retries: 3,
            retry_initial_backoff_ms: 100,
            retry_max_backoff_ms: 1000,
            respect_retry_after: true,
            request_header_overrides: Default::default(),
            request_metadata: Default::default(),
            thinking_budget: None,
            temperature: None,
            top_p: None,
            top_k: None,
        }
    }
}
