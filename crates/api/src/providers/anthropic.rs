use crate::client::{ProviderClient, ProviderConfig};
use crate::error::ApiError;
use crate::sse_parser::create_sse_stream;
use crate::types::{MessageRequest, MessageResponse, RequestMessage, ToolDefinition};
use claude_runtime::conversation::{AssistantEvent, QueryConfig};
use claude_runtime::session::{ConversationMessage, ContentBlock, MessageRole};
use futures::Stream;
use reqwest::Client;
use serde_json::json;
use std::future::Future;
use std::pin::Pin;

pub struct AnthropicClient {
    http: Client,
    config: ProviderConfig,
}

impl AnthropicClient {
    pub fn new(config: ProviderConfig) -> Self {
        let http = Client::builder()
            .timeout(std::time::Duration::from_secs(config.timeout_secs))
            .build()
            .expect("Failed to create HTTP client");

        Self { http, config }
    }

    fn build_request(
        &self,
        messages: &[ConversationMessage],
        system: &str,
        config: &QueryConfig,
        tools: Option<Vec<ToolDefinition>>,
        stream: bool,
    ) -> MessageRequest {
        MessageRequest {
            model: config.model.clone(),
            max_tokens: config.max_tokens,
            system: if system.is_empty() {
                None
            } else {
                Some(system.to_string())
            },
            messages: messages
                .iter()
                .map(|m| RequestMessage {
                    role: match m.role {
                        MessageRole::System => "user".to_string(),
                        MessageRole::User => "user".to_string(),
                        MessageRole::Assistant => "assistant".to_string(),
                    },
                    content: m
                        .content
                        .iter()
                        .map(|block| match block {
                            ContentBlock::Text { text } => {
                                json!({ "type": "text", "text": text })
                            }
                            ContentBlock::ToolUse { id, name, input, .. } => {
                                json!({ "type": "tool_use", "id": id, "name": name, "input": input })
                            }
                            ContentBlock::ToolResult { tool_use_id, content, is_error } => {
                                let mut obj = json!({
                                    "type": "tool_result",
                                    "tool_use_id": tool_use_id,
                                    "content": content
                                });
                                if let Some(true) = is_error {
                                    obj["is_error"] = json!(true);
                                }
                                obj
                            }
                            ContentBlock::Image { source } => {
                                json!({
                                    "type": "image",
                                    "source": {
                                        "type": source.source_type,
                                        "media_type": source.media_type,
                                        "data": source.data
                                    }
                                })
                            }
                            ContentBlock::Thinking { thinking } => {
                                json!({ "type": "thinking", "thinking": thinking })
                            }
                        })
                        .collect(),
                })
                .collect(),
            temperature: config.temperature,
            stream: if stream { Some(true) } else { None },
            tools,
        }
    }
}

impl ProviderClient for AnthropicClient {
    fn create_message(
        &self,
        messages: Vec<ConversationMessage>,
        system: String,
        config: QueryConfig,
    ) -> Pin<Box<dyn Future<Output = Result<MessageResponse, ApiError>> + Send>> {
        let request = self.build_request(&messages, &system, &config, None, false);
        let base_url = self.config.base_url.clone();
        let api_key = self.config.api_key.clone();
        let http = self.http.clone();

        Box::pin(async move {
            let mut req = http
                .post(format!("{base_url}/v1/messages"))
                .json(&request);

            if let Some(ref key) = api_key {
                req = req.header("x-api-key", key);
            }
            req = req.header("anthropic-version", "2023-06-01");

            let response = req.send().await?;
            let status = response.status();

            if status == reqwest::StatusCode::UNAUTHORIZED {
                return Err(ApiError::Auth("Invalid API key".to_string()));
            }
            if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                let retry_after = response
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse().ok());
                return Err(ApiError::RateLimit { retry_after_ms: retry_after });
            }
            if !status.is_success() {
                let body = response.text().await.unwrap_or_default();
                return Err(ApiError::Api {
                    status: status.as_u16(),
                    message: body,
                });
            }

            let body = response.text().await?;
            serde_json::from_str(&body).map_err(ApiError::from)
        })
    }

    fn stream_message(
        &self,
        messages: Vec<ConversationMessage>,
        system: String,
        config: QueryConfig,
    ) -> Pin<
        Box<
            dyn Future<
                    Output = Result<
                        Pin<Box<dyn Stream<Item = Result<AssistantEvent, ApiError>> + Send>>,
                        ApiError,
                    >,
                > + Send,
        >,
    > {
        let request = self.build_request(&messages, &system, &config, None, true);
        let base_url = self.config.base_url.clone();
        let api_key = self.config.api_key.clone();
        let http = self.http.clone();

        Box::pin(async move {
            let mut req = http
                .post(format!("{base_url}/v1/messages"))
                .json(&request);

            if let Some(ref key) = api_key {
                req = req.header("x-api-key", key);
            }
            req = req.header("anthropic-version", "2023-06-01");

            let response = req.send().await?;
            let status = response.status();

            if status == reqwest::StatusCode::UNAUTHORIZED {
                return Err(ApiError::Auth("Invalid API key".to_string()));
            }
            if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                let retry_after = response
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse().ok());
                return Err(ApiError::RateLimit { retry_after_ms: retry_after });
            }
            if !status.is_success() {
                let body = response.text().await.unwrap_or_default();
                return Err(ApiError::Api {
                    status: status.as_u16(),
                    message: body,
                });
            }

            let stream = create_sse_stream(response);
            Ok(stream)
        })
    }
}
