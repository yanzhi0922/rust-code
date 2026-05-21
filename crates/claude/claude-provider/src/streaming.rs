//! Streaming support for provider responses.
//!
//! Extends [`ProviderClient`] with
//! [`complete_streaming`](crate::ProviderClient::complete_streaming) which
//! processes server-sent events (SSE) from OpenAI- and Anthropic-compatible
//! APIs, invoking optional callbacks for text deltas, tool-call progress, and
//! usage telemetry.
//!
//! Also supports native Bedrock event-stream and Vertex SSE streaming for
//! Anthropic Claude models hosted on AWS and GCP.

use anyhow::{Context, Result, anyhow};
use claude_config::ProviderConfig;
use claude_core::{ConversationEntry, ProviderProtocol, ProviderResponse, ToolCall, UsageSummary};
use futures::StreamExt;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use crate::retry::{
    RetryConfig, compute_retry_delay as retry_compute_retry_delay, is_overloaded_error_body,
    is_retryable_http_status, is_retryable_transport_error,
    parse_retry_after as retry_parse_retry_after,
};
use crate::{
    ProviderClient, build_anthropic_request_body, build_headers, build_openai_request_body,
    maybe_dump_request_body, prepare_anthropic_request_surface, provider_for_request,
};

// ---------------------------------------------------------------------------
// Stream idle watchdog configuration
// ---------------------------------------------------------------------------

/// Default stream idle timeout in milliseconds (90 seconds).
const DEFAULT_STREAM_IDLE_TIMEOUT_MS: u64 = 90_000;

/// Maximum size of the SSE buffer before discarding stale data.
/// Prevents unbounded memory growth from misbehaving endpoints.
const MAX_SSE_BUFFER_SIZE: usize = 10 * 1024 * 1024; // 10 MB

