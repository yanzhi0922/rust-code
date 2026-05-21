//! LLM provider client with retry logic and message formatting.
#![allow(clippy::doc_lazy_continuation, clippy::option_map_unit_fn)]
//!
//! Supports OpenAI, Anthropic, Amazon Bedrock, and Google Vertex AI protocols.
//! Handles message conversion, response parsing, exponential back-off retries,
//! and mock-mode responses for testing.
//!
//! # Error classification
//!
//! The [`ProviderError`] enum provides structured error classification matching
//! upstream Claude Code's `categorizeRetryableAPIError`. Each variant carries
//! enough context for the caller to decide whether to retry, compact, or abort.

pub mod advanced_api;
pub mod agent_types;
pub mod api_client;
pub mod attribution;
pub mod beta_headers;
pub mod cache_headers;
pub mod circuit_breaker;
pub mod context;
pub mod conversation_backend;
pub mod cost;
pub mod credential_pool;
pub mod effort_params;
pub mod failover;
pub mod fingerprint;
pub mod max_tokens;
pub mod mcp_api;
pub mod media;
pub mod model_info;
pub mod normalize;
pub mod query_source;
pub mod retry;
pub mod server_tool_use;
pub mod sigv4;
pub mod streaming;
pub mod thinking_blocks;
pub mod workload;

pub use api_client::{ApiClient, ContentBlock, QueryOptions, QueryResult, UsageStats};
pub use circuit_breaker::{CircuitBreaker, CircuitBreakerConfig, CircuitState};
pub use context::{TokenEstimator, dual_ratio_estimate};
pub use conversation_backend::{ConversationBackend, DiscoveredToolScope, ProviderCompatBackend};
pub use retry::{
    ApiErrorKind, FastModeState, OAuthRefreshCallback, ResponseHints, RetryConfig, RetryContext,
    RetryOptions, SubscriberTier, classify_api_error, get_subscriber_tier,
    is_enterprise_subscriber, is_fast_mode_enabled, is_persistent_retry_enabled, is_subscriber,
    should_retry_529, with_retry_ext,
};
pub use streaming::{StreamingCallbacks, StreamingLifecycleEvent};

use crate::model_info::get_model_info;
use crate::retry::{
    get_rate_limit_wait_duration, is_overloaded_error_body, is_retryable_http_status,
    is_retryable_transport_error, should_retry_from_header,
};
use anyhow::{Context, Result, anyhow};
use claude_config::ProviderConfig;
use claude_core::{
    ConversationEntry, ConversationRole, ProviderProtocol, ProviderResponse, ToolCall, UsageSummary,
};
use claude_tools::{
    ToolSpec, runtime_provider_tool_specs,
    runtime_visible_provider_tool_specs_with_discovered_tools,
};
use parking_lot::Mutex;
use reqwest::Client;
use reqwest::header::{
    ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue, USER_AGENT,
};
use serde_json::{Value, json};
use std::borrow::Cow;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::env;
use std::time::Duration;
use uuid::Uuid;

const DEFAULT_AUTO_TOOL_SEARCH_PERCENTAGE: u64 = 10;
const TOOL_SEARCH_CHARS_PER_TOKEN: f64 = 2.5;
const TOOL_REFERENCE_TURN_BOUNDARY: &str = "Tool loaded.";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ToolSearchMode {
    Tst,
    TstAuto,
    Standard,
}

/// HTTP client for communicating with LLM provider APIs.
///
/// Includes an optional circuit breaker per provider name to prevent wasting
/// time on providers that are known to be down, and an optional credential
/// pool for round-robin API key rotation.
pub struct ProviderClient {
    http: Client,
    /// Circuit breakers keyed by provider name.
    breakers: Mutex<HashMap<String, CircuitBreaker>>,
    /// Optional credential pool for round-robin API key rotation.
    credential_pool: Option<credential_pool::CredentialPool>,
}

impl ProviderClient {
    /// Create a new provider client.
    ///
    /// # Errors
    /// Returns an error if the underlying HTTP client cannot be constructed.
    pub fn new() -> Result<Self> {
        let http = Client::builder()
            .build()
            .context("failed to build the provider HTTP client")?;
        Ok(Self {
            http,
            breakers: Mutex::new(HashMap::new()),
            credential_pool: None,
        })
    }

    /// Create a new provider client with a credential pool for API key rotation.
    ///
    /// When a credential pool is set, each request will use the next credential
    /// in the round-robin rotation, overriding the API key from the provider config.
    pub fn with_credential_pool(http: Client, pool: credential_pool::CredentialPool) -> Self {
        Self {
            http,
            breakers: Mutex::new(HashMap::new()),
            credential_pool: Some(pool),
        }
    }

    /// Set the credential pool for API key rotation.
    pub fn set_credential_pool(&mut self, pool: credential_pool::CredentialPool) {
        self.credential_pool = Some(pool);
    }

    /// Resolve the effective API key for a request.
    ///
    /// If a credential pool is available, uses round-robin rotation.
    /// Otherwise, falls back to the provider config's API key.
    fn resolve_api_key(&self, provider: &ProviderConfig) -> Option<String> {
        if let Some(ref pool) = self.credential_pool
            && let Some(cred) = pool.next()
        {
            return Some(cred.api_key.clone());
        }
        provider.api_key.clone()
    }

    /// Get the circuit breaker configuration for the given provider name.
    ///
    /// Currently returns the default configuration for all providers.
    /// Per-provider configuration can be added by looking up the provider
    /// name in a configuration map.
    fn breaker_config_for(_provider_name: &str) -> CircuitBreakerConfig {
        CircuitBreakerConfig::default()
    }

    /// Check the circuit breaker for the given provider.
    ///
    /// Returns `Ok(())` if requests are allowed, or an error describing
    /// why the request was rejected.
    fn check_circuit(&self, provider_name: &str) -> Result<()> {
        let breakers = self.breakers.lock();
        if let Some(breaker) = breakers.get(provider_name) {
            breaker.allow_request().map_err(|state| {
                anyhow!("provider {provider_name} circuit breaker is {state:?} — skipping request")
            })?;
        }
        Ok(())
    }

    /// Record a successful provider call in the circuit breaker.
    fn record_success(&self, provider_name: &str) {
        let mut breakers = self.breakers.lock();
        if let Some(breaker) = breakers.get_mut(provider_name) {
            breaker.record_success();
        }
    }

    /// Record a failed provider call in the circuit breaker.
    ///
    /// Lazily creates a breaker for the provider if one does not yet exist.
    fn record_failure(&self, provider_name: &str) {
        let mut breakers = self.breakers.lock();
        use std::collections::hash_map::Entry;
        match breakers.entry(provider_name.to_owned()) {
            Entry::Occupied(e) => e.into_mut().record_failure(),
            Entry::Vacant(e) => {
                let config = Self::breaker_config_for(provider_name);
                let breaker = CircuitBreaker::new(config);
                breaker.record_failure();
                e.insert(breaker);
            }
        }
    }

    /// Send a completion request to the configured provider.
    ///
    /// Automatically selects the correct protocol (OpenAI / Anthropic) based on
    /// the provider configuration and retries on transient failures.
    ///
    /// # Errors
    /// Returns an error if the API request fails after all retries are exhausted.
    pub async fn complete(
        &self,
        provider: &ProviderConfig,
        conversation: &[ConversationEntry],
    ) -> Result<ProviderResponse> {
        self.complete_with_discovered_tools(provider, conversation, &BTreeSet::new(), None)
            .await
    }

    /// Send a completion request while preserving deferred-tool discovery state
    /// carried forward from compact boundaries.
    ///
    /// # Errors
    /// Returns an error if the API request fails after all retries are exhausted.
    pub async fn complete_with_discovered_tools(
        &self,
        provider: &ProviderConfig,
        conversation: &[ConversationEntry],
        carried_discovered_tools: &BTreeSet<String>,
        request_context: Option<&query_source::ProviderRequestContext>,
    ) -> Result<ProviderResponse> {
        if provider.name == "mock"
            || provider.api_key.as_deref() == Some("mock")
            || provider.base_url.as_deref() == Some("mock://provider")
        {
            return Ok(mock_response(conversation));
        }

        // Check circuit breaker before making the request.
        self.check_circuit(&provider.name)?;

        // Resolve API key: use credential pool rotation if available.
        let effective_provider = if self.credential_pool.is_some() {
            let mut p = provider.clone();
            p.api_key = self.resolve_api_key(provider);
            p
        } else {
            provider.clone()
        };

        let result = if provider_prefers_anthropic_messages_route(&effective_provider) {
            let routed_provider = provider_as_anthropic_compatible(&effective_provider);
            self.complete_anthropic(
                &routed_provider,
                conversation,
                carried_discovered_tools,
                request_context,
            )
            .await
        } else {
            match effective_provider.protocol {
                ProviderProtocol::OpenAi => {
                    self.complete_openai(
                        &effective_provider,
                        conversation,
                        carried_discovered_tools,
                        request_context,
                    )
                    .await
                }
                ProviderProtocol::Anthropic => {
                    self.complete_anthropic(
                        &effective_provider,
                        conversation,
                        carried_discovered_tools,
                        request_context,
                    )
                    .await
                }
                ProviderProtocol::Bedrock => {
                    self.complete_bedrock(
                        &effective_provider,
                        conversation,
                        carried_discovered_tools,
                        request_context,
                    )
                    .await
                }
                ProviderProtocol::Vertex => {
                    self.complete_vertex(
                        &effective_provider,
                        conversation,
                        carried_discovered_tools,
                        request_context,
                    )
                    .await
                }
            }
        };

        match &result {
            Ok(_) => self.record_success(&provider.name),
            Err(_) => self.record_failure(&provider.name),
        }
        result
    }

    /// Complete a conversation with automatic context compaction on context_length_exceeded errors.
    ///
    /// This implements the "reactiveCompact" pattern: if the API returns a 400 error
    /// indicating the context is too long, the conversation is automatically compacted
    /// and the request is retried (up to `max_retries` times).
    pub async fn complete_with_auto_compact(
        &self,
        provider: &ProviderConfig,
        conversation: &[ConversationEntry],
        context_manager: &context::ContextWindowManager,
    ) -> Result<ProviderResponse> {
        let mut current = conversation.to_vec();
        let max_retries = 3;

        for attempt in 0..=max_retries {
            match self.complete(provider, &current).await {
                Ok(response) => return Ok(response),
                Err(error) => {
                    let error_str = error.to_string().to_ascii_lowercase();
                    let is_context_too_long = error_str.contains("context_length_exceeded")
                        || error_str.contains("prompt_too_long")
                        || error_str.contains("too many tokens")
                        || error_str.contains("maximum context length")
                        || error_str.contains("reduce the length");

                    if !is_context_too_long || attempt >= max_retries {
                        return Err(error);
                    }

                    // Try to compact the conversation.
                    match context_manager.compact_on_error(&current) {
                        Some(compacted) => {
                            current = compacted;
                        }
                        None => {
                            return Err(error);
                        }
                    }
                }
            }
        }

        // Should not reach here, but just in case.
        self.complete(provider, &current).await
    }

    async fn complete_openai(
        &self,
        provider: &ProviderConfig,
        conversation: &[ConversationEntry],
        carried_discovered_tools: &BTreeSet<String>,
        request_context: Option<&query_source::ProviderRequestContext>,
    ) -> Result<ProviderResponse> {
        let effective_provider = provider_for_request(provider, request_context);
        let body = build_openai_request_body(
            &effective_provider,
            conversation,
            carried_discovered_tools,
            false,
        )
        .await;
        let base_url = provider
            .base_url
            .as_ref()
            .ok_or_else(|| anyhow!("provider is missing a normalized base URL"))?;
        let response = self
            .send_json_request(
                &effective_provider,
                base_url,
                &body,
                "openai-compatible",
                request_context,
            )
            .await?;
        parse_openai_response(response.0, response.1)
    }

    async fn complete_anthropic(
        &self,
        provider: &ProviderConfig,
        conversation: &[ConversationEntry],
        carried_discovered_tools: &BTreeSet<String>,
        request_context: Option<&query_source::ProviderRequestContext>,
    ) -> Result<ProviderResponse> {
        let effective_provider = provider_for_request(provider, request_context);
        let body = build_anthropic_request_body(
            &effective_provider,
            conversation,
            carried_discovered_tools,
            request_context,
            false,
        )
        .await;
        let base_url = provider
            .base_url
            .as_ref()
            .ok_or_else(|| anyhow!("provider is missing a normalized base URL"))?;
        let response = self
            .send_json_request(
                &effective_provider,
                base_url,
                &body,
                "anthropic-compatible",
                request_context,
            )
            .await?;
        parse_anthropic_response(response.0, response.1)
    }

    /// Send a completion request to Amazon Bedrock using native SigV4 signing.
    ///
    /// If AWS credentials (`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`) are not
    /// available, falls back to the OpenAI-compatible path (useful for Bedrock
    /// proxies like LiteLLM).
    ///
    /// Bedrock Claude models use the Anthropic Messages API format, so the
    /// response is parsed with the Anthropic response parser.
    async fn complete_bedrock(
        &self,
        provider: &ProviderConfig,
        conversation: &[ConversationEntry],
        carried_discovered_tools: &BTreeSet<String>,
        request_context: Option<&query_source::ProviderRequestContext>,
    ) -> Result<ProviderResponse> {
        let effective_provider = provider_for_request(provider, request_context);
        let credentials = match sigv4::load_aws_credentials() {
            Some(creds) => creds,
            None => {
                // No AWS credentials — fall back to OpenAI-compatible proxy mode.
                return self
                    .complete_openai(
                        provider,
                        conversation,
                        carried_discovered_tools,
                        request_context,
                    )
                    .await;
            }
        };

        let model = effective_provider
            .model
            .as_deref()
            .ok_or_else(|| anyhow!("Bedrock provider requires a model ID (e.g. anthropic.claude-sonnet-4-20250514-v1:0)"))?;

        // Build Anthropic-format body for Claude models on Bedrock.
        let (system, messages, tools) = prepare_anthropic_request_surface(
            &effective_provider,
            conversation,
            carried_discovered_tools,
        )
        .await;
        let mut body = json!({
            "anthropic_version": "bedrock-2023-05-31",
            "system": system,
            "messages": messages,
            "tools": tools,
            "max_tokens": effective_provider.max_output_tokens,
        });
        apply_anthropic_request_metadata(&mut body, &effective_provider, request_context);
        if body_uses_tool_search_features(Some(&body)) {
            merge_anthropic_beta_body_param(&mut body, beta_headers::TOOL_SEARCH_BETA_3P);
        }
        let payload =
            serde_json::to_vec(&body).context("failed to serialise Bedrock request body")?;

        // Construct Bedrock InvokeModel URL.
        let host = format!("bedrock-runtime.{}.amazonaws.com", credentials.region);
        let encoded_model = model.replace(':', "%3A").replace('+', "%2B");
        let path = format!("/model/{encoded_model}/invoke");
        let url = format!("https://{host}{path}");

        let (status, text) = self
            .send_bedrock_request(
                &url,
                &host,
                &path,
                &payload,
                &effective_provider,
                &credentials,
            )
            .await?;

        // Bedrock returns Anthropic-format responses for Claude models.
        parse_anthropic_response(status, text)
    }

    /// Send a signed Bedrock request with retry logic.
    ///
    /// Each retry attempt re-signs the request because the `X-Amz-Date` timestamp
    /// changes.
    async fn send_bedrock_request(
        &self,
        url: &str,
        host: &str,
        path: &str,
        payload: &[u8],
        provider: &ProviderConfig,
        credentials: &sigv4::AwsCredentials,
    ) -> Result<(u16, String)> {
        let mut attempt = 0u32;
        loop {
            // Sign the request (must be done per-attempt for fresh timestamp).
            let signed = sigv4::sign("POST", host, path, payload, credentials, "bedrock");

            let mut headers = HeaderMap::new();
            headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
            headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
            headers.insert(
                HeaderName::from_static("host"),
                HeaderValue::from_str(&signed.host)?,
            );
            headers.insert(
                HeaderName::from_static("x-amz-date"),
                HeaderValue::from_str(&signed.x_amz_date)?,
            );
            headers.insert(
                HeaderName::from_static("x-amz-content-sha256"),
                HeaderValue::from_str(&signed.x_amz_content_sha256)?,
            );
            headers.insert(AUTHORIZATION, HeaderValue::from_str(&signed.authorization)?);
            if let Some(ref token) = signed.x_amz_security_token {
                headers.insert(
                    HeaderName::from_static("x-amz-security-token"),
                    HeaderValue::from_str(token)?,
                );
            }

            let response = self
                .http
                .post(url)
                .headers(headers)
                .timeout(Duration::from_millis(provider.timeout_ms))
                .body(payload.to_vec())
                .send()
                .await;

            match response {
                Ok(resp) => {
                    let status = resp.status().as_u16();
                    let retry_after = parse_retry_after(resp.headers(), provider);
                    let text = resp
                        .text()
                        .await
                        .context("failed to read Bedrock response body")?;
                    if is_retryable_http_status(status) && attempt < provider.max_retries {
                        tokio::time::sleep(compute_retry_delay(provider, attempt, retry_after))
                            .await;
                        attempt += 1;
                        continue;
                    }
                    return Ok((status, text));
                }
                Err(error) => {
                    if is_retryable_transport_error(&error) && attempt < provider.max_retries {
                        tokio::time::sleep(compute_retry_delay(provider, attempt, None)).await;
                        attempt += 1;
                        continue;
                    }
                    return Err(error).context("Bedrock request failed");
                }
            }
        }
    }

    /// Send a completion request to Google Vertex AI using OAuth2 Bearer auth.
    ///
    /// If Google credentials are not available, falls back to the OpenAI-compatible
    /// path (useful for Vertex AI proxies).
    ///
    /// Vertex AI Claude models use the Anthropic Messages API format, so the
    /// response is parsed with the Anthropic response parser.
    async fn complete_vertex(
        &self,
        provider: &ProviderConfig,
        conversation: &[ConversationEntry],
        carried_discovered_tools: &BTreeSet<String>,
        request_context: Option<&query_source::ProviderRequestContext>,
    ) -> Result<ProviderResponse> {
        let effective_provider = provider_for_request(provider, request_context);
        let access_token = match load_vertex_access_token() {
            Some(token) => token,
            None => {
                // No Google credentials — fall back to OpenAI-compatible proxy mode.
                return self
                    .complete_openai(
                        provider,
                        conversation,
                        carried_discovered_tools,
                        request_context,
                    )
                    .await;
            }
        };

        let model = effective_provider.model.as_deref().ok_or_else(|| {
            anyhow!("Vertex AI provider requires a model ID (e.g. claude-sonnet-4@20250514)")
        })?;

        let project = std::env::var("GOOGLE_CLOUD_PROJECT")
            .or_else(|_| std::env::var("GCLOUD_PROJECT"))
            .map_err(|_| {
                anyhow!("Vertex AI requires GOOGLE_CLOUD_PROJECT or GCLOUD_PROJECT env var")
            })?;

        let region = std::env::var("GOOGLE_CLOUD_REGION")
            .or_else(|_| std::env::var("CLOUD_ML_REGION"))
            .unwrap_or_else(|_| "us-east5".to_string());

        // Build Anthropic-format body for Claude models on Vertex AI.
        let (system, messages, tools) = prepare_anthropic_request_surface(
            &effective_provider,
            conversation,
            carried_discovered_tools,
        )
        .await;
        let mut body = json!({
            "anthropic_version": "vertex-2023-10-16",
            "system": system,
            "messages": messages,
            "tools": tools,
            "max_tokens": effective_provider.max_output_tokens,
        });
        apply_anthropic_request_metadata(&mut body, &effective_provider, request_context);
        if body_uses_tool_search_features(Some(&body)) {
            merge_anthropic_beta_body_param(&mut body, beta_headers::TOOL_SEARCH_BETA_3P);
        }

        // Construct Vertex AI URL.
        let url = format!(
            "https://{region}-aiplatform.googleapis.com/v1/projects/{project}/locations/{region}/publishers/anthropic/models/{model}:invokeModel"
        );

        let (status, text) = self
            .send_vertex_request(&url, &access_token, &body, &effective_provider)
            .await?;

        // Vertex AI returns Anthropic-format responses for Claude models.
        parse_anthropic_response(status, text)
    }

    /// Send a Vertex AI request with Bearer token auth and retry logic.
    async fn send_vertex_request(
        &self,
        url: &str,
        access_token: &str,
        body: &Value,
        provider: &ProviderConfig,
    ) -> Result<(u16, String)> {
        let mut attempt = 0u32;
        loop {
            let mut headers = HeaderMap::new();
            headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
            headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {access_token}"))?,
            );
            headers.insert(
                USER_AGENT,
                HeaderValue::from_str(&format!("remote-code-rust/{}", env!("CARGO_PKG_VERSION")))?,
            );

            let response = self
                .http
                .post(url)
                .headers(headers)
                .timeout(Duration::from_millis(provider.timeout_ms))
                .json(body)
                .send()
                .await;

            match response {
                Ok(resp) => {
                    let status = resp.status().as_u16();
                    let retry_after = parse_retry_after(resp.headers(), provider);
                    let text = resp
                        .text()
                        .await
                        .context("failed to read Vertex AI response body")?;
                    if is_retryable_http_status(status) && attempt < provider.max_retries {
                        tokio::time::sleep(compute_retry_delay(provider, attempt, retry_after))
                            .await;
                        attempt += 1;
                        continue;
                    }
                    return Ok((status, text));
                }
                Err(error) => {
                    if is_retryable_transport_error(&error) && attempt < provider.max_retries {
                        tokio::time::sleep(compute_retry_delay(provider, attempt, None)).await;
                        attempt += 1;
                        continue;
                    }
                    return Err(error).context("Vertex AI request failed");
                }
            }
        }
    }

    async fn send_json_request(
        &self,
        provider: &ProviderConfig,
        base_url: &str,
        body: &Value,
        label: &str,
        request_context: Option<&query_source::ProviderRequestContext>,
    ) -> Result<(u16, String)> {
        let mut attempt = 0u32;
        loop {
            maybe_dump_request_body(label, body);
            let response = self
                .http
                .post(base_url)
                .headers(build_headers(provider, Some(body), request_context)?)
                .timeout(Duration::from_millis(provider.timeout_ms))
                .json(body)
                .send()
                .await;
            match response {
                Ok(response) => {
                    let status = response.status().as_u16();
                    let headers = response.headers().clone();
                    let retry_after = parse_retry_after(&headers, provider);
                    let text = response
                        .text()
                        .await
                        .with_context(|| format!("failed to read {label} response body"))?;

                    // Check x-should-retry header: if explicitly "false", don't
                    // retry (unless overloaded_error in body for 5xx).
                    if let Some(false) = should_retry_from_header(&headers) {
                        let is_5xx = status >= 500;
                        let overloaded_body = is_overloaded_error_body(text.as_bytes());
                        let is_ant = std::env::var("USER_TYPE").as_deref() == Ok("ant");
                        if !(is_ant && is_5xx) && !overloaded_body {
                            return Ok((status, text));
                        }
                    }

                    // Check if retryable via status code or overloaded body fallback.
                    let is_retryable = is_retryable_http_status(status)
                        || is_overloaded_error_body(text.as_bytes())
                        || should_retry_from_header(&headers) == Some(true);

                    if is_retryable && attempt < provider.max_retries {
                        // For 429, prefer the rate-limit reset header over
                        // exponential back-off.
                        let effective_retry_after = if status == 429 {
                            get_rate_limit_wait_duration(&headers).or(retry_after)
                        } else {
                            retry_after
                        };
                        tokio::time::sleep(compute_retry_delay(
                            provider,
                            attempt,
                            effective_retry_after,
                        ))
                        .await;
                        attempt += 1;
                        continue;
                    }
                    return Ok((status, text));
                }
                Err(error) => {
                    if is_retryable_transport_error(&error) && attempt < provider.max_retries {
                        tokio::time::sleep(compute_retry_delay(provider, attempt, None)).await;
                        attempt += 1;
                        continue;
                    }
                    return Err(error).with_context(|| format!("{label} request failed"));
                }
            }
        }
    }
}

pub(crate) fn maybe_dump_request_body(label: &str, body: &Value) {
    let Ok(dir) = std::env::var("REMOTE_CODE_DUMP_PROVIDER_REQUEST_DIR") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let _ = std::fs::create_dir_all(&dir);
    let timestamp = chrono::Utc::now().format("%Y%m%dT%H%M%S%.3fZ");
    let path = dir.join(format!("{timestamp}-{label}.json"));
    if let Ok(bytes) = serde_json::to_vec_pretty(body) {
        let _ = std::fs::write(path, bytes);
    }
}

/// Build the effective [`ProviderConfig`] for a request, applying any overrides
/// from the [`query_source::ProviderRequestContext`].
///
/// Returns [`Cow::Borrowed`] when no overrides are present (avoids cloning the
/// entire config), and [`Cow::Owned`] only when the config actually needs to be
/// modified.
fn provider_for_request<'a>(
    provider: &'a ProviderConfig,
    request_context: Option<&query_source::ProviderRequestContext>,
) -> Cow<'a, ProviderConfig> {
    let Some(context) = request_context else {
        return Cow::Borrowed(provider);
    };
    if context.model_override.is_none() && context.max_output_tokens.is_none() {
        return Cow::Borrowed(provider);
    }

    let mut effective = provider.clone();
    if let Some(model) = context.model_override.as_ref() {
        effective.model = Some(model.clone());
        // Strip thinking config when falling back to a model that doesn't support it.
        // TS reference: when the target model is not Claude-family, the thinking
        // budget and extended-thinking beta headers must be removed from the request.
        let model_lower = model.to_ascii_lowercase();
        if !model_lower.contains("claude") {
            effective.thinking_budget = None;
        }
    }
    if let Some(max_output_tokens) = context.max_output_tokens {
        effective.max_output_tokens = max_output_tokens;
    }
    Cow::Owned(effective)
}

fn provider_prefers_anthropic_messages_route(provider: &ProviderConfig) -> bool {
    provider.protocol != ProviderProtocol::Anthropic
        && provider
            .base_url
            .as_deref()
            .is_some_and(base_url_looks_anthropic_messages_api)
}

fn base_url_looks_anthropic_messages_api(base_url: &str) -> bool {
    let normalized = base_url.trim().to_ascii_lowercase();
    normalized.ends_with("/messages")
        || normalized.contains("/anthropic/")
        || normalized.ends_with("/anthropic")
        || normalized.contains("compat=anthropic")
}

fn provider_as_anthropic_compatible(provider: &ProviderConfig) -> ProviderConfig {
    let mut routed = provider.clone();
    routed.protocol = ProviderProtocol::Anthropic;
    routed
}

async fn build_openai_request_body(
    provider: &ProviderConfig,
    conversation: &[ConversationEntry],
    carried_discovered_tools: &BTreeSet<String>,
    stream: bool,
) -> Value {
    let model_name = provider.model.as_deref().unwrap_or("");
    let is_reasoning_model = model_name.starts_with("o1")
        || model_name.starts_with("o3")
        || model_name.starts_with("o4");
    let tools = current_openai_tool_schemas(provider, conversation, carried_discovered_tools).await;

    let default_temperature = 0.1;
    let effective_temperature = provider.temperature.unwrap_or(default_temperature);

    let mut body = if is_reasoning_model {
        // Reasoning models (o1/o3/o4-mini) do not support temperature
        // and use max_completion_tokens instead of max_tokens.
        json!({
            "model": provider.model,
            "messages": to_openai_messages(conversation),
            "tools": tools,
            "tool_choice": "auto",
            "max_completion_tokens": provider.max_output_tokens,
            "stream": stream,
        })
    } else {
        json!({
            "model": provider.model,
            "messages": to_openai_messages(conversation),
            "tools": tools,
            "tool_choice": "auto",
            "temperature": effective_temperature,
            "max_tokens": provider.max_output_tokens,
            "stream": stream,
        })
    };

    // Pass through top_p / top_k if configured.
    if let Some(top_p) = provider.top_p {
        body["top_p"] = json!(top_p);
    }
    // OpenAI does not support top_k directly, but some compatible providers do.
    if let Some(top_k) = provider.top_k {
        body["top_k"] = json!(top_k);
    }

    // If thinking_budget is set and the model supports it, add reasoning_effort.
    if is_reasoning_model && let Some(budget) = provider.thinking_budget {
        // Map budget to reasoning_effort: low/medium/high.
        let effort = if budget <= 5000 {
            "low"
        } else if budget <= 20000 {
            "medium"
        } else {
            "high"
        };
        body["reasoning_effort"] = json!(effort);
    }
    body
}

fn compute_retry_delay(
    provider: &ProviderConfig,
    attempt: u32,
    retry_after: Option<Duration>,
) -> Duration {
    let config = RetryConfig {
        max_retries: provider.max_retries,
        base_delay_ms: provider.retry_initial_backoff_ms,
        max_backoff_ms: provider.retry_max_backoff_ms,
        respect_retry_after: provider.respect_retry_after,
        ..RetryConfig::default()
    };
    crate::retry::compute_retry_delay(&config, attempt, retry_after)
}

fn parse_retry_after(headers: &HeaderMap, provider: &ProviderConfig) -> Option<Duration> {
    crate::retry::parse_retry_after(headers, provider.respect_retry_after)
}

/// Load a Google Cloud OAuth2 access token for Vertex AI.
///
/// Tries, in order:
/// 1. `GOOGLE_ACCESS_TOKEN` environment variable (direct token).
/// 2. `gcloud auth print-access-token` CLI command.
///
/// Returns `None` if neither source yields a token.
fn load_vertex_access_token() -> Option<String> {
    // 1. Direct token from environment.
    if let Ok(token) = std::env::var("GOOGLE_ACCESS_TOKEN")
        && !token.is_empty()
    {
        return Some(token);
    }

    // 2. Try gcloud CLI.
    let output = std::process::Command::new("gcloud")
        .args(["auth", "print-access-token"])
        .output()
        .ok()?;

    if output.status.success() {
        let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !token.is_empty() {
            return Some(token);
        }
    }

    None
}

fn apply_request_context_metadata(
    metadata: &mut serde_json::Map<String, Value>,
    request_context: Option<&query_source::ProviderRequestContext>,
) {
    let Some(request_context) = request_context else {
        return;
    };

    metadata
        .entry("session_id".to_owned())
        .or_insert_with(|| json!(request_context.session_id.as_str()));
}

pub(crate) fn apply_anthropic_request_metadata(
    body: &mut Value,
    provider: &ProviderConfig,
    request_context: Option<&query_source::ProviderRequestContext>,
) {
    if provider.request_metadata.is_empty() && request_context.is_none() {
        return;
    }
    let mut metadata = serde_json::Map::new();
    for (key, value) in &provider.request_metadata {
        metadata.insert(key.clone(), json!(value));
    }
    apply_request_context_metadata(&mut metadata, request_context);
    let user_id = serde_json::to_string(&metadata).unwrap_or_else(|_| "{}".to_owned());
    body["metadata"] = json!({
        "user_id": user_id,
    });
}