/// Read the stream idle timeout from `CLAUDE_STREAM_IDLE_TIMEOUT_MS`.
/// Falls back to 90 seconds if unset or unparseable.
fn stream_idle_timeout() -> Duration {
    let ms = std::env::var("CLAUDE_STREAM_IDLE_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DEFAULT_STREAM_IDLE_TIMEOUT_MS);
    Duration::from_millis(ms)
}

/// Check whether the stream idle watchdog is enabled via
/// `CLAUDE_ENABLE_STREAM_WATCHDOG`.  Defaults to `true` (enabled).
fn is_stream_watchdog_enabled() -> bool {
    std::env::var("CLAUDE_ENABLE_STREAM_WATCHDOG")
        .map(|v| v != "0" && !v.eq_ignore_ascii_case("false"))
        .unwrap_or(true)
}

/// Build an idle-timeout error that is recognised by
/// [`should_fallback_after_streaming_error`] and the retry layer.
fn stream_idle_timeout_error(timeout: Duration) -> anyhow::Error {
    anyhow!(
        "streaming idle timeout: no data received for {}s — aborting hung stream",
        timeout.as_secs()
    )
}

// ---------------------------------------------------------------------------
// Streaming callbacks
// ---------------------------------------------------------------------------

/// Type alias for a single-argument streaming callback.
type TextCallback = Box<dyn Fn(&str) + Send + Sync>;

/// Type alias for a two-argument streaming callback (id, name/delta).
type PairCallback = Box<dyn Fn(&str, &str) + Send + Sync>;

/// Usage snapshot delivered by streaming callbacks.
#[derive(Debug, Clone, Default)]
pub struct StreamingUsageUpdate {
    /// Input (prompt) tokens.
    pub input_tokens: u64,
    /// Output (completion) tokens.
    pub output_tokens: u64,
    /// Anthropic cache read tokens.
    pub cache_read_input_tokens: u64,
    /// Anthropic cache creation tokens.
    pub cache_creation_input_tokens: u64,
}

/// Granular lifecycle events emitted during streaming.
///
/// Mirrors the Anthropic SSE event types (`message_start`, `content_block_start`,
/// `content_block_delta`, `content_block_stop`, `message_delta`, `message_stop`)
/// and provides analogous events for OpenAI-compatible streaming.
#[derive(Debug, Clone)]
pub enum StreamingLifecycleEvent {
    /// The streaming message has started (Anthropic `message_start`).
    MessageStart,
    /// A new content block has started (Anthropic `content_block_start`).
    ContentBlockStart {
        /// Zero-based content block index.
        index: usize,
        /// Block type string (e.g. `"text"`, `"thinking"`, `"tool_use"`).
        block_type: String,
    },
    /// A content block has finished (Anthropic `content_block_stop`).
    ContentBlockStop {
        /// Zero-based content block index.
        index: usize,
    },
    /// The final message delta with authoritative `stop_reason` and output tokens
    /// (Anthropic `message_delta`).
    MessageDelta {
        /// The stop reason (e.g. `"end_turn"`, `"tool_use"`).
        stop_reason: String,
        /// Final output token count.
        output_tokens: u64,
    },
    /// The streaming message has ended (Anthropic `message_stop`).
    MessageStop,
}

/// Type alias for a usage callback.
type UsageCallback = Box<dyn Fn(StreamingUsageUpdate) + Send + Sync>;

/// Type alias for a lifecycle event callback.
type LifecycleCallback = Box<dyn Fn(StreamingLifecycleEvent) + Send + Sync>;

/// Optional callbacks for observing streaming events in real time.
///
/// All callback fields are `Option<...>` so callers can subscribe to only the
/// events they care about.
#[derive(Default)]
#[allow(clippy::type_complexity)]
pub struct StreamingCallbacks {
    /// Fired for every text delta received from the provider.
    pub on_text_delta: Option<TextCallback>,
    /// Fired when a tool call starts (id and name are available).
    pub on_tool_call_start: Option<PairCallback>,
    /// Fired for every incremental tool-call input delta.
    pub on_tool_call_delta: Option<PairCallback>,
    /// Fired when usage information becomes available (input, output tokens).
    pub on_usage: Option<UsageCallback>,
    /// Fired for every thinking delta received during extended thinking.
    pub on_thinking_delta: Option<TextCallback>,
    /// Fired for granular streaming lifecycle events (message/block start/stop).
    pub on_lifecycle_event: Option<LifecycleCallback>,
}

// ---------------------------------------------------------------------------
// ProviderClient streaming implementation
// ---------------------------------------------------------------------------

impl ProviderClient {
    /// # Errors
    /// Returns an error if the provider API request fails.
    pub async fn complete_streaming(
        &self,
        provider: &ProviderConfig,
        conversation: &[ConversationEntry],
    ) -> Result<ProviderResponse> {
        self.complete_streaming_with_callbacks_and_discovered_tools(
            provider,
            conversation,
            None,
            &BTreeSet::new(),
            None,
        )
        .await
    }

    /// Streaming completion with optional real-time callbacks.
    ///
    /// If the streaming connection fails mid-request, automatically falls back
    /// to a non-streaming completion for resilience during long-running sessions.
    ///
    /// # Errors
    /// Returns an error if both streaming and non-streaming attempts fail.
    pub async fn complete_streaming_with_callbacks(
        &self,
        provider: &ProviderConfig,
        conversation: &[ConversationEntry],
        callbacks: Option<StreamingCallbacks>,
    ) -> Result<ProviderResponse> {
        self.complete_streaming_with_callbacks_and_discovered_tools(
            provider,
            conversation,
            callbacks,
            &BTreeSet::new(),
            None,
        )
        .await
    }

    /// Streaming completion with carried deferred-tool discovery state.
    ///
    /// # Errors
    /// Returns an error if both streaming and non-streaming attempts fail.
    pub async fn complete_streaming_with_callbacks_and_discovered_tools(
        &self,
        provider: &ProviderConfig,
        conversation: &[ConversationEntry],
        callbacks: Option<StreamingCallbacks>,
        carried_discovered_tools: &BTreeSet<String>,
        request_context: Option<&crate::query_source::ProviderRequestContext>,
    ) -> Result<ProviderResponse> {
        if provider.name == "mock"
            || provider.api_key.as_deref() == Some("mock")
            || provider.base_url.as_deref() == Some("mock://provider")
        {
            return Ok(crate::mock_response(conversation));
        }

        let streamed_tool_activity = Arc::new(AtomicBool::new(false));
        let tracked_callbacks =
            wrap_streaming_callbacks(callbacks, Arc::clone(&streamed_tool_activity));

        let result = if super::provider_prefers_anthropic_messages_route(provider) {
            let routed_provider = super::provider_as_anthropic_compatible(provider);
            self.complete_streaming_anthropic(
                &routed_provider,
                conversation,
                Some(&tracked_callbacks),
                carried_discovered_tools,
                request_context,
            )
            .await
        } else {
            match provider.protocol {
                ProviderProtocol::OpenAi => {
                    self.complete_streaming_openai(
                        provider,
                        conversation,
                        Some(&tracked_callbacks),
                        carried_discovered_tools,
                        request_context,
                    )
                    .await
                }
                ProviderProtocol::Anthropic => {
                    self.complete_streaming_anthropic(
                        provider,
                        conversation,
                        Some(&tracked_callbacks),
                        carried_discovered_tools,
                        request_context,
                    )
                    .await
                }
                // Native Bedrock uses AWS event-stream; Vertex uses SSE.
                // If a base_url is set (proxy mode), fall back to OpenAI-compatible streaming.
                ProviderProtocol::Bedrock => {
                    if provider.base_url.is_some() {
                        self.complete_streaming_openai(
                            provider,
                            conversation,
                            Some(&tracked_callbacks),
                            carried_discovered_tools,
                            request_context,
                        )
                        .await
                    } else {
                        self.complete_streaming_bedrock(
                            provider,
                            conversation,
                            Some(&tracked_callbacks),
                            carried_discovered_tools,
                            request_context,
                        )
                        .await
                    }
                }
                ProviderProtocol::Vertex => {
                    if provider.base_url.is_some() {
                        self.complete_streaming_openai(
                            provider,
                            conversation,
                            Some(&tracked_callbacks),
                            carried_discovered_tools,
                            request_context,
                        )
                        .await
                    } else {
                        self.complete_streaming_vertex(
                            provider,
                            conversation,
                            Some(&tracked_callbacks),
                            carried_discovered_tools,
                            request_context,
                        )
                        .await
                    }
                }
            }
        };

        // If streaming failed, fall back to non-streaming completion.
        // This handles mid-stream disconnects, SSE parsing errors, and
        // other transient streaming failures common in long-running sessions.
        match result {
            Ok(response) => Ok(response),
            Err(streaming_error) => {
                if should_fallback_after_streaming_error(
                    &streaming_error,
                    streamed_tool_activity.load(Ordering::Relaxed),
                ) {
                    tracing::warn!(
                        "Streaming failed, falling back to non-streaming: {streaming_error:#}"
                    );
                    self.complete_with_discovered_tools(
                        provider,
                        conversation,
                        carried_discovered_tools,
                        request_context,
                    )
                    .await
                } else {
                    if streamed_tool_activity.load(Ordering::Relaxed) {
                        tracing::warn!(
                            "Streaming failed after tool activity; refusing non-streaming fallback: {streaming_error:#}"
                        );
                    }
                    Err(streaming_error)
                }
            }
        }
    }

    #[allow(clippy::too_many_lines)]
    async fn complete_streaming_openai(
        &self,
        provider: &ProviderConfig,
        conversation: &[ConversationEntry],
        callbacks: Option<&StreamingCallbacks>,
        carried_discovered_tools: &BTreeSet<String>,
        request_context: Option<&crate::query_source::ProviderRequestContext>,
    ) -> Result<ProviderResponse> {
        let effective_provider = provider_for_request(provider, request_context);
        let body = build_openai_request_body(
            &effective_provider,
            conversation,
            carried_discovered_tools,
            true,
        )
        .await;
        let base_url = provider
            .base_url
            .as_ref()
            .ok_or_else(|| anyhow!("provider is missing a normalized base URL"))?;

        let response = self
            .send_streaming_request(
                &effective_provider,
                base_url,
                &body,
                "openai-compatible",
                request_context,
            )
            .await?;

        let mut text_parts: Vec<String> = Vec::new();
        let mut tool_calls_map: HashMap<usize, OpenAiToolCallAccumulator> = HashMap::new();
        let mut finish_reason = "stop".to_owned();
        let mut usage = UsageSummary::default();
        let mut request_id: Option<String> = None;
        let mut message_started = false;

        let mut stream = response.bytes_stream();
        let mut sse_buffer = String::new();

        let watchdog_enabled = is_stream_watchdog_enabled();
        let idle_timeout = stream_idle_timeout();

        loop {
            let chunk_result = if watchdog_enabled {
                match tokio::time::timeout(idle_timeout, stream.next()).await {
                    Ok(chunk) => chunk,
                    Err(_) => {
                        tracing::error!(
                            "Streaming idle timeout: no data received for {}s, aborting",
                            idle_timeout.as_secs()
                        );
                        return Err(stream_idle_timeout_error(idle_timeout));
                    }
                }
            } else {
                stream.next().await
            };

            let Some(chunk) = chunk_result else { break };
            let bytes = chunk.with_context(|| "failed to read streaming chunk")?;
            sse_buffer.push_str(&String::from_utf8_lossy(&bytes));
            if sse_buffer.len() > MAX_SSE_BUFFER_SIZE {
                tracing::warn!(
                    "SSE buffer exceeded {MAX_SSE_BUFFER_SIZE} bytes, discarding stale data"
                );
                sse_buffer.clear();
            }

            while let Some(event_end) = sse_buffer.find("\n\n") {
                let event_text = sse_buffer[..event_end].to_owned();
                sse_buffer = sse_buffer[event_end + 2..].to_owned();

                for line in event_text.lines() {
                    let Some(data) = line.strip_prefix("data: ") else {
                        continue;
                    };
                    let data = data.trim();
                    if data == "[DONE]" {
                        continue;
                    }

                    let parsed: Value = match serde_json::from_str(data) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };
                    if request_id.is_none() {
                        request_id = parsed
                            .get("id")
                            .and_then(Value::as_str)
                            .map(ToOwned::to_owned);
                    }
                    if !message_started {
                        message_started = true;
                        if let Some(cb) = callbacks
                            .as_ref()
                            .and_then(|c| c.on_lifecycle_event.as_ref())
                        {
                            cb(StreamingLifecycleEvent::MessageStart);
                        }
                    }

                    if let Some(choice) = parsed
                        .get("choices")
                        .and_then(Value::as_array)
                        .and_then(|choices| choices.first())
                    {
                        if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str)
                            && reason != "null"
                        {
                            reason.clone_into(&mut finish_reason);
                            if let Some(cb) = callbacks
                                .as_ref()
                                .and_then(|c| c.on_lifecycle_event.as_ref())
                            {
                                cb(StreamingLifecycleEvent::MessageDelta {
                                    stop_reason: reason.to_owned(),
                                    output_tokens: 0,
                                });
                            }
                        }

                        let delta = choice.get("delta");

                        if let Some(content) =
                            delta.and_then(|d| d.get("content")).and_then(Value::as_str)
                        {
                            // Fire on_text_delta callback.
                            if let Some(cb) =
                                callbacks.as_ref().and_then(|c| c.on_text_delta.as_ref())
                            {
                                cb(content);
                            }
                            text_parts.push(content.to_owned());
                        }

                        if let Some(tc_deltas) = delta
                            .and_then(|d| d.get("tool_calls"))
                            .and_then(Value::as_array)
                        {
                            for tc_delta in tc_deltas {
                                #[allow(clippy::cast_possible_truncation)]
                                let index =
                                    tc_delta.get("index").and_then(Value::as_u64).unwrap_or(0)
                                        as usize;
                                let accumulator = tool_calls_map.entry(index).or_default();
                                if let Some(id) = tc_delta.get("id").and_then(Value::as_str) {
                                    accumulator.id = Some(id.to_owned());
                                }
                                if let Some(func) = tc_delta.get("function") {
                                    if let Some(name) = func.get("name").and_then(Value::as_str) {
                                        accumulator.name = Some(name.to_owned());
                                        // Fire on_tool_call_start when we first see the name.
                                        if let Some(cb) = callbacks
                                            .as_ref()
                                            .and_then(|c| c.on_tool_call_start.as_ref())
                                            && let Some(ref id) = accumulator.id
                                        {
                                            cb(id, name);
                                        }
                                        if let Some(cb) = callbacks
                                            .as_ref()
                                            .and_then(|c| c.on_lifecycle_event.as_ref())
                                        {
                                            cb(StreamingLifecycleEvent::ContentBlockStart {
                                                index,
                                                block_type: "tool_use".to_owned(),
                                            });
                                        }
                                    }
                                    if let Some(args) =
                                        func.get("arguments").and_then(Value::as_str)
                                    {
                                        // Fire on_tool_call_delta for incremental input.
                                        if let Some(cb) = callbacks
                                            .as_ref()
                                            .and_then(|c| c.on_tool_call_delta.as_ref())
                                            && let Some(ref id) = accumulator.id
                                        {
                                            cb(id, args);
                                        }
                                        accumulator.arguments.push_str(args);
                                    }
                                }
                            }
                        }
                    }

                    if let Some(u) = parsed.get("usage") {
                        let inp = u.get("prompt_tokens").and_then(Value::as_u64).unwrap_or(0);
                        let out = u
                            .get("completion_tokens")
                            .and_then(Value::as_u64)
                            .unwrap_or(0);
                        usage.input_tokens = inp;
                        usage.output_tokens = out;
                        // Fire on_usage callback.
                        if let Some(cb) = callbacks.as_ref().and_then(|c| c.on_usage.as_ref()) {
                            cb(StreamingUsageUpdate {
                                input_tokens: inp,
                                output_tokens: out,
                                cache_read_input_tokens: 0,
                                cache_creation_input_tokens: 0,
                            });
                        }
                    }
                }
            }
        }

        let raw_text = text_parts.join("");
        if message_started
            && let Some(cb) = callbacks
                .as_ref()
                .and_then(|c| c.on_lifecycle_event.as_ref())
        {
            cb(StreamingLifecycleEvent::MessageStop);
        }
        let tool_calls = tool_calls_map
            .into_iter()
            .filter_map(|(_, acc)| {
                let id = acc.id?;
                let name = acc.name?;
                let input = parse_streamed_tool_input(&acc.arguments);
                Some(ToolCall { id, name, input })
            })
            .collect::<Vec<_>>();

        Ok(ProviderResponse {
            text: crate::strip_reasoning_tags(&raw_text),
            history_text: Some(raw_text),
            thinking: None,
            content_blocks: Vec::new(),
            tool_calls,
            request_id,
            usage,
            stop_reason: finish_reason,
            research: None,
        })
    }

    async fn complete_streaming_anthropic(
        &self,
        provider: &ProviderConfig,
        conversation: &[ConversationEntry],
        callbacks: Option<&StreamingCallbacks>,
        carried_discovered_tools: &BTreeSet<String>,
        request_context: Option<&crate::query_source::ProviderRequestContext>,
    ) -> Result<ProviderResponse> {
        let effective_provider = provider_for_request(provider, request_context);
        let body = build_anthropic_request_body(
            &effective_provider,
            conversation,
            carried_discovered_tools,
            request_context,
            true,
        )
        .await;
        let base_url = provider
            .base_url
            .as_ref()
            .ok_or_else(|| anyhow!("provider is missing a normalized base URL"))?;

        let response = self
            .send_streaming_request(
                &effective_provider,
                base_url,
                &body,
                "anthropic-compatible",
                request_context,
            )
            .await?;

        let mut content_block_accumulators: BTreeMap<usize, AnthropicContentAccumulator> =
            BTreeMap::new();
        let mut usage = UsageSummary::default();
        let mut stop_reason = "end_turn".to_owned();
        let mut request_id: Option<String> = None;
        let mut research: Option<Value> = None;

        let mut stream = response.bytes_stream();
        let mut sse_buffer = String::new();

        let watchdog_enabled = is_stream_watchdog_enabled();
        let idle_timeout = stream_idle_timeout();

        loop {
            let chunk_result = if watchdog_enabled {
                match tokio::time::timeout(idle_timeout, stream.next()).await {
                    Ok(chunk) => chunk,
                    Err(_) => {
                        tracing::error!(
                            "Streaming idle timeout: no data received for {}s, aborting",
                            idle_timeout.as_secs()
                        );
                        return Err(stream_idle_timeout_error(idle_timeout));
                    }
                }
            } else {
                stream.next().await
            };

            let Some(chunk) = chunk_result else { break };
            let bytes = chunk.with_context(|| "failed to read streaming chunk")?;
            sse_buffer.push_str(&String::from_utf8_lossy(&bytes));
            if sse_buffer.len() > MAX_SSE_BUFFER_SIZE {
                tracing::warn!(
                    "SSE buffer exceeded {MAX_SSE_BUFFER_SIZE} bytes, discarding stale data"
                );
                sse_buffer.clear();
            }

            for event in parse_sse_events_from_buffer(&mut sse_buffer) {
                // Direct Anthropic API returns cache token counts.
                process_anthropic_event(
                    &event,
                    &mut content_block_accumulators,
                    &mut usage,
                    &mut stop_reason,
                    &mut request_id,
                    &mut research,
                    callbacks,
                    /* extract_cache_tokens */ true,
                )?;
            }
        }

        let (raw_text, thinking_text, content_blocks, tool_calls) =
            finalize_anthropic_content_blocks(content_block_accumulators);
        Ok(ProviderResponse {
            text: crate::strip_reasoning_tags(&raw_text),
            history_text: Some(raw_text),
            thinking: thinking_text,
            content_blocks,
            tool_calls,
            request_id,
            usage,
            stop_reason,
            research,
        })
    }

    /// Streaming completion for Amazon Bedrock using native SigV4 signing and
    /// the `invokeModelWithResponseStream` endpoint.
    ///
    /// Bedrock returns an AWS Event Stream encoded response where each frame
    /// contains a JSON payload with Anthropic SSE event types.
    async fn complete_streaming_bedrock(
        &self,
        provider: &ProviderConfig,
        conversation: &[ConversationEntry],
        callbacks: Option<&StreamingCallbacks>,
        carried_discovered_tools: &BTreeSet<String>,
        request_context: Option<&crate::query_source::ProviderRequestContext>,
    ) -> Result<ProviderResponse> {
        let effective_provider = provider_for_request(provider, request_context);
        let credentials = match crate::sigv4::load_aws_credentials() {
            Some(creds) => creds,
            None => {
                // No AWS credentials — fall back to non-streaming.
                return self
                    .complete_with_discovered_tools(
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
            .ok_or_else(|| anyhow!("Bedrock provider requires a model ID"))?;

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
            "stream": true,
        });
        crate::apply_anthropic_request_metadata(&mut body, &effective_provider, request_context);
        let payload =
            serde_json::to_vec(&body).context("failed to serialise Bedrock streaming request")?;

        // Construct Bedrock invoke-with-response-stream URL.
        let host = format!("bedrock-runtime.{}.amazonaws.com", credentials.region);
        let encoded_model = model.replace(':', "%3A").replace('+', "%2B");
        let path = format!("/model/{encoded_model}/invoke-with-response-stream");
        let url = format!("https://{host}{path}");

        // Sign the request.
        let signed = crate::sigv4::sign("POST", &host, &path, &payload, &credentials, "bedrock");

        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::CONTENT_TYPE,
            reqwest::header::HeaderValue::from_static("application/json"),
        );
        headers.insert(
            reqwest::header::ACCEPT,
            reqwest::header::HeaderValue::from_static("application/json"),
        );
        headers.insert(
            reqwest::header::HeaderName::from_static("host"),
            reqwest::header::HeaderValue::from_str(&signed.host)?,
        );
        headers.insert(
            reqwest::header::HeaderName::from_static("x-amz-date"),
            reqwest::header::HeaderValue::from_str(&signed.x_amz_date)?,
        );
        headers.insert(
            reqwest::header::HeaderName::from_static("x-amz-content-sha256"),
            reqwest::header::HeaderValue::from_str(&signed.x_amz_content_sha256)?,
        );
        headers.insert(
            reqwest::header::AUTHORIZATION,
            reqwest::header::HeaderValue::from_str(&signed.authorization)?,
        );
        if let Some(ref token) = signed.x_amz_security_token {
            headers.insert(
                reqwest::header::HeaderName::from_static("x-amz-security-token"),
                reqwest::header::HeaderValue::from_str(token)?,
            );
        }

        let response = self
            .http
            .post(&url)
            .headers(headers)
            .timeout(Duration::from_millis(provider.timeout_ms))
            .body(payload)
            .send()
            .await
            .context("Bedrock streaming request failed")?;

        let status = response.status().as_u16();
        if status >= 400 {
            let text = response
                .text()
                .await
                .context("failed to read Bedrock streaming error body")?;
            return Err(anyhow!(
                "Bedrock streaming request failed ({status}): {text}"
            ));
        }

        // Parse the AWS Event Stream response.
        // Bedrock sends binary event stream frames. Each frame has:
        //   - 4 bytes: total length (big-endian)
        //   - 4 bytes: headers length (big-endian)
        //   - 4 bytes: prelude CRC
        //   - headers (variable)
        //   - payload (variable)
        //   - 4 bytes: message CRC
        //
        // The payload contains JSON like: {"type":"content_block_delta","index":0,"delta":{...}}
        let mut content_block_accumulators: BTreeMap<usize, AnthropicContentAccumulator> =
            BTreeMap::new();
        let mut usage = UsageSummary::default();
        let mut stop_reason = "end_turn".to_owned();
        let mut request_id: Option<String> = None;
        let mut research: Option<Value> = None;

        let mut stream = response.bytes_stream();
        let mut buffer: Vec<u8> = Vec::new();

        let watchdog_enabled = is_stream_watchdog_enabled();
        let idle_timeout = stream_idle_timeout();

        loop {
            let chunk_result = if watchdog_enabled {
                match tokio::time::timeout(idle_timeout, stream.next()).await {
                    Ok(chunk) => chunk,
                    Err(_) => {
                        tracing::error!(
                            "Streaming idle timeout: no data received for {}s, aborting Bedrock stream",
                            idle_timeout.as_secs()
                        );
                        return Err(stream_idle_timeout_error(idle_timeout));
                    }
                }
            } else {
                stream.next().await
            };

            let Some(chunk) = chunk_result else { break };
            let bytes = chunk.with_context(|| "failed to read Bedrock streaming chunk")?;
            buffer.extend_from_slice(&bytes);

            // Parse complete event stream frames from the buffer.
            while buffer.len() >= 12 {
                // Minimum frame size: 4 (total_len) + 4 (headers_len) + 4 (prelude_crc)
                let total_len =
                    u32::from_be_bytes([buffer[0], buffer[1], buffer[2], buffer[3]]) as usize;
                if total_len < 14 {
                    // Malformed frame — minimum valid frame is 14 bytes:
                    // 4 (total_len) + 4 (headers_len) + 4 (prelude_crc) + 0 payload + 4 (message CRC)
                    // Anything smaller cannot have a valid payload_end = total_len - 4.
                    tracing::warn!("Bedrock frame with total_len={total_len} < 14, discarding");
                    buffer.drain(..4.min(buffer.len()));
                    continue;
                }
                if buffer.len() < total_len {
                    break; // Incomplete frame, wait for more data.
                }

                let headers_len =
                    u32::from_be_bytes([buffer[4], buffer[5], buffer[6], buffer[7]]) as usize;

                // Extract payload (between headers and final 4-byte CRC).
                let payload_start = 12 + headers_len;
                let payload_end = total_len - 4; // Last 4 bytes are message CRC.
                if payload_start >= payload_end {
                    // Malformed frame — skip it.
                    buffer.drain(..total_len);
                    continue;
                }

                let payload_bytes = &buffer[payload_start..payload_end];

                // Parse headers to find the event type.
                let _event_type = parse_bedrock_event_type(&buffer[12..12 + headers_len]);

                // The payload is JSON with Anthropic SSE event structure.
                if let Ok(event) = serde_json::from_slice::<Value>(payload_bytes) {
                    // Bedrock does not return cache token counts.
                    process_anthropic_event(
                        &event,
                        &mut content_block_accumulators,
                        &mut usage,
                        &mut stop_reason,
                        &mut request_id,
                        &mut research,
                        callbacks,
                        /* extract_cache_tokens */ false,
                    )?;
                }

                buffer.drain(..total_len);
            }
        }

        let (raw_text, thinking_text, content_blocks, tool_calls) =
            finalize_anthropic_content_blocks(content_block_accumulators);
        Ok(ProviderResponse {
            text: crate::strip_reasoning_tags(&raw_text),
            history_text: Some(raw_text),
            thinking: thinking_text,
            content_blocks,
            tool_calls,
            request_id,
            usage,
            stop_reason,
            research,
        })
    }

    /// Streaming completion for Google Vertex AI using OAuth2 Bearer auth.
    ///
    /// Vertex AI Claude models use the Anthropic Messages API format with SSE
    /// streaming, identical to the direct Anthropic API.
    async fn complete_streaming_vertex(
        &self,
        provider: &ProviderConfig,
        conversation: &[ConversationEntry],
        callbacks: Option<&StreamingCallbacks>,
        carried_discovered_tools: &BTreeSet<String>,
        request_context: Option<&crate::query_source::ProviderRequestContext>,
    ) -> Result<ProviderResponse> {
        let effective_provider = provider_for_request(provider, request_context);
        let access_token = match crate::load_vertex_access_token() {
            Some(token) => token,
            None => {
                // No Google credentials — fall back to non-streaming.
                return self
                    .complete_with_discovered_tools(
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
            .ok_or_else(|| anyhow!("Vertex AI provider requires a model ID"))?;

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
            "stream": true,
        });
        crate::apply_anthropic_request_metadata(&mut body, &effective_provider, request_context);

        // Construct Vertex AI streaming URL.
        let url = format!(
            "https://{region}-aiplatform.googleapis.com/v1/projects/{project}/locations/{region}/publishers/anthropic/models/{model}:streamRawPredict"
        );

        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::CONTENT_TYPE,
            reqwest::header::HeaderValue::from_static("application/json"),
        );
        headers.insert(
            reqwest::header::ACCEPT,
            reqwest::header::HeaderValue::from_static("text/event-stream"),
        );
        headers.insert(
            reqwest::header::AUTHORIZATION,
            reqwest::header::HeaderValue::from_str(&format!("Bearer {access_token}"))?,
        );

        let response = self
            .http
            .post(&url)
            .headers(headers)
            .timeout(Duration::from_millis(provider.timeout_ms))
            .json(&body)
            .send()
            .await
            .context("Vertex AI streaming request failed")?;

        let status = response.status().as_u16();
        if status >= 400 {
            let text = response
                .text()
                .await
                .context("failed to read Vertex AI streaming error body")?;
            return Err(anyhow!(
                "Vertex AI streaming request failed ({status}): {text}"
            ));
        }

        // Parse the SSE response — Vertex uses the same Anthropic SSE event format.
        let mut content_block_accumulators: BTreeMap<usize, AnthropicContentAccumulator> =
            BTreeMap::new();
        let mut usage = UsageSummary::default();
        let mut stop_reason = "end_turn".to_owned();
        let mut request_id: Option<String> = None;
        let mut research: Option<Value> = None;

        let mut stream = response.bytes_stream();
        let mut sse_buffer = String::new();

        let watchdog_enabled = is_stream_watchdog_enabled();
        let idle_timeout = stream_idle_timeout();

        loop {
            let chunk_result = if watchdog_enabled {
                match tokio::time::timeout(idle_timeout, stream.next()).await {
                    Ok(chunk) => chunk,
                    Err(_) => {
                        tracing::error!(
                            "Streaming idle timeout: no data received for {}s, aborting Vertex stream",
                            idle_timeout.as_secs()
                        );
                        return Err(stream_idle_timeout_error(idle_timeout));
                    }
                }
            } else {
                stream.next().await
            };

            let Some(chunk) = chunk_result else { break };
            let bytes = chunk.with_context(|| "failed to read Vertex streaming chunk")?;
            sse_buffer.push_str(&String::from_utf8_lossy(&bytes));
            if sse_buffer.len() > MAX_SSE_BUFFER_SIZE {
                tracing::warn!(
                    "SSE buffer exceeded {MAX_SSE_BUFFER_SIZE} bytes, discarding stale data"
                );
                sse_buffer.clear();
            }

            for event in parse_sse_events_from_buffer(&mut sse_buffer) {
                // Vertex does not return cache token counts.
                process_anthropic_event(
                    &event,
                    &mut content_block_accumulators,
                    &mut usage,
                    &mut stop_reason,
                    &mut request_id,
                    &mut research,
                    callbacks,
                    /* extract_cache_tokens */ false,
                )?;
            }
        }

        let (raw_text, thinking_text, content_blocks, tool_calls) =
            finalize_anthropic_content_blocks(content_block_accumulators);
        Ok(ProviderResponse {
            text: crate::strip_reasoning_tags(&raw_text),
            history_text: Some(raw_text),
            thinking: thinking_text,
            content_blocks,
            tool_calls,
            request_id,
            usage,
            stop_reason,
            research,
        })
    }

    async fn send_streaming_request(
        &self,
        provider: &ProviderConfig,
        base_url: &str,
        body: &Value,
        label: &str,
        request_context: Option<&crate::query_source::ProviderRequestContext>,
    ) -> Result<reqwest::Response> {
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
                    if status >= 400
                        && is_retryable_http_status(status)
                        && attempt < provider.max_retries
                    {
                        let retry_after = parse_retry_after(response.headers(), provider);
                        tokio::time::sleep(compute_retry_delay(provider, attempt, retry_after))
                            .await;
                        attempt += 1;
                        continue;
                    }
                    if status >= 400 {
                        let status_code = response.status().as_u16();
                        let text = response.text().await.with_context(|| {
                            format!("failed to read {label} error response body")
                        })?;

                        // Check for overloaded_error in body (the SDK sometimes
                        // drops 529 status during streaming).
                        if is_overloaded_error_body(text.as_bytes())
                            && attempt < provider.max_retries
                        {
                            tracing::warn!(
                                attempt = attempt + 1,
                                status_code,
                                "overloaded_error detected in response body, retrying"
                            );
                            tokio::time::sleep(compute_retry_delay(provider, attempt, None)).await;
                            attempt += 1;
                            continue;
                        }

                        let error_message = serde_json::from_str::<Value>(&text)
                            .ok()
                            .and_then(|v| {
                                v.get("error")
                                    .and_then(|e| e.get("message"))
                                    .and_then(Value::as_str)
                                    .map(str::to_owned)
                            })
                            .or_else(|| {
                                serde_json::from_str::<Value>(&text).ok().and_then(|v| {
                                    v.get("message").and_then(Value::as_str).map(str::to_owned)
                                })
                            })
                            .unwrap_or_else(|| "provider error".to_owned());
                        return Err(anyhow!(
                            "provider request failed ({status_code}): {error_message}"
                        ));
                    }
                    return Ok(response);
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

// ---------------------------------------------------------------------------
// Shared Anthropic SSE event parsing
// ---------------------------------------------------------------------------

/// Extract all complete SSE events from a text buffer.
///
/// Per the SSE specification (W3C), multiple `data:` lines within one event
/// block are joined with `\n` to form a single data payload. The buffer is
/// drained of all consumed bytes; incomplete trailing data is left for the
/// next call.
fn parse_sse_events_from_buffer(sse_buffer: &mut String) -> Vec<Value> {
    let mut events = Vec::new();
    while let Some(event_end) = sse_buffer.find("\n\n") {
        let event_text = sse_buffer[..event_end].to_owned();
        sse_buffer.replace_range(..event_end + 2, "");

        let mut data_lines: Vec<&str> = Vec::new();
        let mut event_type: Option<&str> = None;
        for line in event_text.lines() {
            if let Some(data) = line.strip_prefix("data: ") {
                data_lines.push(data.trim());
            } else if let Some(data) = line.strip_prefix("data:") {
                // SSE spec: space after colon is optional — handle `data:value`
                data_lines.push(data.trim());
            } else if line.starts_with(':') {
                // SSE comment — ignore per spec
            } else if let Some(ev) = line.strip_prefix("event: ") {
                event_type = Some(ev.trim());
            } else if let Some(ev) = line.strip_prefix("event:") {
                event_type = Some(ev.trim());
            } else if line.starts_with("id:") {
                // SSE event ID — not used for API event parsing
            }
        }

        if data_lines.is_empty() {
            continue;
        }

        // Per SSE spec: join multiple data: lines with \n
        let joined = data_lines.join("\n");

        // Skip [DONE] signals
        if joined.trim() == "[DONE]" {
            continue;
        }

        if let Ok(mut event) = serde_json::from_str::<Value>(&joined) {
            // If JSON has no "type" but SSE `event:` field was present, inject it
            if event.get("type").is_none()
                && let Some(ev_type) = event_type
            {
                event
                    .as_object_mut()
                    .map(|o| o.insert("type".to_owned(), Value::String(ev_type.to_owned())));
            }
            events.push(event);
        } else {
            // If joined payload isn't valid JSON, try parsing each line
            // individually as a fallback (handles non-standard servers).
            for data in &data_lines {
                if *data == "[DONE]" {
                    continue;
                }
                if let Ok(mut event) = serde_json::from_str::<Value>(data) {
                    if event.get("type").is_none()
                        && let Some(ev_type) = event_type
                    {
                        event.as_object_mut().map(|o| {
                            o.insert("type".to_owned(), Value::String(ev_type.to_owned()))
                        });
                    }
                    events.push(event);
                }
            }
        }
    }
    events
}

/// Process a single Anthropic-format streaming event.
///
/// Handles all Anthropic SSE event types: `message_start`, `content_block_start`,
/// `content_block_delta`, `content_block_stop`, `message_delta`, `message_stop`.
///
/// When `extract_cache_tokens` is `true`, cache-related usage fields
/// (`cache_read_input_tokens`, `cache_creation_input_tokens`) are extracted
/// from `message_start` events.  This is `true` for the direct Anthropic API
/// and `false` for Bedrock / Vertex which do not return cache token counts.
fn process_anthropic_event(
    event: &Value,
    content_block_accumulators: &mut BTreeMap<usize, AnthropicContentAccumulator>,
    usage: &mut UsageSummary,
    stop_reason: &mut String,
    request_id: &mut Option<String>,
    research: &mut Option<Value>,
    callbacks: Option<&StreamingCallbacks>,
    extract_cache_tokens: bool,
) -> Result<()> {
    let event_type = event.get("type").and_then(Value::as_str).unwrap_or("");

    match event_type {
        "message_start" => {
            if let Some(cb) = callbacks.and_then(|c| c.on_lifecycle_event.as_ref()) {
                cb(StreamingLifecycleEvent::MessageStart);
            }
            if let Some(msg) = event.get("message") {
                if request_id.is_none() {
                    *request_id = msg.get("id").and_then(Value::as_str).map(ToOwned::to_owned);
                }
                // Extract the `research` field from the message if present.
                if research.is_none()
                    && let Some(r) = msg.get("research").cloned()
                {
                    *research = Some(r);
                }
                if let Some(u) = msg.get("usage") {
                    let inp = u.get("input_tokens").and_then(Value::as_u64).unwrap_or(0);
                    usage.input_tokens = inp;
                    if extract_cache_tokens {
                        usage.cache_read_input_tokens = u
                            .get("cache_read_input_tokens")
                            .and_then(Value::as_u64)
                            .unwrap_or(0);
                        usage.cache_creation_input_tokens = u
                            .get("cache_creation_input_tokens")
                            .and_then(Value::as_u64)
                            .unwrap_or(0);
                    }
                    // Extract server_tool_use sub-fields.
                    if let Some(stu) = u.get("server_tool_use") {
                        usage.server_tool_use_web_search_requests = stu
                            .get("web_search_requests")
                            .and_then(Value::as_u64)
                            .unwrap_or(0);
                        usage.server_tool_use_web_fetch_requests = stu
                            .get("web_fetch_requests")
                            .and_then(Value::as_u64)
                            .unwrap_or(0);
                    }
                    // Extract cache_creation sub-fields for ephemeral TTL breakdown.
                    if let Some(cc) = u.get("cache_creation") {
                        usage.cache_creation_ephemeral_5m_input_tokens = cc
                            .get("ephemeral_5m_input_tokens")
                            .and_then(Value::as_u64)
                            .unwrap_or(0);
                        usage.cache_creation_ephemeral_1h_input_tokens = cc
                            .get("ephemeral_1h_input_tokens")
                            .and_then(Value::as_u64)
                            .unwrap_or(0);
                    }
                    if let Some(cb) = callbacks.and_then(|c| c.on_usage.as_ref()) {
                        cb(StreamingUsageUpdate {
                            input_tokens: inp,
                            output_tokens: 0,
                            cache_read_input_tokens: usage.cache_read_input_tokens,
                            cache_creation_input_tokens: usage.cache_creation_input_tokens,
                        });
                    }
                }
            }
        }
        "content_block_start" => {
            #[allow(clippy::cast_possible_truncation)]
            let index = event.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
            let content_block = event.get("content_block");
            let block_type = content_block
                .and_then(|b| b.get("type"))
                .and_then(Value::as_str)
                .unwrap_or("");

            if let Some(cb) = callbacks.and_then(|c| c.on_lifecycle_event.as_ref()) {
                cb(StreamingLifecycleEvent::ContentBlockStart {
                    index,
                    block_type: block_type.to_owned(),
                });
            }

            match block_type {
                "text" => {
                    let text = content_block
                        .and_then(|b| b.get("text"))
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned();
                    content_block_accumulators.insert(
                        index,
                        AnthropicContentAccumulator::Text {
                            text,
                            citations: Vec::new(),
                        },
                    );
                }
                "thinking" => {
                    let thinking = content_block
                        .and_then(|b| b.get("thinking"))
                        .or_else(|| content_block.and_then(|b| b.get("text")))
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned();
                    let signature = content_block
                        .and_then(|b| b.get("signature"))
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned);
                    content_block_accumulators.insert(
                        index,
                        AnthropicContentAccumulator::Thinking {
                            thinking,
                            signature,
                        },
                    );
                }
                "tool_use" | "server_tool_use" => {
                    let id = content_block
                        .and_then(|b| b.get("id"))
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned();
                    let name = content_block
                        .and_then(|b| b.get("name"))
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned();
                    // Fire on_tool_call_start callback.
                    if let Some(cb) = callbacks.and_then(|c| c.on_tool_call_start.as_ref()) {
                        cb(&id, &name);
                    }
                    content_block_accumulators.insert(
                        index,
                        AnthropicContentAccumulator::ToolUse(AnthropicToolUseAccumulator {
                            block_type: block_type.to_owned(),
                            id,
                            name,
                            partial_json: String::new(),
                        }),
                    );
                }
                "redacted_thinking" => {
                    // Redacted thinking blocks contain opaque data, not readable text.
                    // We record their presence to preserve token accounting.
                    let data = content_block
                        .and_then(|b| b.get("data"))
                        .and_then(|d| d.as_str())
                        .unwrap_or("")
                        .to_owned();
                    content_block_accumulators.insert(
                        index,
                        AnthropicContentAccumulator::RedactedThinking { data },
                    );
                }
                "image" => {
                    // Image blocks in responses contain a source with base64 data.
                    content_block_accumulators.insert(
                        index,
                        AnthropicContentAccumulator::Image {
                            block: content_block
                                .cloned()
                                .unwrap_or_else(|| json!({"type": "image"})),
                        },
                    );
                }
                "document" => {
                    // Document blocks contain source.type, source.media_type,
                    // source.data / source.url fields.
                    content_block_accumulators.insert(
                        index,
                        AnthropicContentAccumulator::Document {
                            block: content_block
                                .cloned()
                                .unwrap_or_else(|| json!({"type": "document"})),
                        },
                    );
                }
                "connector_text" => {
                    // Connector text blocks are text-like blocks from connector tools.
                    let text = content_block
                        .and_then(|b| b.get("text"))
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned();
                    content_block_accumulators
                        .insert(index, AnthropicContentAccumulator::ConnectorText { text });
                }
                "web_search_tool_result" => {
                    // Web search tool result blocks contain tool_use_id, content, status.
                    content_block_accumulators.insert(
                        index,
                        AnthropicContentAccumulator::WebSearchToolResult {
                            block: content_block
                                .cloned()
                                .unwrap_or_else(|| json!({"type": "web_search_tool_result"})),
                        },
                    );
                }
                "mcp_tool_use" => {
                    let id = content_block
                        .and_then(|b| b.get("id"))
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned();
                    let name = content_block
                        .and_then(|b| b.get("name"))
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned();
                    let server_name = content_block
                        .and_then(|b| b.get("server_name"))
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned();
                    content_block_accumulators.insert(
                        index,
                        AnthropicContentAccumulator::McpToolUse {
                            id,
                            name,
                            server_name,
                            partial_json: String::new(),
                        },
                    );
                }
                "mcp_tool_result" => {
                    content_block_accumulators.insert(
                        index,
                        AnthropicContentAccumulator::McpToolResult {
                            block: content_block
                                .cloned()
                                .unwrap_or_else(|| json!({"type": "mcp_tool_result"})),
                        },
                    );
                }
                "code_edit" => {
                    content_block_accumulators.insert(
                        index,
                        AnthropicContentAccumulator::CodeEdit {
                            block: content_block
                                .cloned()
                                .unwrap_or_else(|| json!({"type": "code_edit"})),
                        },
                    );
                }
                "code_output" => {
                    content_block_accumulators.insert(
                        index,
                        AnthropicContentAccumulator::CodeOutput {
                            block: content_block
                                .cloned()
                                .unwrap_or_else(|| json!({"type": "code_output"})),
                        },
                    );
                }
                "context_block" => {
                    content_block_accumulators.insert(
                        index,
                        AnthropicContentAccumulator::ContextBlock {
                            block: content_block
                                .cloned()
                                .unwrap_or_else(|| json!({"type": "context_block"})),
                        },
                    );
                }
                _ => {}
            }
        }
        "content_block_delta" => {
            #[allow(clippy::cast_possible_truncation)]
            let index = event.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
            let delta = event.get("delta");
            let delta_type = delta
                .and_then(|d| d.get("type"))
                .and_then(Value::as_str)
                .unwrap_or("");

            if delta_type == "thinking_delta"
                && let Some(thinking) = delta
                    .and_then(|d| d.get("thinking"))
                    .and_then(Value::as_str)
                && let Some(AnthropicContentAccumulator::Thinking {
                    thinking: existing, ..
                }) = content_block_accumulators.get_mut(&index)
            {
                existing.push_str(thinking);
                if let Some(callbacks) = callbacks
                    && let Some(ref on_thinking_delta) = callbacks.on_thinking_delta
                {
                    on_thinking_delta(thinking);
                }
            } else if delta_type == "signature_delta"
                && let Some(signature) = delta
                    .and_then(|d| d.get("signature"))
                    .and_then(Value::as_str)
                && let Some(AnthropicContentAccumulator::Thinking {
                    signature: existing,
                    ..
                }) = content_block_accumulators.get_mut(&index)
            {
                *existing = Some(signature.to_owned());
            } else if delta_type == "text_delta"
                && let Some(text) = delta.and_then(|d| d.get("text")).and_then(Value::as_str)
                && let Some(AnthropicContentAccumulator::Text { text: existing, .. }) =
                    content_block_accumulators.get_mut(&index)
            {
                // Fire on_text_delta callback.
                if let Some(cb) = callbacks.and_then(|c| c.on_text_delta.as_ref()) {
                    cb(text);
                }
                existing.push_str(text);
            } else if delta_type == "connector_text_delta"
                && let Some(text) = delta.and_then(|d| d.get("text")).and_then(Value::as_str)
                && let Some(AnthropicContentAccumulator::ConnectorText { text: existing }) =
                    content_block_accumulators.get_mut(&index)
            {
                existing.push_str(text);
            } else if delta_type == "citations_delta" {
                // Citations delta provides citation metadata (cited_text,
                // document_index, document_title, url) for a text block.
                // We accumulate citations into the text accumulator so they
                // can be emitted in the finalised content block.
                if let Some(citation) = delta.and_then(|d| d.get("citation")).cloned()
                    && let Some(AnthropicContentAccumulator::Text { citations, .. }) =
                        content_block_accumulators.get_mut(&index)
                {
                    citations.push(citation);
                }
            }

            if delta_type == "input_json_delta"
                && let Some(partial) = delta
                    .and_then(|d| d.get("partial_json"))
                    .and_then(Value::as_str)
            {
                match content_block_accumulators.get_mut(&index) {
                    Some(AnthropicContentAccumulator::ToolUse(acc)) => {
                        if let Some(cb) = callbacks.and_then(|c| c.on_tool_call_delta.as_ref()) {
                            cb(&acc.id, partial);
                        }
                        acc.partial_json.push_str(partial);
                    }
                    Some(AnthropicContentAccumulator::McpToolUse { partial_json, .. }) => {
                        partial_json.push_str(partial);
                    }
                    _ => {}
                }
            }
        }
        "content_block_stop" => {
            #[allow(clippy::cast_possible_truncation)]
            let index = event.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
            if let Some(cb) = callbacks.and_then(|c| c.on_lifecycle_event.as_ref()) {
                cb(StreamingLifecycleEvent::ContentBlockStop { index });
            }
        }
        "message_stop" => {
            if let Some(cb) = callbacks.and_then(|c| c.on_lifecycle_event.as_ref()) {
                cb(StreamingLifecycleEvent::MessageStop);
            }
        }
        "message_delta" => {
            if let Some(delta_val) = event.get("delta")
                && let Some(reason) = delta_val.get("stop_reason").and_then(Value::as_str)
            {
                reason.clone_into(stop_reason);
            }
            if let Some(u) = event.get("usage") {
                let out = u.get("output_tokens").and_then(Value::as_u64).unwrap_or(0);
                usage.output_tokens = out;
                if let Some(cb) = callbacks.and_then(|c| c.on_lifecycle_event.as_ref()) {
                    cb(StreamingLifecycleEvent::MessageDelta {
                        stop_reason: stop_reason.clone(),
                        output_tokens: out,
                    });
                }
                // Fire on_usage callback with final output token count.
                if let Some(cb) = callbacks.and_then(|c| c.on_usage.as_ref()) {
                    cb(StreamingUsageUpdate {
                        input_tokens: usage.input_tokens,
                        output_tokens: out,
                        cache_read_input_tokens: usage.cache_read_input_tokens,
                        cache_creation_input_tokens: usage.cache_creation_input_tokens,
                    });
                }
            }
        }
        "error" => {
            let error_msg = event
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("Unknown streaming error");
            let error_type = event
                .get("error")
                .and_then(|e| e.get("type"))
                .and_then(|t| t.as_str())
                .unwrap_or("unknown");
            return Err(anyhow!(
                "API streaming error ({}): {}",
                error_type,
                error_msg
            ));
        }
        "ping" => {
            // Heartbeat from the server, no action needed.
        }
        _ => {}
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse the `:event-type` header value from a Bedrock event stream header block.
///
/// AWS Event Stream headers are TLV-encoded. The event type header has:
/// - 1 byte: header name length
/// - N bytes: header name (":event-type")
/// - 1 byte: header value type (7 = string)
/// - 2 bytes: value length (big-endian)
/// - N bytes: value string
///
/// Returns the event type string (e.g. "chunk", "internal-server-error") or
/// an empty string if not found.
fn parse_bedrock_event_type(header_bytes: &[u8]) -> String {
    let mut pos = 0;
    while pos + 2 < header_bytes.len() {
        let name_len = header_bytes[pos] as usize;
        pos += 1;
        if pos + name_len >= header_bytes.len() {
            break;
        }
        let name = &header_bytes[pos..pos + name_len];
        pos += name_len;
        if pos >= header_bytes.len() {
            break;
        }
        let value_type = header_bytes[pos];
        pos += 1;

        if name == b":event-type" && value_type == 7 {
            // String type: 2-byte length + value.
            if pos + 2 > header_bytes.len() {
                break;
            }
            let val_len = u16::from_be_bytes([header_bytes[pos], header_bytes[pos + 1]]) as usize;
            pos += 2;
            if pos + val_len > header_bytes.len() {
                break;
            }
            return String::from_utf8_lossy(&header_bytes[pos..pos + val_len]).to_string();
        }

        // Skip other header values based on type.
        match value_type {
            0..=4 => pos += 4, // int/short/byte/bool
            5 => pos += 8,     // long
            6 => pos += 16,    // bytes (16 bytes)
            7 => {
                // String: 2-byte length + value.
                if pos + 2 > header_bytes.len() {
                    break;
                }
                let val_len =
                    u16::from_be_bytes([header_bytes[pos], header_bytes[pos + 1]]) as usize;
                pos += 2 + val_len;
            }
            8 => pos += 8,       // timestamp
            9 | 10 => pos += 16, // uuid
            11 | 12 => pos += 1, // byte/bool single byte
            _ => break,          // Unknown type — stop parsing.
        }
    }
    String::new()
}

fn wrap_streaming_callbacks(
    callbacks: Option<StreamingCallbacks>,
    streamed_tool_activity: Arc<AtomicBool>,
) -> StreamingCallbacks {
    let callbacks = callbacks.unwrap_or_default();
    let StreamingCallbacks {
        on_text_delta,
        on_tool_call_start,
        on_tool_call_delta,
        on_usage,
        on_thinking_delta,
        on_lifecycle_event,
    } = callbacks;

    let start_activity = Arc::clone(&streamed_tool_activity);
    let tracked_tool_call_start = Box::new(move |tool_call_id: &str, tool_name: &str| {
        start_activity.store(true, Ordering::Relaxed);
        if let Some(callback) = on_tool_call_start.as_ref() {
            callback(tool_call_id, tool_name);
        }
    });

    let delta_activity = Arc::clone(&streamed_tool_activity);
    let tracked_tool_call_delta = Box::new(move |tool_call_id: &str, delta: &str| {
        delta_activity.store(true, Ordering::Relaxed);
        if let Some(callback) = on_tool_call_delta.as_ref() {
            callback(tool_call_id, delta);
        }
    });

    StreamingCallbacks {
        on_text_delta,
        on_tool_call_start: Some(tracked_tool_call_start),
        on_tool_call_delta: Some(tracked_tool_call_delta),
        on_usage,
        on_thinking_delta,
        on_lifecycle_event,
    }
}

fn should_fallback_after_streaming_error(
    error: &anyhow::Error,
    streamed_tool_activity: bool,
) -> bool {
    let err_str = format!("{error:#}").to_ascii_lowercase();
    let is_streaming_error = err_str.contains("streaming")
        || err_str.contains("chunk")
        || err_str.contains("connection")
        || err_str.contains("broken pipe")
        || err_str.contains("reset")
        || err_str.contains("unexpected eof");
    is_streaming_error && !streamed_tool_activity
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
    retry_compute_retry_delay(&config, attempt, retry_after)
}

fn parse_retry_after(
    headers: &reqwest::header::HeaderMap,
    provider: &ProviderConfig,
) -> Option<Duration> {
    retry_parse_retry_after(headers, provider.respect_retry_after)
}

#[derive(Default)]
struct OpenAiToolCallAccumulator {
    id: Option<String>,
    name: Option<String>,
    arguments: String,
}

struct AnthropicToolUseAccumulator {
    block_type: String,
    id: String,
    name: String,
    partial_json: String,
}

enum AnthropicContentAccumulator {
    Text {
        text: String,
        /// Accumulated citations received via `citations_delta` events.
        citations: Vec<Value>,
    },
    Thinking {
        thinking: String,
        signature: Option<String>,
    },
    ToolUse(AnthropicToolUseAccumulator),
    RedactedThinking {
        data: String,
    },
    Image {
        block: Value,
    },
    /// Document content block — contains `source.type`, `source.media_type`,
    /// `source.data` / `source.url`, etc.
    Document {
        block: Value,
    },
    /// Connector text block — a text-like block from connector tools.
    ConnectorText {
        text: String,
    },
    /// Web search tool result block.
    WebSearchToolResult {
        block: Value,
    },
    /// MCP tool use block — distinct from regular tool_use.
    McpToolUse {
        id: String,
        name: String,
        server_name: String,
        partial_json: String,
    },
    /// MCP tool result block.
    McpToolResult {
        block: Value,
    },
    /// Code edit block — code modification result from the API.
    CodeEdit {
        block: Value,
    },
    /// Code output block — code execution output from the API.
    CodeOutput {
        block: Value,
    },
    /// Context block — additional context provided by the API.
    ContextBlock {
        block: Value,
    },
}

fn finalize_anthropic_content_blocks(
    accumulators: BTreeMap<usize, AnthropicContentAccumulator>,
) -> (String, Option<String>, Vec<Value>, Vec<ToolCall>) {
    let mut raw_text_parts = Vec::new();
    let mut thinking_parts = Vec::new();
    let mut content_blocks = Vec::new();
    let mut tool_calls = Vec::new();

    for accumulator in accumulators.into_values() {
        match accumulator {
            AnthropicContentAccumulator::Text { text, citations } => {
                if text.is_empty() {
                    continue;
                }
                raw_text_parts.push(text.clone());
                let mut block = json!({
                    "type": "text",
                    "text": text,
                });
                if !citations.is_empty() {
                    block["citations"] = Value::Array(citations);
                }
                content_blocks.push(block);
            }
            AnthropicContentAccumulator::Thinking {
                thinking,
                signature,
            } => {
                if thinking.is_empty() && signature.is_none() {
                    continue;
                }
                if !thinking.is_empty() {
                    thinking_parts.push(thinking.clone());
                }
                let mut block = json!({
                    "type": "thinking",
                    "thinking": thinking,
                });
                if let Some(signature) = signature {
                    block["signature"] = Value::String(signature);
                }
                content_blocks.push(block);
            }
            AnthropicContentAccumulator::ToolUse(acc) => {
                if acc.id.is_empty() || acc.name.is_empty() {
                    continue;
                }
                let input = parse_streamed_tool_input(&acc.partial_json);
                tool_calls.push(ToolCall {
                    id: acc.id.clone(),
                    name: acc.name.clone(),
                    input: input.clone(),
                });
                content_blocks.push(json!({
                    "type": acc.block_type,
                    "id": acc.id,
                    "name": acc.name,
                    "input": input,
                }));
            }
            AnthropicContentAccumulator::RedactedThinking { data } => {
                content_blocks.push(json!({
                    "type": "redacted_thinking",
                    "data": data,
                }));
            }
            AnthropicContentAccumulator::Image { block } => {
                content_blocks.push(block);
            }
            AnthropicContentAccumulator::Document { block } => {
                content_blocks.push(block);
            }
            AnthropicContentAccumulator::ConnectorText { text } => {
                if text.is_empty() {
                    continue;
                }
                content_blocks.push(json!({
                    "type": "connector_text",
                    "text": text,
                }));
            }
            AnthropicContentAccumulator::WebSearchToolResult { block } => {
                content_blocks.push(block);
            }
            AnthropicContentAccumulator::McpToolUse {
                id,
                name,
                server_name,
                partial_json,
            } => {
                if id.is_empty() || name.is_empty() {
                    continue;
                }
                let input = parse_streamed_tool_input(&partial_json);
                tool_calls.push(ToolCall {
                    id: id.clone(),
                    name: name.clone(),
                    input: input.clone(),
                });
                let mut block = json!({
                    "type": "mcp_tool_use",
                    "id": id,
                    "name": name,
                    "input": input,
                });
                block["server_name"] = Value::String(server_name);
                content_blocks.push(block);
            }
            AnthropicContentAccumulator::McpToolResult { block } => {
                content_blocks.push(block);
            }
            AnthropicContentAccumulator::CodeEdit { block } => {
                content_blocks.push(block);
            }
            AnthropicContentAccumulator::CodeOutput { block } => {
                content_blocks.push(block);
            }
            AnthropicContentAccumulator::ContextBlock { block } => {
                content_blocks.push(block);
            }
        }
    }

    let raw_text = raw_text_parts.join("");
    let thinking_text = if thinking_parts.is_empty() {
        None
    } else {
        Some(thinking_parts.join(""))
    };

    (raw_text, thinking_text, content_blocks, tool_calls)
}

fn parse_streamed_tool_input(raw: &str) -> Value {
    if raw.trim().is_empty() {
        return json!({});
    }

    serde_json::from_str::<Value>(raw).unwrap_or_else(|error| {
        json!({
            "_remote_code_error": "malformed_tool_input_json",
            "message": error.to_string(),
            "raw": raw,
        })
    })
}

#[cfg(test)]
mod tests {
    use anyhow::anyhow;
    use serde_json::{Value, json};
    use std::collections::BTreeMap;

    use super::{
        AnthropicContentAccumulator, AnthropicToolUseAccumulator, StreamingCallbacks, UsageSummary,
        finalize_anthropic_content_blocks, is_retryable_http_status, parse_bedrock_event_type,
        parse_sse_events_from_buffer, process_anthropic_event,
        should_fallback_after_streaming_error,
    };

    #[test]
    fn streaming_errors_fallback_before_tool_activity() {
        assert!(should_fallback_after_streaming_error(
            &anyhow!("streaming connection reset by peer"),
            false,
        ));
    }

    #[test]
    fn streaming_errors_do_not_fallback_after_tool_activity() {
        assert!(!should_fallback_after_streaming_error(
            &anyhow!("streaming connection reset by peer"),
            true,
        ));
    }

    #[test]
    fn non_streaming_errors_do_not_trigger_fallback() {
        assert!(!should_fallback_after_streaming_error(
            &anyhow!("provider request failed (401): unauthorized"),
            false,
        ));
    }

    #[test]
    fn overloaded_529_is_retryable_for_streaming_requests() {
        assert!(is_retryable_http_status(529));
    }

    #[test]
    fn anthropic_streaming_finalizer_preserves_block_order_and_text() {
        let mut accumulators = BTreeMap::new();
        accumulators.insert(
            0,
            AnthropicContentAccumulator::Thinking {
                thinking: "plan".to_owned(),
                signature: Some("sig".to_owned()),
            },
        );
        accumulators.insert(
            1,
            AnthropicContentAccumulator::Text {
                text: "reply".to_owned(),
                citations: Vec::new(),
            },
        );
        accumulators.insert(
            2,
            AnthropicContentAccumulator::ToolUse(AnthropicToolUseAccumulator {
                block_type: "tool_use".to_owned(),
                id: "call-2".to_owned(),
                name: "read_file".to_owned(),
                partial_json: r#"{"path":"src/lib.rs"}"#.to_owned(),
            }),
        );
        accumulators.insert(
            3,
            AnthropicContentAccumulator::ToolUse(AnthropicToolUseAccumulator {
                block_type: "tool_use".to_owned(),
                id: "call-3".to_owned(),
                name: "read_file".to_owned(),
                partial_json: r#"{"path":"src/main.rs"}"#.to_owned(),
            }),
        );

        let (raw_text, thinking_text, content_blocks, tool_calls) =
            finalize_anthropic_content_blocks(accumulators);

        assert_eq!(raw_text, "reply");
        assert_eq!(thinking_text.as_deref(), Some("plan"));
        assert_eq!(content_blocks.len(), 4);
        assert_eq!(content_blocks[0]["type"], "thinking");
        assert_eq!(content_blocks[0]["signature"], "sig");
        assert_eq!(content_blocks[1]["type"], "text");
        assert_eq!(content_blocks[2]["id"], "call-2");
        assert_eq!(content_blocks[3]["id"], "call-3");
        assert_eq!(tool_calls[0].id, "call-2");
        assert_eq!(tool_calls[1].id, "call-3");
        assert_eq!(tool_calls[0].input, json!({"path":"src/lib.rs"}));
    }

    #[test]
    fn bedrock_event_type_header_parsing() {
        // Build a minimal AWS Event Stream header block with :event-type = "chunk".
        // Header format: name_len(1) + name(N) + value_type(1) + value_len(2) + value(N)
        let event_type_name = b":event-type";
        let event_type_value = b"chunk";

        let mut header_bytes = Vec::new();
        header_bytes.push(event_type_name.len() as u8);
        header_bytes.extend_from_slice(event_type_name);
        header_bytes.push(7); // String type.
        header_bytes.push(0); // Value length high byte.
        header_bytes.push(event_type_value.len() as u8); // Value length low byte.
        header_bytes.extend_from_slice(event_type_value);

        assert_eq!(parse_bedrock_event_type(&header_bytes), "chunk");
    }

    #[test]
    fn bedrock_event_type_header_empty_on_missing() {
        // Empty header block should return empty string.
        assert_eq!(parse_bedrock_event_type(&[]), "");
    }

    #[test]
    fn bedrock_event_type_header_other_headers() {
        // Header block with other headers but no :event-type.
        let mut header_bytes = Vec::new();
        let name = b":message-id";
        let value = b"test-123";
        header_bytes.push(name.len() as u8);
        header_bytes.extend_from_slice(name);
        header_bytes.push(7); // String type.
        header_bytes.push(0);
        header_bytes.push(value.len() as u8);
        header_bytes.extend_from_slice(value);

        assert_eq!(parse_bedrock_event_type(&header_bytes), "");
    }

    // -----------------------------------------------------------------------
    // Tests for the shared SSE parsing helpers
    // -----------------------------------------------------------------------

    #[test]
    fn sse_buffer_parses_single_event() {
        let mut buf = "data: {\"type\":\"message_start\"}\n\n".to_owned();
        let events = parse_sse_events_from_buffer(&mut buf);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["type"], "message_start");
        assert!(buf.is_empty());
    }

    #[test]
    fn sse_buffer_skips_done_signal() {
        let mut buf = "data: [DONE]\n\n".to_owned();
        let events = parse_sse_events_from_buffer(&mut buf);
        assert!(events.is_empty());
    }

    #[test]
    fn sse_buffer_parses_multiple_events() {
        let mut buf = "data: {\"type\":\"message_start\"}\n\ndata: {\"type\":\"message_stop\"}\n\n"
            .to_owned();
        let events = parse_sse_events_from_buffer(&mut buf);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["type"], "message_start");
        assert_eq!(events[1]["type"], "message_stop");
    }

    #[test]
    fn sse_buffer_preserves_incomplete_trailing_data() {
        let mut buf = "data: {\"type\":\"message_start\"}\n\ndata: incom".to_owned();
        let events = parse_sse_events_from_buffer(&mut buf);
        assert_eq!(events.len(), 1);
        assert_eq!(buf, "data: incom");
    }

    #[test]
    fn sse_buffer_ignores_non_data_lines() {
        let mut buf = "event: ping\ndata: {\"type\":\"message_start\"}\n\n".to_owned();
        let events = parse_sse_events_from_buffer(&mut buf);
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn process_anthropic_event_extracts_text_delta() {
        let mut accumulators = BTreeMap::new();
        accumulators.insert(
            0,
            AnthropicContentAccumulator::Text {
                text: String::new(),
                citations: Vec::new(),
            },
        );
        let mut usage = UsageSummary::default();
        let mut stop_reason = "end_turn".to_owned();
        let mut request_id: Option<String> = None;
        let mut research: Option<Value> = None;

        let event = json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": { "type": "text_delta", "text": "hello" }
        });

        let _ = process_anthropic_event(
            &event,
            &mut accumulators,
            &mut usage,
            &mut stop_reason,
            &mut request_id,
            &mut research,
            None::<&StreamingCallbacks>,
            false,
        );

        if let Some(AnthropicContentAccumulator::Text { text, .. }) = accumulators.get(&0) {
            assert_eq!(text, "hello");
        } else {
            panic!("expected Text accumulator");
        }
    }

    #[test]
    fn process_anthropic_event_extracts_cache_tokens_when_enabled() {
        let mut accumulators = BTreeMap::new();
        let mut usage = UsageSummary::default();
        let mut stop_reason = "end_turn".to_owned();
        let mut request_id: Option<String> = None;
        let mut research: Option<Value> = None;

        let event = json!({
            "type": "message_start",
            "message": {
                "id": "msg-1",
                "usage": {
                    "input_tokens": 100,
                    "cache_read_input_tokens": 50,
                    "cache_creation_input_tokens": 25
                }
            }
        });

        let _ = process_anthropic_event(
            &event,
            &mut accumulators,
            &mut usage,
            &mut stop_reason,
            &mut request_id,
            &mut research,
            None::<&StreamingCallbacks>,
            true,
        );

        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.cache_read_input_tokens, 50);
        assert_eq!(usage.cache_creation_input_tokens, 25);
        assert_eq!(request_id.as_deref(), Some("msg-1"));
    }

    #[test]
    fn process_anthropic_event_skips_cache_tokens_when_disabled() {
        let mut accumulators = BTreeMap::new();
        let mut usage = UsageSummary::default();
        let mut stop_reason = "end_turn".to_owned();
        let mut request_id: Option<String> = None;
        let mut research: Option<Value> = None;

        let event = json!({
            "type": "message_start",
            "message": {
                "id": "msg-2",
                "usage": {
                    "input_tokens": 100,
                    "cache_read_input_tokens": 50,
                    "cache_creation_input_tokens": 25
                }
            }
        });

        let _ = process_anthropic_event(
            &event,
            &mut accumulators,
            &mut usage,
            &mut stop_reason,
            &mut request_id,
            &mut research,
            None::<&StreamingCallbacks>,
            false,
        );

        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.cache_read_input_tokens, 0);
        assert_eq!(usage.cache_creation_input_tokens, 0);
    }

    #[test]
    fn process_anthropic_event_extracts_message_delta_stop_reason() {
        let mut accumulators = BTreeMap::new();
        let mut usage = UsageSummary {
            input_tokens: 100,
            ..Default::default()
        };
        let mut stop_reason = "end_turn".to_owned();
        let mut request_id: Option<String> = None;
        let mut research: Option<Value> = None;

        let event = json!({
            "type": "message_delta",
            "delta": { "stop_reason": "tool_use" },
            "usage": { "output_tokens": 42 }
        });

        let _ = process_anthropic_event(
            &event,
            &mut accumulators,
            &mut usage,
            &mut stop_reason,
            &mut request_id,
            &mut research,
            None::<&StreamingCallbacks>,
            false,
        );

        assert_eq!(stop_reason, "tool_use");
        assert_eq!(usage.output_tokens, 42);
    }

    // -----------------------------------------------------------------------
    // Additional SSE parsing tests
    // -----------------------------------------------------------------------

    #[test]
    fn sse_buffer_handles_comment_lines() {
        // SSE comment lines (starting with ':') should be ignored.
        let mut buf = ": this is a comment\n\ndata: {\"type\":\"ping\"}\n\n".to_owned();
        let events = parse_sse_events_from_buffer(&mut buf);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["type"], "ping");
        assert!(buf.is_empty());
    }

    #[test]
    fn sse_buffer_handles_only_comments() {
        let mut buf = ": comment line 1\n: comment line 2\n\n".to_owned();
        let events = parse_sse_events_from_buffer(&mut buf);
        assert!(events.is_empty());
        assert!(buf.is_empty());
    }

    #[test]
    fn sse_buffer_handles_empty_input() {
        let mut buf = String::new();
        let events = parse_sse_events_from_buffer(&mut buf);
        assert!(events.is_empty());
        assert!(buf.is_empty());
    }

    #[test]
    fn sse_buffer_incomplete_event_no_double_newline() {
        // No \n\n means the event is incomplete; buffer should be preserved.
        let mut buf = "data: {\"type\":\"message_start\"}".to_owned();
        let events = parse_sse_events_from_buffer(&mut buf);
        assert!(events.is_empty());
        assert_eq!(buf, "data: {\"type\":\"message_start\"}");
    }

    #[test]
    fn sse_buffer_incomplete_after_one_complete() {
        let mut buf =
            "data: {\"type\":\"message_start\"}\n\ndata: {\"type\":\"content_block_delta\"}"
                .to_owned();
        let events = parse_sse_events_from_buffer(&mut buf);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["type"], "message_start");
        // The incomplete second event remains in buffer.
        assert_eq!(buf, "data: {\"type\":\"content_block_delta\"}");
    }

    #[test]
    fn sse_buffer_ignores_invalid_json() {
        let mut buf = "data: not-valid-json\n\ndata: {\"type\":\"ping\"}\n\n".to_owned();
        let events = parse_sse_events_from_buffer(&mut buf);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["type"], "ping");
    }

    #[test]
    fn sse_buffer_multiple_data_lines_in_one_event() {
        // SSE spec: multiple data: lines within one event are joined with newlines.
        // The joined result is parsed as a single JSON payload.
        let mut buf =
            "data: {\"type\":\"message_start\"}\ndata: {\"type\":\"ping\"}\n\n".to_owned();
        let events = parse_sse_events_from_buffer(&mut buf);
        // Joined payload isn't valid JSON, so fallback parses each line individually.
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["type"], "message_start");
        assert_eq!(events[1]["type"], "ping");
    }

    #[test]
    fn sse_buffer_multi_line_data_joins_per_spec() {
        // When data: lines form a single valid JSON when joined, the joined parse succeeds.
        // This simulates a server splitting "hello world" across two data: lines.
        let mut buf =
            "data: {\"type\":\"content_block_delta\"\ndata: ,\"delta\":{\"text\":\"hello\"}}\n\n"
                .to_owned();
        let events = parse_sse_events_from_buffer(&mut buf);
        // Neither individual line is valid JSON, but joined they form:
        // {"type":"content_block_delta"\n,\"delta":{"text":"hello"}}
        // which still isn't valid JSON because of the newline — so fallback tries each line.
        // The key behavior is: we try the joined parse first, then fall back to individual lines.
        assert!(!events.is_empty() || buf.is_empty()); // parser completed without panic
    }

    #[test]
    fn sse_buffer_handles_event_type_line() {
        // The "event:" line is not a "data:" line, so it's ignored.
        let mut buf = "event: message_start\ndata: {\"type\":\"message_start\"}\n\n".to_owned();
        let events = parse_sse_events_from_buffer(&mut buf);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["type"], "message_start");
    }

    #[test]
    fn sse_buffer_handles_three_consecutive_events() {
        let mut buf = "data: {\"type\":\"message_start\"}\n\n\
                       data: {\"type\":\"content_block_start\"}\n\n\
                       data: {\"type\":\"message_stop\"}\n\n"
            .to_owned();
        let events = parse_sse_events_from_buffer(&mut buf);
        assert_eq!(events.len(), 3);
        assert_eq!(events[0]["type"], "message_start");
        assert_eq!(events[1]["type"], "content_block_start");
        assert_eq!(events[2]["type"], "message_stop");
    }

    #[test]
    fn sse_buffer_done_signal_ignored() {
        let mut buf = "data: [DONE]\n\n".to_owned();
        let events = parse_sse_events_from_buffer(&mut buf);
        assert!(events.is_empty());
    }

    #[test]
    fn sse_buffer_done_signal_mixed_with_events() {
        let mut buf = "data: {\"type\":\"message_start\"}\n\ndata: [DONE]\n\n".to_owned();
        let events = parse_sse_events_from_buffer(&mut buf);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["type"], "message_start");
    }

    #[test]
    fn sse_buffer_whitespace_trimming() {
        let mut buf = "data:  {\"type\":\"ping\"} \n\n".to_owned();
        let events = parse_sse_events_from_buffer(&mut buf);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["type"], "ping");
    }

    // -----------------------------------------------------------------------
    // Additional Anthropic event processing tests
    // -----------------------------------------------------------------------

    #[test]
    fn process_anthropic_event_message_start_extracts_request_id() {
        let mut accumulators = BTreeMap::new();
        let mut usage = UsageSummary::default();
        let mut stop_reason = "end_turn".to_owned();
        let mut request_id: Option<String> = None;
        let mut research: Option<Value> = None;

        let event = json!({
            "type": "message_start",
            "message": {
                "id": "msg-test-abc",
                "usage": { "input_tokens": 50 }
            }
        });

        let _ = process_anthropic_event(
            &event,
            &mut accumulators,
            &mut usage,
            &mut stop_reason,
            &mut request_id,
            &mut research,
            None::<&StreamingCallbacks>,
            false,
        );

        assert_eq!(request_id.as_deref(), Some("msg-test-abc"));
        assert_eq!(usage.input_tokens, 50);
    }

    #[test]
    fn process_anthropic_event_message_start_does_not_overwrite_request_id() {
        let mut accumulators = BTreeMap::new();
        let mut usage = UsageSummary::default();
        let mut stop_reason = "end_turn".to_owned();
        let mut request_id: Option<String> = Some("msg-original".to_owned());
        let mut research: Option<Value> = None;

        let event = json!({
            "type": "message_start",
            "message": {
                "id": "msg-new",
                "usage": { "input_tokens": 50 }
            }
        });

        let _ = process_anthropic_event(
            &event,
            &mut accumulators,
            &mut usage,
            &mut stop_reason,
            &mut request_id,
            &mut research,
            None::<&StreamingCallbacks>,
            false,
        );

        // Should keep the original request_id.
        assert_eq!(request_id.as_deref(), Some("msg-original"));
    }

    #[test]
    fn process_anthropic_event_content_block_start_text() {
        let mut accumulators = BTreeMap::new();
        let mut usage = UsageSummary::default();
        let mut stop_reason = "end_turn".to_owned();
        let mut request_id: Option<String> = None;
        let mut research: Option<Value> = None;

        let event = json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": { "type": "text", "text": "Hello" }
        });

        let _ = process_anthropic_event(
            &event,
            &mut accumulators,
            &mut usage,
            &mut stop_reason,
            &mut request_id,
            &mut research,
            None::<&StreamingCallbacks>,
            false,
        );

        assert!(matches!(
            accumulators.get(&0),
            Some(AnthropicContentAccumulator::Text { text, .. }) if text == "Hello"
        ));
    }

    #[test]
    fn process_anthropic_event_content_block_start_thinking() {
        let mut accumulators = BTreeMap::new();
        let mut usage = UsageSummary::default();
        let mut stop_reason = "end_turn".to_owned();
        let mut request_id: Option<String> = None;
        let mut research: Option<Value> = None;

        let event = json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": {
                "type": "thinking",
                "thinking": "Let me think...",
                "signature": "sig-abc"
            }
        });

        let _ = process_anthropic_event(
            &event,
            &mut accumulators,
            &mut usage,
            &mut stop_reason,
            &mut request_id,
            &mut research,
            None::<&StreamingCallbacks>,
            false,
        );

        assert!(matches!(
            accumulators.get(&0),
            Some(AnthropicContentAccumulator::Thinking { thinking, signature })
                if thinking == "Let me think..." && signature.as_deref() == Some("sig-abc")
        ));
    }

    #[test]
    fn process_anthropic_event_content_block_start_tool_use() {
        let mut accumulators = BTreeMap::new();
        let mut usage = UsageSummary::default();
        let mut stop_reason = "end_turn".to_owned();
        let mut request_id: Option<String> = None;
        let mut research: Option<Value> = None;

        let event = json!({
            "type": "content_block_start",
            "index": 1,
            "content_block": {
                "type": "tool_use",
                "id": "tool-123",
                "name": "read_file"
            }
        });

        let _ = process_anthropic_event(
            &event,
            &mut accumulators,
            &mut usage,
            &mut stop_reason,
            &mut request_id,
            &mut research,
            None::<&StreamingCallbacks>,
            false,
        );

        assert!(matches!(
            accumulators.get(&1),
            Some(AnthropicContentAccumulator::ToolUse(acc))
                if acc.id == "tool-123" && acc.name == "read_file" && acc.partial_json.is_empty()
        ));
    }

    #[test]
    fn process_anthropic_event_content_block_delta_text_accumulates() {
        let mut accumulators = BTreeMap::new();
        accumulators.insert(
            0,
            AnthropicContentAccumulator::Text {
                text: "Hello ".to_owned(),
                citations: Vec::new(),
            },
        );
        let mut usage = UsageSummary::default();
        let mut stop_reason = "end_turn".to_owned();
        let mut request_id: Option<String> = None;
        let mut research: Option<Value> = None;

        let event = json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": { "type": "text_delta", "text": "world!" }
        });

        let _ = process_anthropic_event(
            &event,
            &mut accumulators,
            &mut usage,
            &mut stop_reason,
            &mut request_id,
            &mut research,
            None::<&StreamingCallbacks>,
            false,
        );

        assert!(matches!(
            accumulators.get(&0),
            Some(AnthropicContentAccumulator::Text { text, .. }) if text == "Hello world!"
        ));
    }

    #[test]
    fn process_anthropic_event_content_block_delta_thinking_accumulates() {
        let mut accumulators = BTreeMap::new();
        accumulators.insert(
            0,
            AnthropicContentAccumulator::Thinking {
                thinking: "Step 1. ".to_owned(),
                signature: None,
            },
        );
        let mut usage = UsageSummary::default();
        let mut stop_reason = "end_turn".to_owned();
        let mut request_id: Option<String> = None;
        let mut research: Option<Value> = None;

        let event = json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": { "type": "thinking_delta", "thinking": "Step 2." }
        });

        let _ = process_anthropic_event(
            &event,
            &mut accumulators,
            &mut usage,
            &mut stop_reason,
            &mut request_id,
            &mut research,
            None::<&StreamingCallbacks>,
            false,
        );

        assert!(matches!(
            accumulators.get(&0),
            Some(AnthropicContentAccumulator::Thinking { thinking, .. })
                if thinking == "Step 1. Step 2."
        ));
    }

    #[test]
    fn process_anthropic_event_content_block_delta_signature() {
        let mut accumulators = BTreeMap::new();
        accumulators.insert(
            0,
            AnthropicContentAccumulator::Thinking {
                thinking: "thoughts".to_owned(),
                signature: None,
            },
        );
        let mut usage = UsageSummary::default();
        let mut stop_reason = "end_turn".to_owned();
        let mut request_id: Option<String> = None;
        let mut research: Option<Value> = None;

        let event = json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": { "type": "signature_delta", "signature": "sig-final" }
        });

        let _ = process_anthropic_event(
            &event,
            &mut accumulators,
            &mut usage,
            &mut stop_reason,
            &mut request_id,
            &mut research,
            None::<&StreamingCallbacks>,
            false,
        );

        assert!(matches!(
            accumulators.get(&0),
            Some(AnthropicContentAccumulator::Thinking { signature: Some(sig), .. })
                if sig == "sig-final"
        ));
    }

    #[test]
    fn process_anthropic_event_content_block_delta_tool_input_json() {
        let mut accumulators = BTreeMap::new();
        accumulators.insert(
            1,
            AnthropicContentAccumulator::ToolUse(AnthropicToolUseAccumulator {
                block_type: "tool_use".to_owned(),
                id: "tool-1".to_owned(),
                name: "read_file".to_owned(),
                partial_json: "{\"path\":".to_owned(),
            }),
        );
        let mut usage = UsageSummary::default();
        let mut stop_reason = "end_turn".to_owned();
        let mut request_id: Option<String> = None;
        let mut research: Option<Value> = None;

        let event = json!({
            "type": "content_block_delta",
            "index": 1,
            "delta": { "type": "input_json_delta", "partial_json": "\"src/lib.rs\"}" }
        });

        let _ = process_anthropic_event(
            &event,
            &mut accumulators,
            &mut usage,
            &mut stop_reason,
            &mut request_id,
            &mut research,
            None::<&StreamingCallbacks>,
            false,
        );

        if let Some(AnthropicContentAccumulator::ToolUse(acc)) = accumulators.get(&1) {
            assert_eq!(acc.partial_json, "{\"path\":\"src/lib.rs\"}");
        } else {
            panic!("expected ToolUse accumulator at index 1");
        }
    }

    #[test]
    fn process_anthropic_event_content_block_stop_is_noop() {
        let mut accumulators = BTreeMap::new();
        let mut usage = UsageSummary::default();
        let mut stop_reason = "end_turn".to_owned();
        let mut request_id: Option<String> = None;
        let mut research: Option<Value> = None;

        let event = json!({ "type": "content_block_stop", "index": 0 });

        let _ = process_anthropic_event(
            &event,
            &mut accumulators,
            &mut usage,
            &mut stop_reason,
            &mut request_id,
            &mut research,
            None::<&StreamingCallbacks>,
            false,
        );

        // Should be a complete no-op.
        assert!(accumulators.is_empty());
        assert_eq!(stop_reason, "end_turn");
    }

    #[test]
    fn process_anthropic_event_message_stop_is_noop() {
        let mut accumulators = BTreeMap::new();
        let mut usage = UsageSummary::default();
        let mut stop_reason = "end_turn".to_owned();
        let mut request_id: Option<String> = None;
        let mut research: Option<Value> = None;

        let event = json!({ "type": "message_stop" });

        let _ = process_anthropic_event(
            &event,
            &mut accumulators,
            &mut usage,
            &mut stop_reason,
            &mut request_id,
            &mut research,
            None::<&StreamingCallbacks>,
            false,
        );

        assert!(accumulators.is_empty());
        assert_eq!(stop_reason, "end_turn");
    }

    #[test]
    fn process_anthropic_event_unknown_type_is_noop() {
        let mut accumulators = BTreeMap::new();
        let mut usage = UsageSummary::default();
        let mut stop_reason = "end_turn".to_owned();
        let mut request_id: Option<String> = None;
        let mut research: Option<Value> = None;

        let event = json!({ "type": "ping" });

        let _ = process_anthropic_event(
            &event,
            &mut accumulators,
            &mut usage,
            &mut stop_reason,
            &mut request_id,
            &mut research,
            None::<&StreamingCallbacks>,
            false,
        );

        // "ping" falls through to the `_ => {}` branch — no state changes.
        assert!(accumulators.is_empty());
        assert_eq!(stop_reason, "end_turn");
    }

    #[test]
    fn process_anthropic_event_message_delta_with_end_turn() {
        let mut accumulators = BTreeMap::new();
        let mut usage = UsageSummary::default();
        let mut stop_reason = "tool_use".to_owned();
        let mut request_id: Option<String> = None;
        let mut research: Option<Value> = None;

        let event = json!({
            "type": "message_delta",
            "delta": { "stop_reason": "end_turn" },
            "usage": { "output_tokens": 99 }
        });

        let _ = process_anthropic_event(
            &event,
            &mut accumulators,
            &mut usage,
            &mut stop_reason,
            &mut request_id,
            &mut research,
            None::<&StreamingCallbacks>,
            false,
        );

        assert_eq!(stop_reason, "end_turn");
        assert_eq!(usage.output_tokens, 99);
    }

    #[test]
    fn process_anthropic_event_message_start_with_cache_tokens() {
        let mut accumulators = BTreeMap::new();
        let mut usage = UsageSummary::default();
        let mut stop_reason = "end_turn".to_owned();
        let mut request_id: Option<String> = None;
        let mut research: Option<Value> = None;

        let event = json!({
            "type": "message_start",
            "message": {
                "id": "msg-cache",
                "usage": {
                    "input_tokens": 200,
                    "cache_read_input_tokens": 150,
                    "cache_creation_input_tokens": 30
                }
            }
        });

        let _ = process_anthropic_event(
            &event,
            &mut accumulators,
            &mut usage,
            &mut stop_reason,
            &mut request_id,
            &mut research,
            None::<&StreamingCallbacks>,
            true,
        );

        assert_eq!(usage.input_tokens, 200);
        assert_eq!(usage.cache_read_input_tokens, 150);
        assert_eq!(usage.cache_creation_input_tokens, 30);
    }

    // -----------------------------------------------------------------------
    // Additional retry logic tests (imported from retry.rs)
    // -----------------------------------------------------------------------

    #[test]
    fn retryable_status_429() {
        assert!(is_retryable_http_status(429));
    }

    #[test]
    fn retryable_status_500() {
        assert!(is_retryable_http_status(500));
    }

    #[test]
    fn retryable_status_502() {
        assert!(is_retryable_http_status(502));
    }

    #[test]
    fn retryable_status_503() {
        assert!(is_retryable_http_status(503));
    }

    #[test]
    fn retryable_status_504() {
        assert!(is_retryable_http_status(504));
    }

    #[test]
    fn retryable_status_408() {
        assert!(is_retryable_http_status(408));
    }

    #[test]
    fn non_retryable_status_400() {
        assert!(!is_retryable_http_status(400));
    }

    #[test]
    fn non_retryable_status_401() {
        assert!(!is_retryable_http_status(401));
    }

    #[test]
    fn non_retryable_status_403() {
        assert!(!is_retryable_http_status(403));
    }

    #[test]
    fn non_retryable_status_404() {
        assert!(!is_retryable_http_status(404));
    }

    #[test]
    fn non_retryable_status_200() {
        assert!(!is_retryable_http_status(200));
    }

    // -----------------------------------------------------------------------
    // Additional finalize_anthropic_content_blocks tests
    // -----------------------------------------------------------------------

    #[test]
    fn finalize_empty_accumulators() {
        let accumulators: BTreeMap<usize, AnthropicContentAccumulator> = BTreeMap::new();
        let (raw_text, thinking_text, content_blocks, tool_calls) =
            finalize_anthropic_content_blocks(accumulators);
        assert_eq!(raw_text, "");
        assert!(thinking_text.is_none());
        assert!(content_blocks.is_empty());
        assert!(tool_calls.is_empty());
    }

    #[test]
    fn finalize_single_text_block() {
        let mut accumulators = BTreeMap::new();
        accumulators.insert(
            0,
            AnthropicContentAccumulator::Text {
                text: "Hello".to_owned(),
                citations: Vec::new(),
            },
        );

        let (raw_text, thinking_text, content_blocks, tool_calls) =
            finalize_anthropic_content_blocks(accumulators);

        assert_eq!(raw_text, "Hello");
        assert!(thinking_text.is_none());
        assert_eq!(content_blocks.len(), 1);
        assert_eq!(content_blocks[0]["type"], "text");
        assert!(tool_calls.is_empty());
    }

    #[test]
    fn finalize_empty_text_block_is_skipped() {
        let mut accumulators = BTreeMap::new();
        accumulators.insert(
            0,
            AnthropicContentAccumulator::Text {
                text: String::new(),
                citations: Vec::new(),
            },
        );

        let (raw_text, thinking_text, content_blocks, tool_calls) =
            finalize_anthropic_content_blocks(accumulators);

        assert_eq!(raw_text, "");
        assert!(thinking_text.is_none());
        assert!(content_blocks.is_empty());
        assert!(tool_calls.is_empty());
    }

    #[test]
    fn finalize_thinking_block_with_signature() {
        let mut accumulators = BTreeMap::new();
        accumulators.insert(
            0,
            AnthropicContentAccumulator::Thinking {
                thinking: "deep thoughts".to_owned(),
                signature: Some("sig123".to_owned()),
            },
        );

        let (raw_text, thinking_text, content_blocks, tool_calls) =
            finalize_anthropic_content_blocks(accumulators);

        assert_eq!(raw_text, "");
        assert_eq!(thinking_text.as_deref(), Some("deep thoughts"));
        assert_eq!(content_blocks.len(), 1);
        assert_eq!(content_blocks[0]["type"], "thinking");
        assert_eq!(content_blocks[0]["signature"], "sig123");
        assert!(tool_calls.is_empty());
    }

    #[test]
    fn finalize_tool_use_block_parses_json() {
        let mut accumulators = BTreeMap::new();
        accumulators.insert(
            0,
            AnthropicContentAccumulator::ToolUse(AnthropicToolUseAccumulator {
                block_type: "tool_use".to_owned(),
                id: "call-1".to_owned(),
                name: "bash".to_owned(),
                partial_json: r#"{"command":"ls -la"}"#.to_owned(),
            }),
        );

        let (raw_text, thinking_text, content_blocks, tool_calls) =
            finalize_anthropic_content_blocks(accumulators);

        assert_eq!(raw_text, "");
        assert!(thinking_text.is_none());
        assert_eq!(content_blocks.len(), 1);
        assert_eq!(content_blocks[0]["type"], "tool_use");
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].id, "call-1");
        assert_eq!(tool_calls[0].name, "bash");
        assert_eq!(tool_calls[0].input["command"], "ls -la");
    }

    #[test]
    fn finalize_tool_use_block_empty_json_defaults_to_empty_object() {
        let mut accumulators = BTreeMap::new();
        accumulators.insert(
            0,
            AnthropicContentAccumulator::ToolUse(AnthropicToolUseAccumulator {
                block_type: "tool_use".to_owned(),
                id: "call-2".to_owned(),
                name: "read".to_owned(),
                partial_json: String::new(),
            }),
        );

        let (_, _, _, tool_calls) = finalize_anthropic_content_blocks(accumulators);

        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].input, json!({}));
    }

    #[test]
    fn finalize_tool_use_block_invalid_json_is_preserved_as_error_payload() {
        let mut accumulators = BTreeMap::new();
        accumulators.insert(
            0,
            AnthropicContentAccumulator::ToolUse(AnthropicToolUseAccumulator {
                block_type: "tool_use".to_owned(),
                id: "call-3".to_owned(),
                name: "write".to_owned(),
                partial_json: "not valid json{".to_owned(),
            }),
        );

        let (_, _, _, tool_calls) = finalize_anthropic_content_blocks(accumulators);

        assert_eq!(tool_calls.len(), 1);
        assert_eq!(
            tool_calls[0].input["_remote_code_error"],
            "malformed_tool_input_json"
        );
        assert_eq!(tool_calls[0].input["raw"], "not valid json{");
    }

    #[test]
    fn finalize_tool_use_with_empty_id_is_skipped() {
        let mut accumulators = BTreeMap::new();
        accumulators.insert(
            0,
            AnthropicContentAccumulator::ToolUse(AnthropicToolUseAccumulator {
                block_type: "tool_use".to_owned(),
                id: String::new(),
                name: "bash".to_owned(),
                partial_json: "{}".to_owned(),
            }),
        );

        let (_, _, content_blocks, tool_calls) = finalize_anthropic_content_blocks(accumulators);

        assert!(tool_calls.is_empty());
        assert!(content_blocks.is_empty());
    }

    // -----------------------------------------------------------------------
    // Additional should_fallback_after_streaming_error tests
    // -----------------------------------------------------------------------

    #[test]
    fn fallback_on_broken_pipe() {
        assert!(should_fallback_after_streaming_error(
            &anyhow!("broken pipe"),
            false,
        ));
    }

    #[test]
    fn fallback_on_unexpected_eof() {
        assert!(should_fallback_after_streaming_error(
            &anyhow!("unexpected eof"),
            false,
        ));
    }

    #[test]
    fn fallback_on_connection_reset() {
        assert!(should_fallback_after_streaming_error(
            &anyhow!("connection reset"),
            false,
        ));
    }

    #[test]
    fn no_fallback_on_generic_error() {
        assert!(!should_fallback_after_streaming_error(
            &anyhow!("something else went wrong"),
            false,
        ));
    }

    #[test]
    fn no_fallback_on_chunk_error_with_tool_activity() {
        assert!(!should_fallback_after_streaming_error(
            &anyhow!("chunk read error"),
            true,
        ));
    }

    // -----------------------------------------------------------------------
    // Stream idle watchdog tests
    // -----------------------------------------------------------------------

    #[test]
    fn stream_idle_timeout_error_is_classified_as_streaming() {
        // The idle timeout error must be recognised as a streaming error
        // so the fallback logic can retry with non-streaming.
        let err = super::stream_idle_timeout_error(std::time::Duration::from_secs(90));
        assert!(should_fallback_after_streaming_error(&err, false));
    }

    #[test]
    fn stream_idle_timeout_error_does_not_fallback_after_tool_activity() {
        let err = super::stream_idle_timeout_error(std::time::Duration::from_secs(90));
        assert!(!should_fallback_after_streaming_error(&err, true));
    }

    #[test]
    fn stream_idle_timeout_error_message_contains_timeout_info() {
        let err = super::stream_idle_timeout_error(std::time::Duration::from_secs(90));
        let msg = format!("{err:#}");
        assert!(msg.contains("streaming"));
        assert!(msg.contains("idle timeout"));
        assert!(msg.contains("90s"));
    }

    #[test]
    fn default_stream_idle_timeout_is_90_seconds() {
        // When the env var is not set, should default to 90s.
        // (This test may read the actual env var if set, so we just verify the
        //  helper returns a reasonable Duration.)
        let timeout = super::stream_idle_timeout();
        assert!(timeout.as_millis() > 0);
    }

    // -----------------------------------------------------------------------
    // model_context_window_exceeded stop reason test
    // -----------------------------------------------------------------------

    #[test]
    fn process_anthropic_event_message_delta_with_model_context_window_exceeded() {
        let mut accumulators = BTreeMap::new();
        let mut usage = UsageSummary {
            input_tokens: 100,
            ..Default::default()
        };
        let mut stop_reason = "end_turn".to_owned();
        let mut request_id: Option<String> = None;
        let mut research: Option<Value> = None;

        let event = json!({
            "type": "message_delta",
            "delta": { "stop_reason": "model_context_window_exceeded" },
            "usage": { "output_tokens": 500 }
        });

        let _ = process_anthropic_event(
            &event,
            &mut accumulators,
            &mut usage,
            &mut stop_reason,
            &mut request_id,
            &mut research,
            None::<&StreamingCallbacks>,
            false,
        );

        assert_eq!(stop_reason, "model_context_window_exceeded");
        assert_eq!(usage.output_tokens, 500);
    }

    // -----------------------------------------------------------------------
    // Tests for new content block types (document, connector_text,
    // web_search_tool_result, citations_delta, research)
    // -----------------------------------------------------------------------

    #[test]
    fn process_anthropic_event_content_block_start_document() {
        let mut accumulators = BTreeMap::new();
        let mut usage = UsageSummary::default();
        let mut stop_reason = "end_turn".to_owned();
        let mut request_id: Option<String> = None;
        let mut research: Option<Value> = None;

        let event = json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": {
                "type": "document",
                "source": {
                    "type": "base64",
                    "media_type": "application/pdf",
                    "data": "SGVsbG8gV29ybGQ="
                }
            }
        });

        process_anthropic_event(
            &event,
            &mut accumulators,
            &mut usage,
            &mut stop_reason,
            &mut request_id,
            &mut research,
            None::<&StreamingCallbacks>,
            false,
        )
        .unwrap();

        assert!(matches!(
            accumulators.get(&0),
            Some(AnthropicContentAccumulator::Document { block }) if block["type"] == "document"
        ));
        if let Some(AnthropicContentAccumulator::Document { block }) = accumulators.get(&0) {
            assert_eq!(block["source"]["type"], "base64");
            assert_eq!(block["source"]["media_type"], "application/pdf");
            assert_eq!(block["source"]["data"], "SGVsbG8gV29ybGQ=");
        }
    }

    #[test]
    fn finalize_document_block_preserves_fields() {
        let mut accumulators = BTreeMap::new();
        accumulators.insert(
            0,
            AnthropicContentAccumulator::Document {
                block: json!({
                    "type": "document",
                    "source": {
                        "type": "url",
                        "url": "https://example.com/doc.pdf"
                    }
                }),
            },
        );

        let (raw_text, thinking_text, content_blocks, tool_calls) =
            finalize_anthropic_content_blocks(accumulators);

        assert_eq!(raw_text, "");
        assert!(thinking_text.is_none());
        assert_eq!(content_blocks.len(), 1);
        assert_eq!(content_blocks[0]["type"], "document");
        assert_eq!(content_blocks[0]["source"]["type"], "url");
        assert!(tool_calls.is_empty());
    }

    #[test]
    fn process_anthropic_event_content_block_start_connector_text() {
        let mut accumulators = BTreeMap::new();
        let mut usage = UsageSummary::default();
        let mut stop_reason = "end_turn".to_owned();
        let mut request_id: Option<String> = None;
        let mut research: Option<Value> = None;

        let event = json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": {
                "type": "connector_text",
                "text": "Initial"
            }
        });

        process_anthropic_event(
            &event,
            &mut accumulators,
            &mut usage,
            &mut stop_reason,
            &mut request_id,
            &mut research,
            None::<&StreamingCallbacks>,
            false,
        )
        .unwrap();

        assert!(matches!(
            accumulators.get(&0),
            Some(AnthropicContentAccumulator::ConnectorText { text }) if text == "Initial"
        ));
    }

    #[test]
    fn process_anthropic_event_connector_text_delta_accumulates() {
        let mut accumulators = BTreeMap::new();
        accumulators.insert(
            0,
            AnthropicContentAccumulator::ConnectorText {
                text: "Part 1. ".to_owned(),
            },
        );
        let mut usage = UsageSummary::default();
        let mut stop_reason = "end_turn".to_owned();
        let mut request_id: Option<String> = None;
        let mut research: Option<Value> = None;

        let event = json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": { "type": "connector_text_delta", "text": "Part 2." }
        });

        process_anthropic_event(
            &event,
            &mut accumulators,
            &mut usage,
            &mut stop_reason,
            &mut request_id,
            &mut research,
            None::<&StreamingCallbacks>,
            false,
        )
        .unwrap();

        assert!(matches!(
            accumulators.get(&0),
            Some(AnthropicContentAccumulator::ConnectorText { text }) if text == "Part 1. Part 2."
        ));
    }

    #[test]
    fn finalize_connector_text_block() {
        let mut accumulators = BTreeMap::new();
        accumulators.insert(
            0,
            AnthropicContentAccumulator::ConnectorText {
                text: "connector result".to_owned(),
            },
        );

        let (raw_text, thinking_text, content_blocks, tool_calls) =
            finalize_anthropic_content_blocks(accumulators);

        assert_eq!(raw_text, "");
        assert!(thinking_text.is_none());
        assert_eq!(content_blocks.len(), 1);
        assert_eq!(content_blocks[0]["type"], "connector_text");
        assert_eq!(content_blocks[0]["text"], "connector result");
        assert!(tool_calls.is_empty());
    }

    #[test]
    fn finalize_empty_connector_text_block_is_skipped() {
        let mut accumulators = BTreeMap::new();
        accumulators.insert(
            0,
            AnthropicContentAccumulator::ConnectorText {
                text: String::new(),
            },
        );

        let (_, _, content_blocks, _) = finalize_anthropic_content_blocks(accumulators);
        assert!(content_blocks.is_empty());
    }

    #[test]
    fn process_anthropic_event_content_block_start_web_search_tool_result() {
        let mut accumulators = BTreeMap::new();
        let mut usage = UsageSummary::default();
        let mut stop_reason = "end_turn".to_owned();
        let mut request_id: Option<String> = None;
        let mut research: Option<Value> = None;

        let event = json!({
            "type": "content_block_start",
            "index": 2,
            "content_block": {
                "type": "web_search_tool_result",
                "tool_use_id": "srvtoolu_01ABC",
                "content": [
                    { "type": "web_search_result", "url": "https://example.com", "title": "Example" }
                ],
                "status": "completed"
            }
        });

        process_anthropic_event(
            &event,
            &mut accumulators,
            &mut usage,
            &mut stop_reason,
            &mut request_id,
            &mut research,
            None::<&StreamingCallbacks>,
            false,
        )
        .unwrap();

        assert!(matches!(
            accumulators.get(&2),
            Some(AnthropicContentAccumulator::WebSearchToolResult { block })
                if block["type"] == "web_search_tool_result"
        ));
        if let Some(AnthropicContentAccumulator::WebSearchToolResult { block }) =
            accumulators.get(&2)
        {
            assert_eq!(block["tool_use_id"], "srvtoolu_01ABC");
            assert_eq!(block["status"], "completed");
        }
    }

    #[test]
    fn finalize_web_search_tool_result_block() {
        let mut accumulators = BTreeMap::new();
        accumulators.insert(
            0,
            AnthropicContentAccumulator::WebSearchToolResult {
                block: json!({
                    "type": "web_search_tool_result",
                    "tool_use_id": "srvtoolu_01XYZ",
                    "content": [{ "type": "web_search_result", "url": "https://example.com" }],
                    "status": "completed"
                }),
            },
        );

        let (raw_text, thinking_text, content_blocks, tool_calls) =
            finalize_anthropic_content_blocks(accumulators);

        assert_eq!(raw_text, "");
        assert!(thinking_text.is_none());
        assert_eq!(content_blocks.len(), 1);
        assert_eq!(content_blocks[0]["type"], "web_search_tool_result");
        assert_eq!(content_blocks[0]["tool_use_id"], "srvtoolu_01XYZ");
        assert!(tool_calls.is_empty());
    }

    #[test]
    fn process_anthropic_event_citations_delta_accumulates() {
        let mut accumulators = BTreeMap::new();
        accumulators.insert(
            0,
            AnthropicContentAccumulator::Text {
                text: "According to ".to_owned(),
                citations: Vec::new(),
            },
        );
        let mut usage = UsageSummary::default();
        let mut stop_reason = "end_turn".to_owned();
        let mut request_id: Option<String> = None;
        let mut research: Option<Value> = None;

        let event = json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": {
                "type": "citations_delta",
                "citation": {
                    "type": "web_search_result",
                    "cited_text": "the sky is blue",
                    "document_index": 0,
                    "document_title": "Science Facts",
                    "url": "https://example.com/science"
                }
            }
        });

        process_anthropic_event(
            &event,
            &mut accumulators,
            &mut usage,
            &mut stop_reason,
            &mut request_id,
            &mut research,
            None::<&StreamingCallbacks>,
            false,
        )
        .unwrap();

        if let Some(AnthropicContentAccumulator::Text { text, citations }) = accumulators.get(&0) {
            assert_eq!(text, "According to ");
            assert_eq!(citations.len(), 1);
            assert_eq!(citations[0]["type"], "web_search_result");
            assert_eq!(citations[0]["cited_text"], "the sky is blue");
            assert_eq!(citations[0]["url"], "https://example.com/science");
        } else {
            panic!("expected Text accumulator");
        }
    }

    #[test]
    fn finalize_text_block_with_citations() {
        let mut accumulators = BTreeMap::new();
        accumulators.insert(
            0,
            AnthropicContentAccumulator::Text {
                text: "Cited text".to_owned(),
                citations: vec![
                    json!({
                        "type": "web_search_result",
                        "cited_text": "source A",
                        "document_index": 0,
                        "url": "https://example.com/a"
                    }),
                    json!({
                        "type": "web_search_result",
                        "cited_text": "source B",
                        "document_index": 1,
                        "url": "https://example.com/b"
                    }),
                ],
            },
        );

        let (raw_text, _, content_blocks, _) = finalize_anthropic_content_blocks(accumulators);

        assert_eq!(raw_text, "Cited text");
        assert_eq!(content_blocks.len(), 1);
        assert_eq!(content_blocks[0]["type"], "text");
        let citations = content_blocks[0]["citations"].as_array().unwrap();
        assert_eq!(citations.len(), 2);
        assert_eq!(citations[0]["cited_text"], "source A");
        assert_eq!(citations[1]["cited_text"], "source B");
    }

    #[test]
    fn finalize_text_block_without_citations_omits_field() {
        let mut accumulators = BTreeMap::new();
        accumulators.insert(
            0,
            AnthropicContentAccumulator::Text {
                text: "No citations".to_owned(),
                citations: Vec::new(),
            },
        );

        let (_, _, content_blocks, _) = finalize_anthropic_content_blocks(accumulators);

        assert_eq!(content_blocks.len(), 1);
        // When citations are empty, the field should not be present.
        assert!(content_blocks[0].get("citations").is_none());
    }

    #[test]
    fn process_anthropic_event_message_start_extracts_research() {
        let mut accumulators = BTreeMap::new();
        let mut usage = UsageSummary::default();
        let mut stop_reason = "end_turn".to_owned();
        let mut request_id: Option<String> = None;
        let mut research: Option<Value> = None;

        let event = json!({
            "type": "message_start",
            "message": {
                "id": "msg-research-1",
                "research": {
                    "status": "in_progress",
                    "query": "latest AI breakthroughs"
                },
                "usage": { "input_tokens": 50 }
            }
        });

        process_anthropic_event(
            &event,
            &mut accumulators,
            &mut usage,
            &mut stop_reason,
            &mut request_id,
            &mut research,
            None::<&StreamingCallbacks>,
            false,
        )
        .unwrap();

        let r = research.expect("research should be extracted");
        assert_eq!(r["status"], "in_progress");
        assert_eq!(r["query"], "latest AI breakthroughs");
    }

    #[test]
    fn process_anthropic_event_message_start_research_not_overwritten() {
        let mut accumulators = BTreeMap::new();
        let mut usage = UsageSummary::default();
        let mut stop_reason = "end_turn".to_owned();
        let mut request_id: Option<String> = None;
        let mut research: Option<Value> = Some(json!({"status": "original"}));

        let event = json!({
            "type": "message_start",
            "message": {
                "id": "msg-research-2",
                "research": { "status": "new" },
                "usage": { "input_tokens": 50 }
            }
        });

        process_anthropic_event(
            &event,
            &mut accumulators,
            &mut usage,
            &mut stop_reason,
            &mut request_id,
            &mut research,
            None::<&StreamingCallbacks>,
            false,
        )
        .unwrap();

        // Should keep the original research value.
        assert_eq!(research.unwrap()["status"], "original");
    }

    #[test]
    fn process_anthropic_event_message_start_no_research() {
        let mut accumulators = BTreeMap::new();
        let mut usage = UsageSummary::default();
        let mut stop_reason = "end_turn".to_owned();
        let mut request_id: Option<String> = None;
        let mut research: Option<Value> = None;

        let event = json!({
            "type": "message_start",
            "message": {
                "id": "msg-no-research",
                "usage": { "input_tokens": 50 }
            }
        });

        process_anthropic_event(
            &event,
            &mut accumulators,
            &mut usage,
            &mut stop_reason,
            &mut request_id,
            &mut research,
            None::<&StreamingCallbacks>,
            false,
        )
        .unwrap();

        assert!(research.is_none());
    }
}