fn build_headers(
    provider: &ProviderConfig,
    body: Option<&Value>,
    request_context: Option<&query_source::ProviderRequestContext>,
) -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert(ACCEPT, HeaderValue::from_static("application/json"));

    if matches!(provider.protocol, ProviderProtocol::Anthropic) {
        // ── Claude Code disguise mode ──────────────────────────────────
        //
        // Coding Plan providers (智谱/阿里云/腾讯云/百度千帆) prioritise
        // requests that look like they come from Claude Code.  We mimic the
        // key identifying headers so our traffic receives the same
        // preferential treatment.
        //
        // This is the same approach used by OpenCode, OpenClaw, Cline, and
        // other open-source coding agents that consume Coding Plan quotas.

        headers.insert(
            HeaderName::from_static("x-app"),
            HeaderValue::from_static("cli"),
        );
        headers.insert(
            USER_AGENT,
            HeaderValue::from_str(&claude_code_user_agent())?,
        );
        let session_id = request_context
            .map(|context| context.session_id.as_str().to_owned())
            .or_else(|| {
                provider
                    .request_metadata
                    .get("session_id")
                    .filter(|value| !value.trim().is_empty())
                    .cloned()
            })
            .unwrap_or_else(|| "unknown".to_owned());
        headers.insert(
            HeaderName::from_static("x-claude-code-session-id"),
            HeaderValue::from_str(&session_id)?,
        );
        if let Ok(container_id) = env::var("CLAUDE_CODE_CONTAINER_ID")
            && !container_id.trim().is_empty()
        {
            headers.insert(
                HeaderName::from_static("x-claude-remote-container-id"),
                HeaderValue::from_str(container_id.trim())?,
            );
        }
        if let Ok(remote_session_id) = env::var("CLAUDE_CODE_REMOTE_SESSION_ID")
            && !remote_session_id.trim().is_empty()
        {
            headers.insert(
                HeaderName::from_static("x-claude-remote-session-id"),
                HeaderValue::from_str(remote_session_id.trim())?,
            );
        }
        if let Ok(client_app) = env::var("CLAUDE_AGENT_SDK_CLIENT_APP")
            && !client_app.trim().is_empty()
        {
            headers.insert(
                HeaderName::from_static("x-client-app"),
                HeaderValue::from_str(client_app.trim())?,
            );
        }
        if env_truthy("CLAUDE_CODE_ADDITIONAL_PROTECTION") {
            headers.insert(
                HeaderName::from_static("x-anthropic-additional-protection"),
                HeaderValue::from_static("true"),
            );
        }
        if provider_looks_first_party_anthropic(provider) {
            headers.insert(
                HeaderName::from_static("x-client-request-id"),
                HeaderValue::from_str(&Uuid::new_v4().to_string())?,
            );
        }
        headers.insert(
            HeaderName::from_static("anthropic-version"),
            HeaderValue::from_static("2023-06-01"),
        );
        let mut betas = beta_headers::DEFAULT_BETA_HEADERS
            .iter()
            .map(|value| (*value).to_owned())
            .collect::<Vec<_>>();
        if body_uses_global_prompt_cache_scope(body) {
            beta_headers::push_beta_once(&mut betas, beta_headers::PROMPT_CACHING_SCOPE_BETA);
        }
        if body_uses_tool_search_features(body) {
            beta_headers::push_beta_once(&mut betas, beta_headers::TOOL_SEARCH_BETA_1P);
        }
        if body_uses_thinking(body) {
            beta_headers::push_beta_once(&mut betas, beta_headers::INTERLEAVED_THINKING_BETA);
        }
        if body_uses_context_management(body) {
            beta_headers::push_beta_once(&mut betas, beta_headers::CONTEXT_MANAGEMENT_BETA);
        }
        if body_uses_output_config_effort(body) {
            beta_headers::push_beta_once(&mut betas, beta_headers::EFFORT_BETA);
        }
        if body_uses_output_config_task_budget(body) {
            beta_headers::push_beta_once(&mut betas, beta_headers::TASK_BUDGETS_BETA);
        }
        if body_uses_output_config_format(body) {
            beta_headers::push_beta_once(&mut betas, beta_headers::STRUCTURED_OUTPUTS_BETA);
        }
        if body_uses_fast_mode(body) {
            beta_headers::push_beta_once(&mut betas, beta_headers::FAST_MODE_BETA);
        }
        beta_headers::merge_env_anthropic_betas(&mut betas);
        headers.insert(
            HeaderName::from_static("anthropic-beta"),
            HeaderValue::from_str(&betas.join(","))?,
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

    if let Some(query_source_context) = request_context.map(request_context_to_query_source_context)
    {
        headers.insert(
            HeaderName::from_static(query_source::QUERY_SOURCE_HEADER),
            HeaderValue::from_str(&query_source::query_source_header(&query_source_context))?,
        );
    }

    // Apply user-supplied header overrides last so they can override
    // any of the defaults above (including the Claude Code disguise).
    for (name, value) in &provider.request_header_overrides {
        let header_name = HeaderName::from_bytes(name.as_bytes())
            .with_context(|| format!("invalid header name {name}"))?;
        let header_value =
            HeaderValue::from_str(value).with_context(|| format!("invalid header {name}"))?;
        headers.insert(header_name, header_value);
    }
    Ok(headers)
}

fn request_context_to_query_source_context(
    context: &query_source::ProviderRequestContext,
) -> query_source::QuerySourceContext {
    let mut query_source_context = query_source::QuerySourceContext::new(context.query_source)
        .with_session_id(context.session_id.as_str().to_owned());
    if let Some(agent_id) = context.agent_id.as_ref() {
        query_source_context = query_source_context.with_agent_id(agent_id.as_str().to_owned());
    }
    query_source_context
}

fn claude_code_user_agent() -> String {
    let user_type = env::var("USER_TYPE").unwrap_or_else(|_| "external".to_owned());
    let entrypoint = env::var("CLAUDE_CODE_ENTRYPOINT").unwrap_or_else(|_| "cli".to_owned());
    let mut parts = vec![user_type, entrypoint];
    if let Ok(version) = env::var("CLAUDE_AGENT_SDK_VERSION")
        && !version.trim().is_empty()
    {
        parts.push(format!("agent-sdk/{}", version.trim()));
    }
    if let Ok(client_app) = env::var("CLAUDE_AGENT_SDK_CLIENT_APP")
        && !client_app.trim().is_empty()
    {
        parts.push(format!("client-app/{}", client_app.trim()));
    }
    format!(
        "claude-cli/{} ({})",
        claude_config::runtime_version(),
        parts.join(", ")
    )
}

fn env_truthy(name: &str) -> bool {
    env::var(name)
        .ok()
        .as_ref()
        .is_some_and(|v| is_env_truthy(v))
}

fn provider_looks_first_party_anthropic(provider: &ProviderConfig) -> bool {
    provider
        .base_url
        .as_deref()
        .is_some_and(is_first_party_anthropic_base_url)
}

fn body_uses_global_prompt_cache_scope(body: Option<&Value>) -> bool {
    body.and_then(|payload| payload.get("system"))
        .and_then(Value::as_array)
        .is_some_and(|blocks| {
            blocks.iter().any(|block| {
                block
                    .get("cache_control")
                    .and_then(|cache| cache.get("scope"))
                    .and_then(Value::as_str)
                    == Some("global")
            })
        })
}

fn body_uses_thinking(body: Option<&Value>) -> bool {
    body.and_then(|payload| payload.get("thinking"))
        .is_some_and(|thinking| {
            thinking
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|kind| kind == "enabled" || kind == "adaptive")
        })
}

fn body_uses_context_management(body: Option<&Value>) -> bool {
    body.and_then(|payload| payload.get("context_management"))
        .is_some()
}

fn body_uses_fast_mode(body: Option<&Value>) -> bool {
    body.and_then(|payload| payload.get("speed"))
        .and_then(Value::as_str)
        == Some("fast")
}

fn body_uses_output_config_effort(body: Option<&Value>) -> bool {
    body.and_then(|payload| payload.get("output_config"))
        .and_then(|output_config| output_config.get("effort"))
        .is_some()
}

fn body_uses_output_config_task_budget(body: Option<&Value>) -> bool {
    body.and_then(|payload| payload.get("output_config"))
        .and_then(|output_config| output_config.get("task_budget"))
        .is_some()
}

fn body_uses_output_config_format(body: Option<&Value>) -> bool {
    body.and_then(|payload| payload.get("output_config"))
        .and_then(|output_config| output_config.get("format"))
        .is_some()
}

fn body_uses_tool_search_features(body: Option<&Value>) -> bool {
    let Some(payload) = body else {
        return false;
    };

    let uses_defer_loading = payload
        .get("tools")
        .and_then(Value::as_array)
        .is_some_and(|tools| {
            tools.iter().any(|tool| {
                tool.get("defer_loading").and_then(Value::as_bool) == Some(true)
                    || matches!(
                        tool.get("name").and_then(Value::as_str),
                        Some("ToolSearch" | "tool_search")
                    )
            })
        });
    if uses_defer_loading {
        return true;
    }

    payload
        .get("messages")
        .and_then(Value::as_array)
        .is_some_and(|messages| {
            messages.iter().any(|message| {
                message
                    .get("content")
                    .and_then(Value::as_array)
                    .is_some_and(|content| {
                        content.iter().any(|block| {
                            block.get("type").and_then(Value::as_str) == Some("tool_result")
                                && block.get("content").and_then(Value::as_array).is_some_and(
                                    |tool_result_content| {
                                        tool_result_content.iter().any(is_tool_reference_block)
                                    },
                                )
                        })
                    })
            })
        })
}

fn merge_anthropic_beta_body_param(body: &mut Value, beta: &str) {
    let mut betas = body
        .get("anthropic_beta")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if !betas.iter().any(|existing| existing == beta) {
        betas.push(beta.to_owned());
    }
    body["anthropic_beta"] = json!(betas);
}

fn to_openai_messages(conversation: &[ConversationEntry]) -> Vec<Value> {
    conversation
        .iter()
        .filter(|entry| !is_client_only_system_message(entry))
        .map(|entry| match entry.role {
            ConversationRole::System => json!({
                "role": role_name(&entry.role),
                "content": entry.history_text(),
            }),
            ConversationRole::User => {
                if entry.content_blocks.is_empty() && entry.attachments.is_empty() {
                    json!({
                        "role": "user",
                        "content": entry.history_text(),
                    })
                } else {
                    let mut parts = Vec::new();
                    if !entry.content_blocks.is_empty() {
                        parts.extend(entry.content_blocks.clone());
                    } else {
                        parts.push(json!({"type": "text", "text": entry.history_text()}));
                    }
                    for att in &entry.attachments {
                        parts.push(json!({
                            "type": "image_url",
                            "image_url": {
                                "url": format!("data:{};base64,{}", att.media_type.mime_type(), att.data),
                            }
                        }));
                    }
                    json!({
                        "role": "user",
                        "content": parts,
                    })
                }
            }
            ConversationRole::Assistant => {
                let mut message = json!({
                    "role": "assistant",
                    "content": entry.history_text(),
                });
                if !entry.tool_calls.is_empty() {
                    message["tool_calls"] = Value::Array(
                        entry
                            .tool_calls
                            .iter()
                            .map(|call| {
                                json!({
                                    "id": call.id,
                                    "type": "function",
                                    "function": {
                                        "name": provider_wire_tool_name(&call.name),
                                        "arguments": call.input.to_string(),
                                    }
                                })
                            })
                            .collect(),
                    );
                }
                message
            }
            ConversationRole::Tool => json!({
                "role": "tool",
                "tool_call_id": entry.tool_call_id,
                "content": entry.text,
            }),
        })
        .collect()
}

fn provider_wire_tool_name(name: &str) -> String {
    claude_tools::provider_wire_tool_name_for(name)
}

fn anthropic_user_blocks(entry: &ConversationEntry) -> Vec<Value> {
    let mut blocks = if entry.content_blocks.is_empty() {
        vec![json!({"type": "text", "text": entry.history_text()})]
    } else {
        entry.content_blocks.clone()
    };
    for att in &entry.attachments {
        blocks.push(json!({
            "type": "image",
            "source": {
                "type": "base64",
                "media_type": att.media_type.mime_type(),
                "data": att.data,
            }
        }));
    }
    blocks
}

fn anthropic_tool_result_block(tool_entry: &ConversationEntry) -> Value {
    let mut tool_result = json!({
        "type": "tool_result",
        "tool_use_id": tool_entry.tool_call_id,
        "content": if tool_entry.content_blocks.is_empty() {
            Value::String(tool_entry.text.clone())
        } else {
            Value::Array(tool_entry.content_blocks.clone())
        },
    });
    if tool_entry.is_error {
        tool_result["is_error"] = Value::Bool(true);
    }
    tool_result
}

fn append_anthropic_user_blocks(target: &mut Vec<Value>, mut addition: Vec<Value>) {
    let seam_is_text = target
        .last()
        .and_then(|block| block.get("type"))
        .and_then(Value::as_str)
        == Some("text")
        && addition
            .first()
            .and_then(|block| block.get("type"))
            .and_then(Value::as_str)
            == Some("text");

    if seam_is_text
        && let Some(Value::Object(last_block)) = target.last_mut()
        && let Some(Value::String(text)) = last_block.get_mut("text")
    {
        text.push('\n');
    }

    target.append(&mut addition);
}

fn to_anthropic_messages(conversation: &[ConversationEntry]) -> (Vec<Value>, Vec<Value>) {
    let mut system = Vec::new();
    for entry in conversation
        .iter()
        .filter(|entry| matches!(entry.role, ConversationRole::System))
        .filter(|entry| !is_client_only_system_message(entry))
    {
        if entry.content_blocks.is_empty() {
            let text = entry.history_text();
            if !text.is_empty() {
                system.push(json!({
                    "type": "text",
                    "text": text,
                }));
            }
        } else {
            system.extend(entry.content_blocks.iter().cloned());
        }
    }
    let non_system = conversation
        .iter()
        .filter(|entry| !matches!(entry.role, ConversationRole::System))
        .filter(|entry| !is_client_only_system_message(entry))
        .collect::<Vec<_>>();
    let mut messages = Vec::new();
    let mut index = 0usize;

    while index < non_system.len() {
        let entry = non_system[index];
        match entry.role {
            ConversationRole::Assistant => {
                if entry.content_blocks.is_empty() {
                    let mut blocks = Vec::new();
                    if !entry.history_text().is_empty() {
                        blocks.push(json!({"type": "text", "text": entry.history_text()}));
                    }
                    for call in &entry.tool_calls {
                        blocks.push(json!({
                            "type": "tool_use",
                            "id": call.id,
                            "name": provider_wire_tool_name(&call.name),
                            "input": call.input,
                        }));
                    }
                    messages.push(json!({
                        "role": "assistant",
                        "content": blocks,
                    }));
                } else {
                    let content_blocks = entry
                        .content_blocks
                        .iter()
                        .map(normalize_provider_assistant_content_block)
                        .collect::<Vec<_>>();
                    messages.push(json!({
                        "role": "assistant",
                        "content": content_blocks,
                    }));
                }
                index += 1;
            }
            ConversationRole::User | ConversationRole::Tool => {
                let mut tool_results = Vec::new();
                let mut user_blocks = Vec::new();

                while index < non_system.len()
                    && !matches!(non_system[index].role, ConversationRole::Assistant)
                {
                    let grouped_entry = non_system[index];
                    match grouped_entry.role {
                        ConversationRole::User => append_anthropic_user_blocks(
                            &mut user_blocks,
                            anthropic_user_blocks(grouped_entry),
                        ),
                        ConversationRole::Tool => {
                            tool_results.push(anthropic_tool_result_block(grouped_entry));
                        }
                        ConversationRole::System | ConversationRole::Assistant => {}
                    }
                    index += 1;
                }

                let mut content = tool_results;
                content.extend(user_blocks);
                if !content.is_empty() {
                    messages.push(json!({
                        "role": "user",
                        "content": content,
                    }));
                }
            }
            ConversationRole::System => index += 1,
        }
    }

    (system, messages)
}

fn normalize_provider_assistant_content_block(block: &Value) -> Value {
    if block.get("type").and_then(Value::as_str) != Some("tool_use") {
        return block.clone();
    }
    let mut normalized = block.clone();
    if let Some(name) = block.get("name").and_then(Value::as_str) {
        normalized["name"] = Value::String(provider_wire_tool_name(name));
    }
    normalized
}

fn is_client_only_system_message(entry: &ConversationEntry) -> bool {
    matches!(entry.role, ConversationRole::System)
        && matches!(
            entry.name.as_deref(),
            Some(
                "memory_saved"
                    | "turn_duration"
                    | "bridge_status"
                    | "api_metrics"
                    | "api_error"
                    | "agents_killed"
            )
        )
}

fn model_supports_tool_reference(model: Option<&str>) -> bool {
    !model
        .unwrap_or_default()
        .to_ascii_lowercase()
        .contains("haiku")
}

fn parse_auto_percentage(value: &str) -> Option<u64> {
    let lower = value.trim().to_ascii_lowercase();
    let percent = lower.strip_prefix("auto:")?.parse::<u64>().ok()?;
    Some(percent.min(100))
}

fn normalize_tool_search_env_value(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_lowercase())
}

fn is_env_truthy(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

fn is_env_defined_falsy(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "0" | "false" | "no" | "off"
    )
}

fn get_tool_search_mode_with_env(
    enable_tool_search: Option<&str>,
    disable_experimental_betas: Option<&str>,
) -> ToolSearchMode {
    if disable_experimental_betas.is_some_and(is_env_truthy) {
        return ToolSearchMode::Standard;
    }

    let enable_tool_search = normalize_tool_search_env_value(enable_tool_search);
    let auto_percent = enable_tool_search
        .as_deref()
        .and_then(parse_auto_percentage);
    if auto_percent == Some(0) {
        return ToolSearchMode::Tst;
    }
    if auto_percent == Some(100) {
        return ToolSearchMode::Standard;
    }
    if enable_tool_search
        .as_deref()
        .is_some_and(|value| value == "auto" || value.starts_with("auto:"))
    {
        return ToolSearchMode::TstAuto;
    }
    if enable_tool_search.as_deref().is_some_and(is_env_truthy) {
        return ToolSearchMode::Tst;
    }
    if enable_tool_search
        .as_deref()
        .is_some_and(is_env_defined_falsy)
    {
        return ToolSearchMode::Standard;
    }
    ToolSearchMode::Tst
}

fn is_first_party_anthropic_base_url(base_url: &str) -> bool {
    reqwest::Url::parse(base_url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(str::to_ascii_lowercase))
        .is_some_and(|host| host == "api.anthropic.com" || host == "api-staging.anthropic.com")
}

fn tool_search_enabled_optimistic_with_env(
    provider: &ProviderConfig,
    enable_tool_search: Option<&str>,
    disable_experimental_betas: Option<&str>,
) -> bool {
    let mode = get_tool_search_mode_with_env(enable_tool_search, disable_experimental_betas);
    if mode == ToolSearchMode::Standard {
        return false;
    }

    let explicit_tool_search = normalize_tool_search_env_value(enable_tool_search).is_some();
    if !explicit_tool_search
        && provider.protocol == ProviderProtocol::Anthropic
        && provider
            .base_url
            .as_deref()
            .is_some_and(|base_url| !is_first_party_anthropic_base_url(base_url))
    {
        return false;
    }

    true
}

fn get_auto_tool_search_percentage(enable_tool_search: Option<&str>) -> u64 {
    match normalize_tool_search_env_value(enable_tool_search) {
        None => DEFAULT_AUTO_TOOL_SEARCH_PERCENTAGE,
        Some(value) if value == "auto" => DEFAULT_AUTO_TOOL_SEARCH_PERCENTAGE,
        Some(value) => parse_auto_percentage(&value).unwrap_or(DEFAULT_AUTO_TOOL_SEARCH_PERCENTAGE),
    }
}

fn deferred_tool_description_chars(specs: &[ToolSpec]) -> usize {
    specs
        .iter()
        .filter(|spec| spec.is_deferred())
        .map(|spec| {
            spec.name.len()
                + spec.description.len()
                + serde_json::to_string(&spec.input_schema)
                    .map(|schema| schema.len())
                    .unwrap_or_default()
        })
        .sum()
}

fn tool_search_auto_threshold_met(
    provider: &ProviderConfig,
    specs: &[ToolSpec],
    enable_tool_search: Option<&str>,
) -> bool {
    let model = provider.model.as_deref().unwrap_or_default();
    let context_window = get_model_info(model).max_context;
    let threshold_tokens =
        context_window.saturating_mul(get_auto_tool_search_percentage(enable_tool_search)) / 100;
    let approx_tokens =
        (deferred_tool_description_chars(specs) as f64 / TOOL_SEARCH_CHARS_PER_TOKEN).ceil() as u64;
    approx_tokens >= threshold_tokens.max(1)
}

fn provider_uses_tool_search_with_env(
    provider: &ProviderConfig,
    specs: &[ToolSpec],
    enable_tool_search: Option<&str>,
    disable_experimental_betas: Option<&str>,
) -> bool {
    if !matches!(
        provider.protocol,
        ProviderProtocol::Anthropic | ProviderProtocol::Bedrock | ProviderProtocol::Vertex
    ) {
        return false;
    }
    if !tool_search_enabled_optimistic_with_env(
        provider,
        enable_tool_search,
        disable_experimental_betas,
    ) {
        return false;
    }
    if !model_supports_tool_reference(provider.model.as_deref()) {
        return false;
    }
    if !specs.iter().any(ToolSpec::is_tool_search) {
        return false;
    }
    if !specs.iter().any(ToolSpec::is_deferred) {
        return false;
    }

    match get_tool_search_mode_with_env(enable_tool_search, disable_experimental_betas) {
        ToolSearchMode::Tst => true,
        ToolSearchMode::TstAuto => {
            tool_search_auto_threshold_met(provider, specs, enable_tool_search)
        }
        ToolSearchMode::Standard => false,
    }
}

fn provider_uses_tool_search(provider: &ProviderConfig, specs: &[ToolSpec]) -> bool {
    let enable_tool_search = env::var("ENABLE_TOOL_SEARCH").ok();
    let disable_experimental_betas = env::var("CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS").ok();
    provider_uses_tool_search_with_env(
        provider,
        specs,
        enable_tool_search.as_deref(),
        disable_experimental_betas.as_deref(),
    )
}

pub async fn provider_runtime_tool_specs_for_request(
    provider: &ProviderConfig,
    conversation: &[ConversationEntry],
    carried_discovered_tools: &BTreeSet<String>,
) -> Vec<ToolSpec> {
    let specs = runtime_provider_tool_specs().await;
    if provider_uses_tool_search(provider, &specs) {
        return runtime_visible_provider_tool_specs_with_discovered_tools(
            conversation,
            carried_discovered_tools,
        )
        .await;
    }

    specs
        .into_iter()
        .filter(|spec| !spec.is_tool_search())
        .collect()
}

async fn current_openai_tool_schemas(
    provider: &ProviderConfig,
    conversation: &[ConversationEntry],
    carried_discovered_tools: &BTreeSet<String>,
) -> Vec<Value> {
    provider_runtime_tool_specs_for_request(provider, conversation, carried_discovered_tools)
        .await
        .into_iter()
        .map(|tool| tool.to_openai_schema())
        .collect()
}

async fn current_anthropic_tool_schemas(
    provider: &ProviderConfig,
    conversation: &[ConversationEntry],
    carried_discovered_tools: &BTreeSet<String>,
) -> Vec<Value> {
    let specs =
        provider_runtime_tool_specs_for_request(provider, conversation, carried_discovered_tools)
            .await;
    let tool_search_enabled = provider_uses_tool_search(provider, &specs);
    specs
        .into_iter()
        .map(|tool| {
            tool.to_anthropic_schema_with_options(tool_search_enabled && tool.is_deferred())
        })
        .collect()
}

fn tool_search_enabled_from_tool_schemas(tools: &[Value]) -> bool {
    tools
        .iter()
        .any(|tool| tool.get("name").and_then(Value::as_str) == Some("ToolSearch"))
}

fn available_tool_names_from_schemas(tools: &[Value]) -> BTreeSet<String> {
    tools
        .iter()
        .filter_map(|tool| tool.get("name").and_then(Value::as_str))
        .map(ToOwned::to_owned)
        .collect()
}

fn is_tool_reference_block(block: &Value) -> bool {
    block.get("type").and_then(Value::as_str) == Some("tool_reference")
}

fn normalize_anthropic_tool_result_content_blocks(
    content_blocks: &[Value],
    available_tool_names: &BTreeSet<String>,
    tool_search_enabled: bool,
) -> Vec<Value> {
    let filtered = content_blocks
        .iter()
        .filter(|block| {
            if !is_tool_reference_block(block) {
                return true;
            }
            if !tool_search_enabled {
                return false;
            }
            block
                .get("tool_name")
                .and_then(Value::as_str)
                .is_none_or(|tool_name| available_tool_names.contains(tool_name))
        })
        .cloned()
        .collect::<Vec<_>>();

    if !filtered.is_empty() {
        return filtered;
    }

    vec![json!({
        "type": "text",
        "text": if tool_search_enabled {
            "[Tool references removed - tools no longer available]"
        } else {
            "[Tool references removed - tool search not enabled]"
        },
    })]
}

fn normalize_anthropic_conversation_for_tool_search(
    conversation: &[ConversationEntry],
    available_tool_names: &BTreeSet<String>,
    tool_search_enabled: bool,
) -> Vec<ConversationEntry> {
    conversation
        .iter()
        .cloned()
        .map(|mut entry| {
            if entry.role != ConversationRole::Tool
                || entry.content_blocks.is_empty()
                || !entry.content_blocks.iter().any(is_tool_reference_block)
            {
                return entry;
            }

            entry.content_blocks = normalize_anthropic_tool_result_content_blocks(
                &entry.content_blocks,
                available_tool_names,
                tool_search_enabled,
            );
            entry
        })
        .collect()
}

fn user_message_content_has_tool_reference(content: &[Value]) -> bool {
    content.iter().any(|block| {
        block.get("type").and_then(Value::as_str) == Some("tool_result")
            && block
                .get("content")
                .and_then(Value::as_array)
                .is_some_and(|tool_result_content| {
                    tool_result_content.iter().any(is_tool_reference_block)
                })
    })
}

fn user_message_has_tool_reference_turn_boundary(content: &[Value]) -> bool {
    content.iter().any(|block| {
        block.get("type").and_then(Value::as_str) == Some("text")
            && block
                .get("text")
                .and_then(Value::as_str)
                .is_some_and(|text| text.starts_with(TOOL_REFERENCE_TURN_BOUNDARY))
    })
}

fn inject_tool_reference_turn_boundary_siblings(messages: Vec<Value>) -> Vec<Value> {
    messages
        .into_iter()
        .map(|mut message| {
            if message.get("role").and_then(Value::as_str) != Some("user") {
                return message;
            }
            let Some(content) = message.get("content").and_then(Value::as_array).cloned() else {
                return message;
            };
            if !user_message_content_has_tool_reference(&content)
                || user_message_has_tool_reference_turn_boundary(&content)
            {
                return message;
            }

            let mut updated = content;
            updated.push(json!({
                "type": "text",
                "text": TOOL_REFERENCE_TURN_BOUNDARY,
            }));
            message["content"] = Value::Array(updated);
            message
        })
        .collect()
}

fn tool_reference_relocation_enabled() -> bool {
    env::var("REMOTE_CODE_TOOLREF_DEFER_J8M")
        .ok()
        .as_deref()
        .map(is_env_truthy)
        .unwrap_or(true)
}

fn relocate_tool_reference_siblings(messages: Vec<Value>) -> Vec<Value> {
    let mut result = messages;

    for index in 0..result.len() {
        let Some(content) = result[index]
            .get("content")
            .and_then(Value::as_array)
            .cloned()
        else {
            continue;
        };
        if result[index].get("role").and_then(Value::as_str) != Some("user")
            || !user_message_content_has_tool_reference(&content)
        {
            continue;
        }

        let text_siblings = content
            .iter()
            .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
            .cloned()
            .collect::<Vec<_>>();
        if text_siblings.is_empty() {
            continue;
        }

        let target_index = ((index + 1)..result.len()).find(|candidate_index| {
            let Some(candidate_content) = result[*candidate_index]
                .get("content")
                .and_then(Value::as_array)
            else {
                return false;
            };
            result[*candidate_index].get("role").and_then(Value::as_str) == Some("user")
                && candidate_content
                    .iter()
                    .any(|block| block.get("type").and_then(Value::as_str) == Some("tool_result"))
                && !user_message_content_has_tool_reference(candidate_content)
        });

        let Some(target_index) = target_index else {
            continue;
        };

        result[index]["content"] = Value::Array(
            content
                .into_iter()
                .filter(|block| block.get("type").and_then(Value::as_str) != Some("text"))
                .collect(),
        );
        let mut target_content = result[target_index]
            .get("content")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        target_content.extend(text_siblings);
        result[target_index]["content"] = Value::Array(target_content);
    }

    result
}

async fn prepare_anthropic_request_surface(
    provider: &ProviderConfig,
    conversation: &[ConversationEntry],
    carried_discovered_tools: &BTreeSet<String>,
) -> (Vec<Value>, Vec<Value>, Vec<Value>) {
    let tools =
        current_anthropic_tool_schemas(provider, conversation, carried_discovered_tools).await;
    let available_tool_names = available_tool_names_from_schemas(&tools);
    let normalized_conversation = normalize_anthropic_conversation_for_tool_search(
        conversation,
        &available_tool_names,
        tool_search_enabled_from_tool_schemas(&tools),
    );
    let (system, messages) = to_anthropic_messages(&normalized_conversation);
    let messages = if tool_reference_relocation_enabled() {
        relocate_tool_reference_siblings(messages)
    } else {
        inject_tool_reference_turn_boundary_siblings(messages)
    };
    // Normalize messages for API: role alternation, tool pairing, thinking cleanup
    let mut messages = messages;
    let tool_search = tool_search_enabled_from_tool_schemas(&tools);
    let available_tool_names_hashset: HashSet<String> = available_tool_names.into_iter().collect();
    let config = normalize::NormalizeConfig {
        tool_search_enabled: tool_search,
        available_tool_names: Some(&available_tool_names_hashset),
    };
    normalize::normalize_messages_for_api_with_config(&mut messages, config);
    (system, messages, tools)
}

async fn build_anthropic_request_body(
    provider: &ProviderConfig,
    conversation: &[ConversationEntry],
    carried_discovered_tools: &BTreeSet<String>,
    request_context: Option<&query_source::ProviderRequestContext>,
    stream: bool,
) -> Value {
    let (mut system, messages, tools) =
        prepare_anthropic_request_surface(provider, conversation, carried_discovered_tools).await;

    // Prepend billing attribution as the first system prompt block, matching
    // the official Claude Code CLI.  The fingerprint is derived from the first
    // user message text using the same SHA-256 + salt algorithm.
    if matches!(provider.protocol, ProviderProtocol::Anthropic) {
        let fp = fingerprint::compute_attribution_fingerprint(
            &messages,
            claude_config::runtime_version(),
        );
        let attr_text = attribution::build_billing_attribution_text(&fp);
        system.insert(
            0,
            json!({
                "type": "text",
                "text": attr_text,
            }),
        );
    }

    let mut body = json!({
        "model": provider.model,
        "system": system,
        "messages": messages,
        "tools": tools,
        "max_tokens": provider.max_output_tokens,
        "stream": stream,
    });

    merge_anthropic_extra_body_params(&mut body);
    apply_anthropic_request_metadata(&mut body, provider, request_context);
    apply_anthropic_thinking_options(&mut body, provider);
    apply_anthropic_sampling_params(&mut body, provider);
    apply_anthropic_output_config(&mut body, provider, request_context);
    apply_anthropic_fast_mode(&mut body, provider, request_context);
    apply_anthropic_context_management(&mut body, provider);
    apply_anthropic_interleaved_thinking(&mut body, provider);

    // Detect resume: if there are tool-role entries, this is a continued conversation.
    let is_resume = conversation
        .iter()
        .any(|entry| matches!(entry.role, ConversationRole::Tool));
    add_stable_cache_control(&mut body, is_resume);
    body
}

fn merge_anthropic_extra_body_params(body: &mut Value) {
    let extra_params = beta_headers::get_extra_body_params(None);
    let Some(extra_object) = extra_params.as_object() else {
        return;
    };

    for (key, value) in extra_object {
        if key == "output_config"
            && let Some(existing) = body.get_mut("output_config")
            && let (Some(existing_object), Some(extra_output_config)) =
                (existing.as_object_mut(), value.as_object())
        {
            for (output_key, output_value) in extra_output_config {
                existing_object
                    .entry(output_key.clone())
                    .or_insert_with(|| output_value.clone());
            }
            continue;
        }
        if body.get(key).is_none() {
            body[key.clone()] = value.clone();
        }
    }
}

fn apply_anthropic_thinking_options(body: &mut Value, provider: &ProviderConfig) {
    if claude_config::env_vars::disable_thinking() {
        body["temperature"] = json!(1.0);
        return;
    }

    let Some(raw_budget) = provider.thinking_budget else {
        body["temperature"] = json!(1.0);
        return;
    };

    let model = provider.model.as_deref().unwrap_or("");

    if !claude_config::env_vars::disable_adaptive_thinking()
        && thinking_blocks::should_use_adaptive_thinking(model)
    {
        body["thinking"] = json!({ "type": "adaptive" });
        return;
    }

    let max_tokens = body.get("max_tokens").and_then(Value::as_u64).unwrap_or(0);
    let budget = if max_tokens > 0 {
        std::cmp::min(u64::from(raw_budget), max_tokens.saturating_sub(1)) as u32
    } else {
        raw_budget
    };

    body["thinking"] = json!({
        "type": "enabled",
        "budget_tokens": budget,
    });
    let current_max = body.get("max_tokens").and_then(Value::as_u64).unwrap_or(0);
    if current_max <= u64::from(budget) {
        body["max_tokens"] = json!(u64::from(budget) + 4096);
    }
}

/// Apply configurable sampling parameters (temperature, top_p, top_k) to the
/// Anthropic request body.
///
/// When extended thinking is enabled, the Anthropic API requires
/// `temperature` to be exactly 1.0 — in that case we do not override it.
/// When thinking is disabled, the configured temperature (or a default of
/// 1.0) is used.
fn apply_anthropic_sampling_params(body: &mut Value, provider: &ProviderConfig) {
    // When thinking is enabled, Anthropic requires temperature=1.0 — skip override.
    let thinking_enabled = body
        .get("thinking")
        .and_then(|t| t.get("type"))
        .and_then(Value::as_str)
        .is_some_and(|t| t == "enabled" || t == "adaptive");

    if !thinking_enabled {
        // Use configured temperature, or default to 1.0 (already set by
        // apply_anthropic_thinking_options when thinking is disabled).
        if let Some(temp) = provider.temperature {
            body["temperature"] = json!(temp);
        }
    }

    if let Some(top_p) = provider.top_p {
        body["top_p"] = json!(top_p);
    }
    if let Some(top_k) = provider.top_k {
        body["top_k"] = json!(top_k);
    }
}

/// Apply interleaved thinking mode based on the `DISABLE_INTERLEAVED_THINKING`
/// env var.
///
/// When thinking is enabled and interleaved thinking is NOT disabled, we set
/// `thinking.interleaved` to true.  When `DISABLE_INTERLEAVED_THINKING` is
/// set, we explicitly set it to false (or omit it, which defaults to false).
fn apply_anthropic_interleaved_thinking(body: &mut Value, _provider: &ProviderConfig) {
    let Some(thinking) = body.get_mut("thinking") else {
        return;
    };
    // Only applies when thinking is enabled.
    if thinking
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|t| t == "enabled")
    {
        let interleaved = !claude_config::env_vars::disable_interleaved_thinking();
        thinking["interleaved"] = json!(interleaved);
    }
}

fn apply_anthropic_output_config(
    body: &mut Value,
    provider: &ProviderConfig,
    request_context: Option<&query_source::ProviderRequestContext>,
) {
    let mut output_config = body
        .get("output_config")
        .cloned()
        .unwrap_or_else(|| json!({}));

    if let Some(context) = request_context {
        if let Some(effort) = context.effort.as_deref() {
            let model = provider.model.as_deref().unwrap_or_default();
            if crate::effort_params::model_supports_effort(model)
                && output_config.get("effort").is_none()
            {
                output_config["effort"] = json!(effort);
            }
        }

        if first_party_only_request(provider)
            && let Some(task_budget) = context.task_budget.as_ref()
            && output_config.get("task_budget").is_none()
        {
            output_config["task_budget"] = json!({
                "type": "tokens",
                "total": task_budget.total,
            });
            if let Some(remaining) = task_budget.remaining {
                output_config["task_budget"]["remaining"] = json!(remaining);
            }
        }
    }

    if output_config
        .as_object()
        .is_some_and(|object| !object.is_empty())
    {
        body["output_config"] = output_config;
    }
}

fn apply_anthropic_fast_mode(
    body: &mut Value,
    provider: &ProviderConfig,
    request_context: Option<&query_source::ProviderRequestContext>,
) {
    let Some(context) = request_context else {
        return;
    };
    if context.fast_mode
        && (first_party_only_request(provider)
            || env::var("REMOTE_CODE_FORCE_FAST_MODE")
                .ok()
                .as_deref()
                .is_some_and(is_env_truthy))
    {
        body["speed"] = json!("fast");
    }
}

fn apply_anthropic_context_management(body: &mut Value, provider: &ProviderConfig) {
    if !body.get("thinking").is_some_and(|thinking| {
        thinking
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|value| value != "disabled")
    }) {
        return;
    }
    if !first_party_only_request(provider)
        && !env::var("REMOTE_CODE_FORCE_CONTEXT_MANAGEMENT")
            .ok()
            .as_deref()
            .is_some_and(is_env_truthy)
    {
        return;
    }
    if body.get("context_management").is_none() {
        body["context_management"] = json!({
            "edits": [{
                "type": "clear_thinking_20251015",
                "keep": "all",
            }],
        });
    }
}

fn first_party_only_request(provider: &ProviderConfig) -> bool {
    provider
        .base_url
        .as_deref()
        .is_some_and(is_first_party_anthropic_base_url)
}

fn parse_openai_response(status: u16, raw_text: String) -> Result<ProviderResponse> {
    let payload: Value = serde_json::from_str(&raw_text)
        .with_context(|| format!("provider returned non-JSON output: {}", truncate(&raw_text)))?;
    if status >= 400 {
        let error_message = payload
            .get("error")
            .and_then(|value| value.get("message"))
            .and_then(Value::as_str)
            .or_else(|| payload.get("message").and_then(Value::as_str))
            .unwrap_or("provider error");
        let pe = classify_provider_error(status, error_message, "openai");
        return Err(anyhow::Error::from(pe).context(format!(
            "provider request failed ({status}): {error_message}"
        )));
    }

    let choice = payload
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .ok_or_else(|| anyhow!("provider response did not include choices[0].message"))?;

    let tool_calls = choice
        .get("tool_calls")
        .and_then(Value::as_array)
        .map(|tool_calls| {
            tool_calls
                .iter()
                .map(parse_openai_tool_call)
                .collect::<Result<Vec<_>>>()
                .map(|calls| calls.into_iter().flatten().collect::<Vec<_>>())
        })
        .transpose()?
        .unwrap_or_default();
    let raw_assistant_text = coerce_text_content(choice.get("content")).trim().to_owned();
    let usage = payload.get("usage").cloned().unwrap_or_default();

    // OpenAI reasoning models may include reasoning in the refusal field or
    // as a reasoning_content field (non-standard, some providers expose it).
    let reasoning_text = choice
        .get("reasoning_content")
        .and_then(Value::as_str)
        .map(String::from)
        .or_else(|| {
            choice
                .get("reasoning")
                .and_then(Value::as_str)
                .map(String::from)
        });

    Ok(ProviderResponse {
        text: strip_reasoning_tags(&raw_assistant_text),
        history_text: Some(raw_assistant_text),
        thinking: reasoning_text,
        content_blocks: Vec::new(),
        tool_calls,
        request_id: payload
            .get("id")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        usage: UsageSummary {
            input_tokens: usage
                .get("prompt_tokens")
                .and_then(Value::as_u64)
                .unwrap_or_default(),
            output_tokens: usage
                .get("completion_tokens")
                .and_then(Value::as_u64)
                .unwrap_or_default(),
            cache_read_input_tokens: usage
                .get("cached_tokens")
                .or_else(|| {
                    usage
                        .get("prompt_tokens_details")
                        .and_then(|d| d.get("cached_tokens"))
                })
                .and_then(Value::as_u64)
                .unwrap_or_default(),
            cache_creation_input_tokens: usage
                .get("prompt_tokens_details")
                .and_then(|d| d.get("all_cached_tokens"))
                .and_then(Value::as_u64)
                .unwrap_or_default(),
            ..Default::default()
        },
        stop_reason: payload
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
            .and_then(|choice| choice.get("finish_reason"))
            .and_then(Value::as_str)
            .unwrap_or("stop")
            .to_owned(),
        research: None,
    })
}

fn parse_anthropic_response(status: u16, raw_text: String) -> Result<ProviderResponse> {
    let payload: Value = serde_json::from_str(&raw_text)
        .with_context(|| format!("provider returned non-JSON output: {}", truncate(&raw_text)))?;
    if status >= 400 {
        let error_message = payload
            .get("error")
            .and_then(|value| value.get("message"))
            .and_then(Value::as_str)
            .or_else(|| payload.get("message").and_then(Value::as_str))
            .unwrap_or("provider error");
        let pe = classify_provider_error(status, error_message, "anthropic");
        return Err(anyhow::Error::from(pe).context(format!(
            "provider request failed ({status}): {error_message}"
        )));
    }
    let blocks = payload
        .get("content")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let text = blocks
        .iter()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n");
    let tool_calls = blocks
        .iter()
        .map(parse_anthropic_tool_like_call)
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    let thinking_text: String = blocks
        .iter()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("thinking"))
        .filter_map(|block| block.get("thinking").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n");
    let usage = payload.get("usage").cloned().unwrap_or_default();

    Ok(ProviderResponse {
        text: strip_reasoning_tags(&text),
        history_text: Some(text),
        thinking: if thinking_text.is_empty() {
            None
        } else {
            Some(thinking_text)
        },
        content_blocks: blocks,
        tool_calls,
        request_id: payload
            .get("id")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        usage: UsageSummary {
            input_tokens: usage
                .get("input_tokens")
                .and_then(Value::as_u64)
                .unwrap_or_default(),
            output_tokens: usage
                .get("output_tokens")
                .and_then(Value::as_u64)
                .unwrap_or_default(),
            cache_read_input_tokens: usage
                .get("cache_read_input_tokens")
                .and_then(Value::as_u64)
                .unwrap_or_default(),
            cache_creation_input_tokens: usage
                .get("cache_creation_input_tokens")
                .and_then(Value::as_u64)
                .unwrap_or_default(),
            ..Default::default()
        },
        stop_reason: payload
            .get("stop_reason")
            .and_then(Value::as_str)
            .unwrap_or("stop")
            .to_owned(),
        research: None,
    })
}

fn parse_openai_tool_call(value: &Value) -> Result<Option<ToolCall>> {
    let Some(function) = value.get("function") else {
        return Ok(None);
    };
    let Some(id) = value.get("id").and_then(Value::as_str) else {
        return Ok(None);
    };
    let Some(name) = function.get("name").and_then(Value::as_str) else {
        return Ok(None);
    };
    let input = match function.get("arguments").and_then(Value::as_str) {
        Some(raw) if !raw.trim().is_empty() => serde_json::from_str(raw).with_context(|| {
            format!("provider returned invalid JSON arguments for tool call `{name}`")
        })?,
        _ => json!({}),
    };
    Ok(Some(ToolCall {
        id: id.to_owned(),
        name: name.to_owned(),
        input,
    }))
}

fn parse_anthropic_tool_call(value: &Value) -> Result<ToolCall> {
    let block_type = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("tool_use");
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("provider {block_type} block is missing string id"))?
        .to_owned();
    let name = value
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("provider {block_type} block is missing string name"))?
        .to_owned();
    let input = value.get("input").cloned().unwrap_or_else(|| json!({}));
    Ok(ToolCall { id, name, input })
}

fn parse_anthropic_tool_like_call(value: &Value) -> Result<Option<ToolCall>> {
    match value.get("type").and_then(Value::as_str) {
        Some("tool_use") | Some("server_tool_use") => parse_anthropic_tool_call(value).map(Some),
        _ => Ok(None),
    }
}

fn coerce_text_content(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(value)) => value.clone(),
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| {
                if let Some(text) = item.as_str() {
                    return Some(text.to_owned());
                }
                item.get("text")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned)
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Some(Value::Object(object)) => object
            .get("text")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .unwrap_or_default(),
        _ => String::new(),
    }
}

fn role_name(role: &ConversationRole) -> &'static str {
    match role {
        ConversationRole::System => "system",
        ConversationRole::User => "user",
        ConversationRole::Assistant => "assistant",
        ConversationRole::Tool => "tool",
    }
}

fn mock_response(conversation: &[ConversationEntry]) -> ProviderResponse {
    let user_prompt = conversation
        .iter()
        .rev()
        .find(|entry| matches!(entry.role, ConversationRole::User))
        .map_or_else(
            || "No prompt supplied.".to_owned(),
            ConversationEntry::history_text,
        );
    let has_tool_result_after_latest_user = conversation
        .iter()
        .rev()
        .take_while(|entry| !matches!(entry.role, ConversationRole::User))
        .any(|entry| matches!(entry.role, ConversationRole::Tool));
    ProviderResponse {
        text: if has_tool_result_after_latest_user {
            "mock provider observed the tool result and is ready to finish.".to_owned()
        } else {
            format!("mock provider response: {}", truncate(&user_prompt))
        },
        history_text: Some(user_prompt.clone()),
        thinking: None,
        content_blocks: Vec::new(),
        tool_calls: if !has_tool_result_after_latest_user
            && user_prompt.to_ascii_lowercase().contains("list files")
        {
            vec![ToolCall {
                id: "mock-tool-call-1".to_owned(),
                name: claude_tools::builtin_tool_specs()
                    .first()
                    .map_or_else(|| "list_directory".to_owned(), |tool| tool.name.clone()),
                input: json!({"path": ".", "recursive": false, "max_entries": 32}),
            }]
        } else {
            Vec::new()
        },
        request_id: Some("mock-request-id".to_owned()),
        usage: UsageSummary {
            input_tokens: 16,
            output_tokens: 12,
            cache_read_input_tokens: 0,
            cache_creation_input_tokens: 0,
            ..Default::default()
        },
        stop_reason: "end_turn".to_owned(),
        research: None,
    }
}

pub(crate) fn truncate(value: &str) -> String {
    value.chars().take(240).collect()
}

fn strip_reasoning_tags(text: &str) -> String {
    let mut remaining = text.to_owned();
    loop {
        let Some(start) = remaining.find("<think>") else {
            break;
        };
        let Some(end) = remaining[start..].find("</think>") else {
            break;
        };
        let end = start + end + "</think>".len();
        remaining.replace_range(start..end, "");
    }
    remaining.trim().to_owned()
}

fn normalize_tool_cache_order(tools: &mut Vec<Value>) {
    let mut builtin_tools = Vec::new();
    let mut mcp_tools = Vec::new();
    for tool in std::mem::take(tools) {
        let is_mcp_tool = tool
            .get("name")
            .and_then(Value::as_str)
            .is_some_and(|name| name.starts_with("mcp__"));
        if is_mcp_tool {
            mcp_tools.push(tool);
        } else {
            builtin_tools.push(tool);
        }
    }
    builtin_tools.extend(mcp_tools);
    *tools = builtin_tools;
}

/// Add stable Anthropic prompt caching markers (`cache_control: {"type": "ephemeral"}`)
/// to strategic locations in the request body so that the system prompt,
/// tool definitions, and the most recent user message are cached server-side.
///
/// When `is_resume` is true (conversation has prior tool results), the tool list
/// is kept exactly as-is to avoid `deferred_tools_delta` cache-miss issues.
fn add_stable_cache_control(body: &mut Value, is_resume: bool) {
    // 0. Keep built-in tools as a stable prefix and append MCP tools after them.
    //    The runtime already produces deterministic ordering within each bucket.
    if let Some(tools) = body.get_mut("tools")
        && let Some(tools_arr) = tools.as_array_mut()
    {
        normalize_tool_cache_order(tools_arr);
    }

    // 1. System message — always ensure array format with cache_control.
    if let Some(system) = body.get_mut("system") {
        if system.is_string() {
            let text = system.as_str().unwrap_or("").to_owned();
            *system = json!([{
                "type": "text",
                "text": text,
                "cache_control": {"type": "ephemeral"}
            }]);
        } else if let Some(system_arr) = system.as_array_mut()
            && !system_arr
                .iter()
                .any(|block| block.get("cache_control").is_some())
            && let Some(last) = system_arr.last_mut()
        {
            last["cache_control"] = json!({"type": "ephemeral"});
        }
    }

    // 2. Most recent user message — ensure content is array format, mark cache_control.
    if let Some(messages) = body.get_mut("messages")
        && let Some(msg_arr) = messages.as_array_mut()
    {
        for msg in msg_arr.iter_mut().rev() {
            if msg["role"] == "user" {
                if let Some(content) = msg.get_mut("content") {
                    if content.is_string() {
                        let text = content.as_str().unwrap_or("").to_owned();
                        *content = json!([{
                            "type": "text",
                            "text": text,
                            "cache_control": {"type": "ephemeral"}
                        }]);
                    } else if let Some(content_arr) = content.as_array_mut()
                        && let Some(last_block) = content_arr.last_mut()
                    {
                        last_block["cache_control"] = json!({"type": "ephemeral"});
                    }
                }
                break;
            }
        }
    }

    // 4. Resume scenario: tool list must remain identical to avoid cache invalidation.
    //    Currently the tool list is always the full builtin set, which is inherently stable.
    //    When is_resume is true, we skip tool reordering above to preserve cache hits.
    if is_resume {
        tracing::debug!(
            "add_stable_cache_control: resume mode — tool list kept as-is for cache stability"
        );
    }
}

// ---------------------------------------------------------------------------
// Structured error classification
// ---------------------------------------------------------------------------

/// Structured provider error with classification for retry/recovery decisions.
///
/// Matches upstream Claude Code's `categorizeRetryableAPIError` logic.
#[derive(Debug, Clone)]
pub struct ProviderError {
    /// Error category.
    pub category: ErrorCategory,
    /// HTTP status code (if applicable).
    pub status_code: Option<u16>,
    /// Human-readable error message.
    pub message: String,
    /// Provider name that produced the error.
    pub provider_name: String,
    /// Suggested recovery action.
    pub recovery: RecoveryAction,
}

/// Classification of provider errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCategory {
    /// Rate limit exceeded (429).
    RateLimit,
    /// Authentication failure (401/403).
    Authentication,
    /// Request too large / prompt too long (400/413).
    PromptTooLong,
    /// Model not found or unavailable (404).
    ModelNotFound,
    /// Server error (5xx).
    ServerError,
    /// Network / connectivity error.
    Network,
    /// Timeout.
    Timeout,
    /// Streaming interrupted.
    StreamInterrupted,
    /// Invalid request format.
    InvalidRequest,
    /// Quota / billing exceeded (402).
    QuotaExceeded,
    /// Unknown / unclassified error.
    Unknown,
}

/// Suggested recovery action for a provider error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryAction {
    /// Retry with exponential backoff.
    Retry,
    /// Retry after compacting the conversation.
    CompactAndRetry,
    /// Failover to a different provider.
    Failover,
    /// Abort the operation.
    Abort,
    /// Ask the user to fix configuration.
    FixConfig,
}

/// Classify an HTTP status code and error message into a structured error.
#[must_use]
pub fn classify_provider_error(
    status_code: u16,
    message: &str,
    provider_name: &str,
) -> ProviderError {
    let (category, recovery) = match status_code {
        429 => (ErrorCategory::RateLimit, RecoveryAction::Retry),
        401 | 403 => (ErrorCategory::Authentication, RecoveryAction::FixConfig),
        402 => (ErrorCategory::QuotaExceeded, RecoveryAction::Failover),
        404 => (ErrorCategory::ModelNotFound, RecoveryAction::FixConfig),
        413 => (
            ErrorCategory::PromptTooLong,
            RecoveryAction::CompactAndRetry,
        ),
        400 => {
            // Check if it's a prompt-too-long error disguised as 400.
            if message.contains("prompt is too long")
                || message.contains("context_length_exceeded")
                || message.contains("maximum context length")
            {
                (
                    ErrorCategory::PromptTooLong,
                    RecoveryAction::CompactAndRetry,
                )
            } else {
                (ErrorCategory::InvalidRequest, RecoveryAction::Abort)
            }
        }
        500 | 502 | 503 | 504 => (ErrorCategory::ServerError, RecoveryAction::Retry),
        _ => (ErrorCategory::Unknown, RecoveryAction::Retry),
    };

    ProviderError {
        category,
        status_code: Some(status_code),
        message: message.to_owned(),
        provider_name: provider_name.to_owned(),
        recovery,
    }
}

/// Classify a network/transport error.
#[must_use]
pub fn classify_network_error(error: &str, provider_name: &str) -> ProviderError {
    let (category, recovery) = if error.contains("timed out") || error.contains("timeout") {
        (ErrorCategory::Timeout, RecoveryAction::Retry)
    } else if error.contains("connection refused") || error.contains("couldn't connect") {
        (ErrorCategory::Network, RecoveryAction::Retry)
    } else if error.contains("tls")
        || error.contains("certificate")
        || error.contains("ssl")
        || error.contains("dns")
        || error.contains("resolve")
    {
        (ErrorCategory::Network, RecoveryAction::FixConfig)
    } else {
        (ErrorCategory::Network, RecoveryAction::Retry)
    };

    ProviderError {
        category,
        status_code: None,
        message: error.to_owned(),
        provider_name: provider_name.to_owned(),
        recovery,
    }
}

/// Check if an error is retryable.
#[must_use]
pub fn is_retryable(error: &ProviderError) -> bool {
    matches!(
        error.recovery,
        RecoveryAction::Retry | RecoveryAction::CompactAndRetry | RecoveryAction::Failover
    )
}

/// Check if an error indicates the prompt is too long.
#[must_use]
pub fn is_prompt_too_long(error: &ProviderError) -> bool {
    error.category == ErrorCategory::PromptTooLong
}

impl std::fmt::Display for ProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[{}] {} (status={:?}, provider={})",
            match self.category {
                ErrorCategory::RateLimit => "RateLimit",
                ErrorCategory::Authentication => "Authentication",
                ErrorCategory::PromptTooLong => "PromptTooLong",
                ErrorCategory::ModelNotFound => "ModelNotFound",
                ErrorCategory::ServerError => "ServerError",
                ErrorCategory::Network => "Network",
                ErrorCategory::Timeout => "Timeout",
                ErrorCategory::StreamInterrupted => "StreamInterrupted",
                ErrorCategory::InvalidRequest => "InvalidRequest",
                ErrorCategory::QuotaExceeded => "QuotaExceeded",
                ErrorCategory::Unknown => "Unknown",
            },
            self.message,
            self.status_code,
            self.provider_name,
        )
    }
}

impl std::error::Error for ProviderError {}

/// Try to extract a [`ProviderError`] from an `anyhow::Error` chain.
/// Returns `None` if the error chain does not contain a `ProviderError`.
#[must_use]
pub fn extract_provider_error(error: &anyhow::Error) -> Option<&ProviderError> {
    error.downcast_ref::<ProviderError>()
}

#[cfg(test)]
mod tests {
    use super::{
        ProviderClient, add_stable_cache_control, apply_anthropic_request_metadata, build_headers,
        current_anthropic_tool_schemas, get_tool_search_mode_with_env,
        inject_tool_reference_turn_boundary_siblings, mock_response, parse_anthropic_response,
        parse_openai_response, provider_runtime_tool_specs_for_request,
        provider_uses_tool_search_with_env, relocate_tool_reference_siblings, strip_reasoning_tags,
        to_anthropic_messages, to_openai_messages,
    };
    use axum::{Json, Router, extract::State, routing::post};
    use claude_core::{ConversationEntry, ToolCall};
    use serde_json::json;
    use std::collections::BTreeSet;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use tokio::net::TcpListener;

    fn test_provider_config(base_url: String) -> claude_config::ProviderConfig {
        claude_config::ProviderConfig {
            name: "custom".to_owned(),
            base_url: Some(base_url),
            api_key: Some("test-key".to_owned()),
            model: Some("test-model".to_owned()),
            protocol: claude_core::ProviderProtocol::OpenAi,
            timeout_ms: 10_000,
            max_output_tokens: 512,
            max_retries: 2,
            retry_initial_backoff_ms: 10,
            retry_max_backoff_ms: 20,
            respect_retry_after: false,
            request_header_overrides: Default::default(),
            request_metadata: Default::default(),
            thinking_budget: None,
            temperature: None,
            top_p: None,
            top_k: None,
        }
    }

    #[test]
    fn anthropic_messages_endpoint_overrides_openai_protocol_for_routing() {
        let provider = test_provider_config("https://example.test/v1/messages".to_owned());

        assert!(super::provider_prefers_anthropic_messages_route(&provider));

        let routed = super::provider_as_anthropic_compatible(&provider);
        assert_eq!(routed.protocol, claude_core::ProviderProtocol::Anthropic);
        assert_eq!(
            routed.base_url.as_deref(),
            Some("https://example.test/v1/messages")
        );
    }

    #[test]
    fn openai_chat_endpoint_keeps_openai_protocol_for_routing() {
        let provider = test_provider_config("https://example.test/v1/chat/completions".to_owned());

        assert!(!super::provider_prefers_anthropic_messages_route(&provider));
    }

    #[test]
    fn reasoning_tags_are_removed() {
        assert_eq!(strip_reasoning_tags("<think>abc</think>done"), "done");
    }

    #[test]
    fn cache_control_keeps_builtin_tools_before_mcp_tools() {
        let mut body = json!({
            "tools": [
                {"name": "mcp__zeta__search"},
                {"name": "read_file"},
                {"name": "mcp__alpha__lookup"},
                {"name": "write_file"}
            ]
        });

        add_stable_cache_control(&mut body, false);

        let tool_names = body["tools"]
            .as_array()
            .unwrap_or_else(|| panic!("tools array"))
            .iter()
            .filter_map(|tool| tool.get("name").and_then(serde_json::Value::as_str))
            .collect::<Vec<_>>();
        assert_eq!(
            tool_names,
            vec![
                "read_file",
                "write_file",
                "mcp__zeta__search",
                "mcp__alpha__lookup",
            ]
        );
    }

    #[test]
    fn tool_search_mode_honors_env_and_kill_switch() {
        assert_eq!(
            get_tool_search_mode_with_env(None, None),
            super::ToolSearchMode::Tst
        );
        assert_eq!(
            get_tool_search_mode_with_env(Some("auto"), None),
            super::ToolSearchMode::TstAuto
        );
        assert_eq!(
            get_tool_search_mode_with_env(Some("auto:0"), None),
            super::ToolSearchMode::Tst
        );
        assert_eq!(
            get_tool_search_mode_with_env(Some("auto:100"), None),
            super::ToolSearchMode::Standard
        );
        assert_eq!(
            get_tool_search_mode_with_env(Some("true"), Some("1")),
            super::ToolSearchMode::Standard
        );
    }

    #[tokio::test]
    async fn proxy_anthropic_requests_fall_back_to_inline_tools_by_default() {
        let mut provider = test_provider_config("https://proxy.example.com/v1/messages".to_owned());
        provider.protocol = claude_core::ProviderProtocol::Anthropic;

        let specs = provider_runtime_tool_specs_for_request(&provider, &[], &BTreeSet::new()).await;
        let names = specs
            .iter()
            .map(|spec| spec.provider_wire_name())
            .collect::<Vec<_>>();

        assert!(names.contains(&"WebFetch"));
        assert!(!names.contains(&"ToolSearch"));
    }

    #[tokio::test]
    async fn proxy_anthropic_tool_search_can_be_forced_on_explicitly() {
        let mut provider = test_provider_config("https://proxy.example.com/v1/messages".to_owned());
        provider.protocol = claude_core::ProviderProtocol::Anthropic;

        let specs = claude_tools::runtime_provider_tool_specs().await;
        assert!(!provider_uses_tool_search_with_env(
            &provider, &specs, None, None
        ));
        assert!(provider_uses_tool_search_with_env(
            &provider,
            &specs,
            Some("true"),
            None,
        ));
    }

    #[tokio::test]
    async fn anthropic_tool_schemas_only_include_discovered_deferred_tools() {
        let mut provider = test_provider_config("https://api.anthropic.com/v1/messages".to_owned());
        provider.protocol = claude_core::ProviderProtocol::Anthropic;

        let initial = current_anthropic_tool_schemas(&provider, &[], &BTreeSet::new())
            .await
            .into_iter()
            .filter_map(|tool| {
                tool.get("name")
                    .and_then(serde_json::Value::as_str)
                    .map(ToOwned::to_owned)
            })
            .collect::<Vec<_>>();

        assert!(initial.iter().any(|name| name == "ToolSearch"));
        assert!(initial.iter().any(|name| name == "Read"));
        assert!(!initial.iter().any(|name| name == "WebFetch"));

        let discovered = current_anthropic_tool_schemas(
            &provider,
            &[ConversationEntry::tool(
                "tool-1",
                "tool_search",
                r#"{"query":"web","results":[{"name":"web_fetch"}]}"#,
                false,
            )],
            &BTreeSet::new(),
        )
        .await;

        assert!(
            discovered.iter().any(
                |tool| tool.get("name").and_then(serde_json::Value::as_str) == Some("WebFetch")
            )
        );
        assert!(discovered.iter().any(|tool| {
            tool.get("name").and_then(serde_json::Value::as_str) == Some("WebFetch")
                && tool
                    .get("defer_loading")
                    .and_then(serde_json::Value::as_bool)
                    == Some(true)
        }));
    }

    #[tokio::test]
    async fn anthropic_tool_schemas_include_carried_discovered_tools() {
        let mut provider = test_provider_config("https://api.anthropic.com/v1/messages".to_owned());
        provider.protocol = claude_core::ProviderProtocol::Anthropic;

        let carried = BTreeSet::from(["web_fetch".to_owned()]);
        let discovered = current_anthropic_tool_schemas(&provider, &[], &carried).await;

        assert!(discovered.iter().any(|tool| {
            tool.get("name").and_then(serde_json::Value::as_str) == Some("WebFetch")
                && tool
                    .get("defer_loading")
                    .and_then(serde_json::Value::as_bool)
                    == Some(true)
        }));
    }

    #[test]
    fn cache_control_preserves_existing_system_cache_markers() {
        let mut body = json!({
            "system": [
                {
                    "type": "text",
                    "text": "static",
                    "cache_control": {"type": "ephemeral", "scope": "global"}
                },
                {
                    "type": "text",
                    "text": "dynamic"
                }
            ]
        });

        add_stable_cache_control(&mut body, false);

        let system = body["system"].as_array().expect("system array");
        assert_eq!(system[0]["cache_control"]["scope"], "global");
        assert!(system[1].get("cache_control").is_none());
    }

    #[test]
    fn anthropic_headers_include_prompt_caching_scope_when_requested() {
        let mut provider = test_provider_config("https://api.anthropic.com/v1/messages".to_owned());
        provider.protocol = claude_core::ProviderProtocol::Anthropic;
        let body = json!({
            "system": [
                {
                    "type": "text",
                    "text": "static",
                    "cache_control": {"type": "ephemeral", "scope": "global"}
                }
            ]
        });

        let headers = build_headers(&provider, Some(&body), None).expect("headers");
        let beta = headers
            .get("anthropic-beta")
            .expect("anthropic-beta header")
            .to_str()
            .expect("anthropic-beta header should be utf8");
        assert!(beta.contains("prompt-caching-scope-2026-01-05"));
    }

    #[test]
    fn anthropic_headers_include_tool_search_beta_when_tool_search_is_active() {
        let mut provider = test_provider_config("https://api.anthropic.com/v1/messages".to_owned());
        provider.protocol = claude_core::ProviderProtocol::Anthropic;
        let body = json!({
            "tools": [
                {"name": "ToolSearch"},
                {"name": "WebFetch", "defer_loading": true}
            ]
        });

        let headers = build_headers(&provider, Some(&body), None).expect("headers");
        let beta = headers
            .get("anthropic-beta")
            .expect("anthropic-beta header")
            .to_str()
            .expect("anthropic-beta header should be utf8");
        assert!(beta.contains(crate::beta_headers::TOOL_SEARCH_BETA_1P));
    }

    #[test]
    fn anthropic_headers_include_output_config_and_fast_mode_betas() {
        let mut provider = test_provider_config("https://api.anthropic.com/v1/messages".to_owned());
        provider.protocol = claude_core::ProviderProtocol::Anthropic;
        provider.model = Some("minimax-m2.7".to_owned());
        let body = json!({
            "output_config": {
                "effort": "high",
                "task_budget": {"type": "tokens", "total": 32000},
                "format": {"type": "json_schema", "schema": {"type": "object"}}
            },
            "speed": "fast"
        });

        let headers = build_headers(&provider, Some(&body), None).expect("headers");
        let beta = headers
            .get("anthropic-beta")
            .expect("anthropic-beta header")
            .to_str()
            .expect("anthropic-beta header should be utf8");

        assert!(beta.contains(crate::beta_headers::CLAUDE_CODE_BETA));
        assert!(beta.contains(crate::beta_headers::EFFORT_BETA));
        assert!(beta.contains(crate::beta_headers::TASK_BUDGETS_BETA));
        assert!(beta.contains(crate::beta_headers::STRUCTURED_OUTPUTS_BETA));
        assert!(beta.contains(crate::beta_headers::FAST_MODE_BETA));
    }

    #[test]
    fn anthropic_headers_include_claude_code_identity_fields() {
        let mut provider = test_provider_config("https://api.anthropic.com/v1/messages".to_owned());
        provider.protocol = claude_core::ProviderProtocol::Anthropic;
        let request_context = crate::query_source::ProviderRequestContext::new(
            crate::query_source::QuerySource::Sdk,
            claude_core::SessionId::from("session-ctx"),
        );

        let headers = build_headers(&provider, None, Some(&request_context)).expect("headers");

        assert_eq!(
            headers.get("x-app").and_then(|h| h.to_str().ok()),
            Some("cli")
        );
        assert_eq!(
            headers
                .get("x-claude-code-session-id")
                .and_then(|h| h.to_str().ok()),
            Some("session-ctx")
        );
        let user_agent = headers
            .get(reqwest::header::USER_AGENT)
            .and_then(|h| h.to_str().ok())
            .expect("user-agent");
        assert!(user_agent.starts_with("claude-cli/"));
        assert!(headers.get("x-client-request-id").is_some());
    }

    #[test]
    fn anthropic_header_overrides_can_replace_reference_headers() {
        let mut provider = test_provider_config("https://api.anthropic.com/v1/messages".to_owned());
        provider.protocol = claude_core::ProviderProtocol::Anthropic;
        provider
            .request_header_overrides
            .insert("x-app".to_owned(), "custom-app".to_owned());
        provider
            .request_header_overrides
            .insert("User-Agent".to_owned(), "custom-agent".to_owned());

        let headers = build_headers(&provider, None, None).expect("headers");

        assert_eq!(
            headers.get("x-app").and_then(|h| h.to_str().ok()),
            Some("custom-app")
        );
        assert_eq!(
            headers
                .get(reqwest::header::USER_AGENT)
                .and_then(|h| h.to_str().ok()),
            Some("custom-agent")
        );
    }

    #[tokio::test]
    async fn anthropic_request_body_applies_context_controls() {
        let mut provider = test_provider_config("https://proxy.example.com/v1/messages".to_owned());
        provider.protocol = claude_core::ProviderProtocol::Anthropic;
        provider.model = Some("minimax-m2.7".to_owned());
        let request_context = crate::query_source::ProviderRequestContext::new(
            crate::query_source::QuerySource::Sdk,
            claude_core::SessionId::from("session-ctx"),
        )
        .with_effort(Some("high".to_owned()))
        .with_fast_mode(true)
        .with_task_budget(Some(crate::query_source::ProviderTaskBudget {
            total: 12_000,
            remaining: Some(8_000),
        }));

        let body = super::build_anthropic_request_body(
            &provider,
            &[ConversationEntry::user("hello")],
            &BTreeSet::new(),
            Some(&request_context),
            true,
        )
        .await;

        assert_eq!(body["stream"], true);
        assert_eq!(body["output_config"]["effort"], "high");
        assert!(body["output_config"].get("task_budget").is_none());
        assert!(body.get("speed").is_none());

        provider.base_url = Some("https://api.anthropic.com/v1/messages".to_owned());
        provider.model = Some("claude-sonnet-4-6-20260401".to_owned());
        let body = super::build_anthropic_request_body(
            &provider,
            &[ConversationEntry::user("hello")],
            &BTreeSet::new(),
            Some(&request_context),
            true,
        )
        .await;

        assert_eq!(body["output_config"]["task_budget"]["total"], 12_000);
        assert_eq!(body["output_config"]["task_budget"]["remaining"], 8_000);
        assert_eq!(body["speed"], "fast");
    }

    #[test]
    fn request_context_is_serialized_into_query_source_header() {
        let provider = test_provider_config("https://api.anthropic.com/v1/messages".to_owned());
        let request_context = crate::query_source::ProviderRequestContext::new(
            crate::query_source::QuerySource::SessionMemory,
            claude_core::SessionId::from("session-123"),
        )
        .with_agent_id(claude_core::AgentId::from("agent-456"));

        let headers = build_headers(&provider, None, Some(&request_context)).expect("headers");
        let query_source = headers
            .get(crate::query_source::QUERY_SOURCE_HEADER)
            .expect("query source header")
            .to_str()
            .expect("query source header should be utf8");
        assert_eq!(
            query_source,
            "source=session_memory;session_id=session-123;agent_id=agent-456"
        );
    }

    #[test]
    fn mock_provider_uses_latest_prompt() {
        let response = mock_response(&[ConversationEntry::user("hello world")]);
        assert!(response.text.contains("hello world"));
    }

    #[test]
    fn openai_messages_include_user_role() {
        let messages = to_openai_messages(&[ConversationEntry::user("ship it")]);
        assert_eq!(messages[0]["role"], "user");
    }

    #[test]
    fn openai_messages_replay_tool_use_with_provider_wire_names() {
        let mut assistant = ConversationEntry::assistant("");
        assistant.tool_calls = vec![ToolCall {
            id: "call-1".to_owned(),
            name: "bash_command".to_owned(),
            input: json!({"command":"pwd"}),
        }];

        let messages = to_openai_messages(&[assistant]);
        assert_eq!(messages[0]["tool_calls"][0]["function"]["name"], "Bash");
    }

    #[test]
    fn openai_messages_preserve_user_content_blocks() {
        let mut reminder = ConversationEntry::user(String::new());
        reminder.history_text = Some("__meta__".to_owned());
        reminder.content_blocks = vec![json!({
            "type": "text",
            "text": "<system-reminder>\nlate connect\n</system-reminder>",
        })];

        let messages = to_openai_messages(&[reminder]);
        let content = messages[0]["content"]
            .as_array()
            .expect("content blocks array");
        assert_eq!(content[0]["type"], "text");
        assert!(
            content[0]["text"]
                .as_str()
                .expect("text")
                .contains("system-reminder")
        );
    }

    #[test]
    fn anthropic_messages_emit_standalone_tool_results_when_no_user_prompt_follows() {
        let mut assistant = ConversationEntry::assistant("");
        assistant.tool_calls = vec![
            ToolCall {
                id: "call-1".to_owned(),
                name: "read_file".to_owned(),
                input: json!({"path":"src/main.rs"}),
            },
            ToolCall {
                id: "call-2".to_owned(),
                name: "read_file".to_owned(),
                input: json!({"path":"src/lib.rs"}),
            },
        ];

        let (_system, messages) = to_anthropic_messages(&[
            ConversationEntry::user("inspect"),
            assistant,
            ConversationEntry::tool("call-1", "read_file", "main", false),
            ConversationEntry::tool("call-2", "read_file", "lib", false),
        ]);

        assert_eq!(messages.len(), 3);
        assert_eq!(messages[1]["role"], "assistant");
        assert_eq!(messages[2]["role"], "user");
        let tool_results = messages[2]["content"]
            .as_array()
            .expect("tool results should be a content array");
        assert_eq!(tool_results.len(), 2);
        assert_eq!(tool_results[0]["type"], "tool_result");
        assert_eq!(tool_results[0]["tool_use_id"], "call-1");
        assert_eq!(tool_results[1]["tool_use_id"], "call-2");
        assert!(tool_results[0].get("is_error").is_none());
    }

    #[test]
    fn anthropic_messages_prepend_tool_results_to_following_user_message() {
        let mut assistant = ConversationEntry::assistant("");
        assistant.tool_calls = vec![ToolCall {
            id: "call-1".to_owned(),
            name: "replace_in_file".to_owned(),
            input: json!({"path":"src/main.rs"}),
        }];

        let (_system, messages) = to_anthropic_messages(&[
            ConversationEntry::user("inspect"),
            assistant,
            ConversationEntry::tool("call-1", "replace_in_file", "interrupted", true),
            ConversationEntry::user("continue the interrupted task"),
        ]);

        assert_eq!(messages.len(), 3);
        assert_eq!(messages[1]["role"], "assistant");
        assert_eq!(messages[2]["role"], "user");
        let content = messages[2]["content"]
            .as_array()
            .expect("merged user message should be a content array");
        assert_eq!(content[0]["type"], "tool_result");
        assert_eq!(content[0]["tool_use_id"], "call-1");
        assert_eq!(content[0]["is_error"], true);
        assert_eq!(content[1]["type"], "text");
        assert_eq!(content[1]["text"], "continue the interrupted task");
    }

    #[test]
    fn anthropic_messages_fold_interleaved_resume_prompts_into_one_user_turn() {
        let mut assistant = ConversationEntry::assistant("");
        assistant.tool_calls = vec![ToolCall {
            id: "call-1".to_owned(),
            name: "replace_in_file".to_owned(),
            input: json!({"path":"src/tests.rs"}),
        }];

        let (_system, messages) = to_anthropic_messages(&[
            ConversationEntry::user("original prompt"),
            assistant,
            ConversationEntry::user("resume prompt 1"),
            ConversationEntry::tool("call-1", "replace_in_file", "interrupted", true),
            ConversationEntry::user("resume prompt 2"),
            ConversationEntry::user("resume prompt 3"),
        ]);

        assert_eq!(messages.len(), 3);
        let content = messages[2]["content"]
            .as_array()
            .expect("folded user turn should be a content array");
        assert_eq!(content[0]["type"], "tool_result");
        assert_eq!(content[0]["tool_use_id"], "call-1");
        assert_eq!(content[1]["type"], "text");
        assert_eq!(content[1]["text"], "resume prompt 1\n");
        assert_eq!(content[2]["type"], "text");
        assert_eq!(content[2]["text"], "resume prompt 2\n");
        assert_eq!(content[3]["type"], "text");
        assert_eq!(content[3]["text"], "resume prompt 3");
    }

    #[test]
    fn anthropic_messages_preserve_system_content_blocks() {
        let mut system = ConversationEntry::system("flattened fallback");
        system.content_blocks = vec![
            json!({"type": "text", "text": "block 1"}),
            json!({"type": "text", "text": "block 2"}),
        ];

        let (system_blocks, _messages) = to_anthropic_messages(&[system]);

        assert_eq!(system_blocks.len(), 2);
        assert_eq!(system_blocks[0]["text"], "block 1");
        assert_eq!(system_blocks[1]["text"], "block 2");
    }

    #[test]
    fn provider_messages_drop_client_only_memory_saved_system_entries() {
        let mut memory_saved =
            ConversationEntry::system(r#"{"writtenPaths":["C:/Users/example/.claude/memory.md"]}"#);
        memory_saved.name = Some("memory_saved".to_owned());

        let openai_messages =
            to_openai_messages(&[ConversationEntry::system("sys"), memory_saved.clone()]);
        assert_eq!(openai_messages.len(), 1);
        assert_eq!(openai_messages[0]["content"], "sys");

        let (system_blocks, anthropic_messages) =
            to_anthropic_messages(&[ConversationEntry::system("sys"), memory_saved]);
        assert_eq!(system_blocks.len(), 1);
        assert_eq!(system_blocks[0]["text"], "sys");
        assert!(anthropic_messages.is_empty());
    }

    #[test]
    fn anthropic_messages_only_emit_is_error_for_failed_tool_results() {
        let mut assistant = ConversationEntry::assistant("");
        assistant.tool_calls = vec![ToolCall {
            id: "call-1".to_owned(),
            name: "read_file".to_owned(),
            input: json!({"path":"src/main.rs"}),
        }];

        let (_system, messages) = to_anthropic_messages(&[
            ConversationEntry::user("inspect"),
            assistant,
            ConversationEntry::tool("call-1", "read_file", "permission denied", true),
        ]);

        let tool_results = messages[2]["content"]
            .as_array()
            .expect("tool results should be a content array");
        assert_eq!(tool_results[0]["is_error"], true);
    }

    #[test]
    fn anthropic_tool_results_preserve_structured_content_blocks() {
        let mut assistant = ConversationEntry::assistant("");
        assistant.tool_calls = vec![ToolCall {
            id: "call-1".to_owned(),
            name: "tool_search".to_owned(),
            input: json!({"query":"select:read_mcp_resource"}),
        }];

        let mut tool = ConversationEntry::tool("call-1", "tool_search", "structured", false);
        tool.content_blocks = vec![json!({
            "type": "tool_reference",
            "tool_name": "read_mcp_resource",
        })];

        let (_system, messages) =
            to_anthropic_messages(&[ConversationEntry::user("load tool"), assistant, tool]);
        let tool_results = messages[2]["content"]
            .as_array()
            .expect("tool results should be a content array");
        assert_eq!(tool_results[0]["type"], "tool_result");
        assert!(tool_results[0]["content"].is_array());
        assert_eq!(tool_results[0]["content"][0]["type"], "tool_reference");
        assert_eq!(
            tool_results[0]["content"][0]["tool_name"],
            "read_mcp_resource"
        );
    }

    #[test]
    fn anthropic_groups_tool_result_with_follow_up_user_blocks() {
        let mut assistant = ConversationEntry::assistant("");
        assistant.tool_calls = vec![ToolCall {
            id: "call-1".to_owned(),
            name: "read_file".to_owned(),
            input: json!({"path":"src/main.rs"}),
        }];
        let tool = ConversationEntry::tool("call-1", "read_file", "ok", false);
        let follow_up = ConversationEntry::user_with_content_blocks(vec![
            json!({
                "type": "text",
                "text": "Approved. Proceed with the change.",
            }),
            json!({
                "type": "text",
                "text": "Extra UI note.",
            }),
        ]);

        let (_system, messages) = to_anthropic_messages(&[
            ConversationEntry::user("inspect"),
            assistant,
            tool,
            follow_up,
        ]);
        let content = messages[2]["content"]
            .as_array()
            .expect("tool result turn should be grouped");
        assert_eq!(content[0]["type"], "tool_result");
        assert_eq!(content[1]["type"], "text");
        assert_eq!(content[1]["text"], "Approved. Proceed with the change.");
        assert_eq!(content[2]["type"], "text");
        assert_eq!(content[2]["text"], "Extra UI note.");
    }

    #[test]
    fn anthropic_tool_reference_messages_relocate_text_siblings() {
        let relocated = relocate_tool_reference_siblings(vec![
            json!({
                "role": "user",
                "content": [
                    {
                        "type": "tool_result",
                        "tool_use_id": "call-1",
                        "content": [
                            {"type": "tool_reference", "tool_name": "web_fetch"}
                        ]
                    },
                    {"type": "text", "text": "system reminder"},
                ],
            }),
            json!({
                "role": "assistant",
                "content": [{"type": "text", "text": "continue"}],
            }),
            json!({
                "role": "user",
                "content": [
                    {
                        "type": "tool_result",
                        "tool_use_id": "call-2",
                        "content": "ok"
                    }
                ],
            }),
        ]);

        let source_content = relocated[0]["content"].as_array().expect("source content");
        assert_eq!(source_content.len(), 1);
        assert_eq!(source_content[0]["type"], "tool_result");

        let target_content = relocated[2]["content"].as_array().expect("target content");
        assert_eq!(target_content.len(), 2);
        assert_eq!(target_content[0]["type"], "tool_result");
        assert_eq!(target_content[1]["type"], "text");
        assert_eq!(target_content[1]["text"], "system reminder");
    }

    #[test]
    fn anthropic_tool_reference_messages_inject_turn_boundary_when_relocation_disabled() {
        let injected = inject_tool_reference_turn_boundary_siblings(vec![json!({
            "role": "user",
            "content": [
                {
                    "type": "tool_result",
                    "tool_use_id": "call-1",
                    "content": [
                        {"type": "tool_reference", "tool_name": "web_fetch"}
                    ]
                }
            ],
        })]);

        let content = injected[0]["content"].as_array().expect("content array");
        assert_eq!(content.len(), 2);
        assert_eq!(content[1]["type"], "text");
        assert_eq!(content[1]["text"], "Tool loaded.");
    }

    #[test]
    fn openai_response_parser_handles_success() {
        let raw = r#"{"choices":[{"message":{"content":"hello"}}],"usage":{"prompt_tokens":1,"completion_tokens":2}}"#;
        let parsed = parse_openai_response(200, raw.to_owned());
        assert!(parsed.is_ok());
        let parsed = parsed.unwrap_or_else(|error| panic!("parse failed: {error}"));
        assert_eq!(parsed.text, "hello");
        assert_eq!(parsed.usage.output_tokens, 2);
    }

    #[test]
    fn openai_response_parser_rejects_malformed_tool_arguments() {
        let raw = r#"{"choices":[{"message":{"tool_calls":[{"id":"call-1","type":"function","function":{"name":"bash","arguments":"not json{"}}]}}]}"#;
        let error = parse_openai_response(200, raw.to_owned())
            .expect_err("malformed tool arguments must fail response parsing");
        assert!(
            error
                .to_string()
                .contains("invalid JSON arguments for tool call `bash`"),
            "{error:#}"
        );
    }

    #[test]
    fn openai_response_parser_captures_request_id() {
        let raw = r#"{"id":"chatcmpl-123","choices":[{"message":{"content":"hello"}}],"usage":{"prompt_tokens":1,"completion_tokens":2}}"#;
        let parsed =
            parse_openai_response(200, raw.to_owned()).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(parsed.request_id.as_deref(), Some("chatcmpl-123"));
    }

    #[test]
    fn anthropic_response_parser_captures_request_id() {
        let raw = r#"{"id":"msg_123","type":"message","role":"assistant","content":[{"type":"text","text":"hello"}],"usage":{"input_tokens":3,"output_tokens":4},"stop_reason":"end_turn"}"#;
        let parsed = parse_anthropic_response(200, raw.to_owned())
            .unwrap_or_else(|error| panic!("parse failed: {error}"));
        assert_eq!(parsed.request_id.as_deref(), Some("msg_123"));
        assert_eq!(parsed.text, "hello");
    }

    #[test]
    fn anthropic_request_metadata_is_serialized_into_user_id() {
        let mut provider = test_provider_config("https://api.anthropic.com/v1/messages".to_owned());
        provider.protocol = claude_core::ProviderProtocol::Anthropic;
        provider
            .request_metadata
            .insert("session_id".to_owned(), "session-123".to_owned());
        provider
            .request_metadata
            .insert("device_id".to_owned(), "test-device".to_owned());
        let mut body = json!({
            "model": "claude-test",
            "messages": [],
        });

        let request_context = crate::query_source::ProviderRequestContext::new(
            crate::query_source::QuerySource::Agent,
            claude_core::SessionId::from("session-ctx"),
        );

        apply_anthropic_request_metadata(&mut body, &provider, Some(&request_context));

        let metadata = body
            .get("metadata")
            .and_then(|value| value.get("user_id"))
            .and_then(serde_json::Value::as_str)
            .expect("metadata.user_id");
        let parsed = serde_json::from_str::<serde_json::Value>(metadata)
            .unwrap_or_else(|error| panic!("invalid metadata json: {error}"));
        assert_eq!(parsed["session_id"], "session-123");
        assert_eq!(parsed["device_id"], "test-device");
    }

    #[tokio::test]
    async fn request_context_overrides_anthropic_model_and_max_tokens() {
        let mut provider = test_provider_config("https://api.anthropic.com/v1/messages".to_owned());
        provider.protocol = claude_core::ProviderProtocol::Anthropic;
        provider.model = Some("primary-model".to_owned());
        provider.max_output_tokens = 4_096;
        let request_context = crate::query_source::ProviderRequestContext::new(
            crate::query_source::QuerySource::Sdk,
            claude_core::SessionId::from("session-ctx"),
        )
        .with_model_override(Some("fallback-model".to_owned()))
        .with_max_output_tokens(Some(16_384));

        let effective = super::provider_for_request(&provider, Some(&request_context));
        let body = super::build_anthropic_request_body(
            &effective,
            &[ConversationEntry::user("hello")],
            &BTreeSet::new(),
            Some(&request_context),
            false,
        )
        .await;

        assert_eq!(body["model"], "fallback-model");
        assert_eq!(body["max_tokens"], 16_384);
    }

    #[tokio::test]
    async fn request_context_overrides_openai_model_and_max_tokens() {
        let mut provider = test_provider_config("https://example.invalid/v1/chat".to_owned());
        provider.model = Some("primary-model".to_owned());
        provider.max_output_tokens = 1_024;
        let request_context = crate::query_source::ProviderRequestContext::new(
            crate::query_source::QuerySource::Sdk,
            claude_core::SessionId::from("session-ctx"),
        )
        .with_model_override(Some("fallback-model".to_owned()))
        .with_max_output_tokens(Some(8_192));

        let effective = super::provider_for_request(&provider, Some(&request_context));
        let body = super::build_openai_request_body(
            &effective,
            &[ConversationEntry::user("hello")],
            &BTreeSet::new(),
            true,
        )
        .await;

        assert_eq!(body["model"], "fallback-model");
        assert_eq!(body["max_tokens"], 8_192);
        assert_eq!(body["stream"], true);
    }

    #[tokio::test]
    async fn provider_retries_retryable_status_then_succeeds() {
        async fn handler(
            State(attempts): State<Arc<AtomicUsize>>,
        ) -> (
            axum::http::StatusCode,
            [(&'static str, &'static str); 1],
            Json<serde_json::Value>,
        ) {
            let attempt = attempts.fetch_add(1, Ordering::SeqCst);
            if attempt == 0 {
                return (
                    axum::http::StatusCode::TOO_MANY_REQUESTS,
                    [("connection", "close")],
                    Json(json!({"error": {"message": "slow down"}})),
                );
            }
            (
                axum::http::StatusCode::OK,
                [("connection", "close")],
                Json(json!({
                    "choices": [{"message": {"content": "retried ok"}}],
                    "usage": {"prompt_tokens": 3, "completion_tokens": 4}
                })),
            )
        }

        let attempts = Arc::new(AtomicUsize::new(0));
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap_or_else(|error| panic!("listener bind failed: {error}"));
        let address = listener
            .local_addr()
            .unwrap_or_else(|error| panic!("local addr failed: {error}"));
        let server_attempts = Arc::clone(&attempts);
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new()
                    .route("/chat/completions", post(handler))
                    .with_state(server_attempts),
            )
            .await
            .unwrap_or_else(|error| panic!("server failed: {error}"));
        });

        let client = ProviderClient::new().unwrap_or_else(|error| panic!("client failed: {error}"));
        let response = client
            .complete(
                &test_provider_config(format!("http://{address}/chat/completions")),
                &[ConversationEntry::user("hello")],
            )
            .await
            .unwrap_or_else(|error| panic!("completion failed: {error:?}"));

        server.abort();
        assert_eq!(response.text, "retried ok");
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn provider_retries_529_then_succeeds() {
        async fn handler(
            State(attempts): State<Arc<AtomicUsize>>,
        ) -> (
            axum::http::StatusCode,
            [(&'static str, &'static str); 1],
            Json<serde_json::Value>,
        ) {
            let attempt = attempts.fetch_add(1, Ordering::SeqCst);
            if attempt == 0 {
                return (
                    axum::http::StatusCode::from_u16(529).expect("529 status"),
                    [("connection", "close")],
                    Json(json!({"error": {"message": "overloaded"}})),
                );
            }
            (
                axum::http::StatusCode::OK,
                [("connection", "close")],
                Json(json!({
                    "choices": [{"message": {"content": "retried 529 ok"}}],
                    "usage": {"prompt_tokens": 3, "completion_tokens": 4}
                })),
            )
        }

        let attempts = Arc::new(AtomicUsize::new(0));
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap_or_else(|error| panic!("listener bind failed: {error}"));
        let address = listener
            .local_addr()
            .unwrap_or_else(|error| panic!("local addr failed: {error}"));
        let server_attempts = Arc::clone(&attempts);
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new()
                    .route("/chat/completions", post(handler))
                    .with_state(server_attempts),
            )
            .await
            .unwrap_or_else(|error| panic!("server failed: {error}"));
        });

        let client = ProviderClient::new().unwrap_or_else(|error| panic!("client failed: {error}"));
        let response = client
            .complete(
                &test_provider_config(format!("http://{address}/chat/completions")),
                &[ConversationEntry::user("hello")],
            )
            .await
            .unwrap_or_else(|error| panic!("completion failed: {error:?}"));

        server.abort();
        assert_eq!(response.text, "retried 529 ok");
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn provider_does_not_retry_non_retryable_status() {
        async fn handler(
            State(attempts): State<Arc<AtomicUsize>>,
        ) -> (
            axum::http::StatusCode,
            [(&'static str, &'static str); 1],
            Json<serde_json::Value>,
        ) {
            attempts.fetch_add(1, Ordering::SeqCst);
            (
                axum::http::StatusCode::UNAUTHORIZED,
                [("connection", "close")],
                Json(json!({"error": {"message": "bad api key"}})),
            )
        }

        let attempts = Arc::new(AtomicUsize::new(0));
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap_or_else(|error| panic!("listener bind failed: {error}"));
        let address = listener
            .local_addr()
            .unwrap_or_else(|error| panic!("local addr failed: {error}"));
        let server_attempts = Arc::clone(&attempts);
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new()
                    .route("/chat/completions", post(handler))
                    .with_state(server_attempts),
            )
            .await
            .unwrap_or_else(|error| panic!("server failed: {error}"));
        });

        let client = ProviderClient::new().unwrap_or_else(|error| panic!("client failed: {error}"));
        let error = client
            .complete(
                &test_provider_config(format!("http://{address}/chat/completions")),
                &[ConversationEntry::user("hello")],
            )
            .await
            .expect_err("request should fail");

        server.abort();
        let provider_error = super::extract_provider_error(&error)
            .unwrap_or_else(|| panic!("provider error missing from chain: {error:#}"));
        assert_eq!(provider_error.status_code, Some(401));
        assert_eq!(
            provider_error.category,
            super::ErrorCategory::Authentication
        );
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    // ── Error classification tests ────────────────────────────────────

    #[test]
    fn classify_429_as_rate_limit() {
        let err = super::classify_provider_error(429, "rate limited", "test-provider");
        assert_eq!(err.category, super::ErrorCategory::RateLimit);
        assert_eq!(err.recovery, super::RecoveryAction::Retry);
        assert!(super::is_retryable(&err));
    }

    #[test]
    fn classify_401_as_authentication() {
        let err = super::classify_provider_error(401, "invalid api key", "test-provider");
        assert_eq!(err.category, super::ErrorCategory::Authentication);
        assert_eq!(err.recovery, super::RecoveryAction::FixConfig);
        assert!(!super::is_retryable(&err));
    }

    #[test]
    fn classify_400_prompt_too_long() {
        let err = super::classify_provider_error(
            400,
            "prompt is too long: maximum context length exceeded",
            "test-provider",
        );
        assert_eq!(err.category, super::ErrorCategory::PromptTooLong);
        assert_eq!(err.recovery, super::RecoveryAction::CompactAndRetry);
        assert!(super::is_prompt_too_long(&err));
        assert!(super::is_retryable(&err));
    }

    #[test]
    fn classify_400_context_length_exceeded() {
        let err = super::classify_provider_error(400, "context_length_exceeded", "test-provider");
        assert_eq!(err.category, super::ErrorCategory::PromptTooLong);
        assert!(super::is_prompt_too_long(&err));
    }

    #[test]
    fn classify_500_as_server_error() {
        let err = super::classify_provider_error(500, "internal server error", "test-provider");
        assert_eq!(err.category, super::ErrorCategory::ServerError);
        assert_eq!(err.recovery, super::RecoveryAction::Retry);
        assert!(super::is_retryable(&err));
    }

    #[test]
    fn classify_503_as_server_error() {
        let err = super::classify_provider_error(503, "service unavailable", "test-provider");
        assert_eq!(err.category, super::ErrorCategory::ServerError);
        assert!(super::is_retryable(&err));
    }

    #[test]
    fn classify_404_as_model_not_found() {
        let err = super::classify_provider_error(404, "model not found", "test-provider");
        assert_eq!(err.category, super::ErrorCategory::ModelNotFound);
        assert_eq!(err.recovery, super::RecoveryAction::FixConfig);
    }

    #[test]
    fn classify_402_as_quota_exceeded() {
        let err = super::classify_provider_error(402, "insufficient quota", "test-provider");
        assert_eq!(err.category, super::ErrorCategory::QuotaExceeded);
        assert_eq!(err.recovery, super::RecoveryAction::Failover);
    }

    #[test]
    fn classify_network_timeout() {
        let err = super::classify_network_error("connection timed out", "test-provider");
        assert_eq!(err.category, super::ErrorCategory::Timeout);
        assert_eq!(err.recovery, super::RecoveryAction::Retry);
        assert!(super::is_retryable(&err));
    }

    #[test]
    fn classify_network_dns_error() {
        let err = super::classify_network_error("dns resolve failed", "test-provider");
        assert_eq!(err.category, super::ErrorCategory::Network);
        assert_eq!(err.recovery, super::RecoveryAction::FixConfig);
    }

    #[test]
    fn classify_400_generic_as_invalid_request() {
        let err = super::classify_provider_error(400, "invalid parameter", "test-provider");
        assert_eq!(err.category, super::ErrorCategory::InvalidRequest);
        assert_eq!(err.recovery, super::RecoveryAction::Abort);
    }

    // --- apply_anthropic_thinking_options budget clamping ---

    #[test]
    fn thinking_budget_is_clamped_to_max_tokens_minus_one() {
        let mut provider = test_provider_config("https://api.anthropic.com/v1/messages".to_owned());
        provider.protocol = claude_core::ProviderProtocol::Anthropic;
        provider.max_output_tokens = 8_192;
        provider.thinking_budget = Some(16_384); // exceeds max_output_tokens

        let mut body = json!({
            "max_tokens": 8_192u64,
        });

        super::apply_anthropic_thinking_options(&mut body, &provider);

        let budget = body["thinking"]["budget_tokens"].as_u64().unwrap();
        assert_eq!(budget, 8_191, "budget should be clamped to max_tokens - 1");
        assert_eq!(
            body["max_tokens"].as_u64().unwrap(),
            8_192,
            "max_tokens should remain unchanged since budget < max_tokens"
        );
    }

    #[test]
    fn thinking_budget_unchanged_when_already_below_max_tokens() {
        let mut provider = test_provider_config("https://api.anthropic.com/v1/messages".to_owned());
        provider.protocol = claude_core::ProviderProtocol::Anthropic;
        provider.max_output_tokens = 8_192;
        provider.thinking_budget = Some(5_000);

        let mut body = json!({
            "max_tokens": 8_192u64,
        });

        super::apply_anthropic_thinking_options(&mut body, &provider);

        let budget = body["thinking"]["budget_tokens"].as_u64().unwrap();
        assert_eq!(
            budget, 5_000,
            "budget should be unchanged when already below max_tokens"
        );
    }

    #[test]
    fn thinking_budget_equal_to_max_tokens_is_clamped() {
        let mut provider = test_provider_config("https://api.anthropic.com/v1/messages".to_owned());
        provider.protocol = claude_core::ProviderProtocol::Anthropic;
        provider.max_output_tokens = 8_192;
        provider.thinking_budget = Some(8_192); // exactly equal

        let mut body = json!({
            "max_tokens": 8_192u64,
        });

        super::apply_anthropic_thinking_options(&mut body, &provider);

        let budget = body["thinking"]["budget_tokens"].as_u64().unwrap();
        assert_eq!(
            budget, 8_191,
            "budget equal to max_tokens must be clamped to max_tokens - 1"
        );
    }

    #[test]
    fn thinking_budget_clamp_with_max_tokens_one() {
        let mut provider = test_provider_config("https://api.anthropic.com/v1/messages".to_owned());
        provider.protocol = claude_core::ProviderProtocol::Anthropic;
        provider.max_output_tokens = 1;
        provider.thinking_budget = Some(10_000);

        let mut body = json!({
            "max_tokens": 1u64,
        });

        super::apply_anthropic_thinking_options(&mut body, &provider);

        let budget = body["thinking"]["budget_tokens"].as_u64().unwrap();
        assert_eq!(
            budget, 0,
            "budget should clamp to 0 when max_tokens is 1 (1 - 1 = 0)"
        );
        // The safety net should still kick in and raise max_tokens.
        assert!(body["max_tokens"].as_u64().unwrap() > budget);
    }
}
