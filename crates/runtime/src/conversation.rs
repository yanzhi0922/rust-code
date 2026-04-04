use crate::compact::CompactStrategy;
use crate::session::{ContentBlock, ConversationMessage, MessageRole, TokenUsage};
use futures::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::pin::Pin;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryConfig {
    pub model: String,
    pub max_tokens: u32,
    pub system_prompt: String,
    pub max_turns: u32,
    pub max_budget_usd: Option<f64>,
    pub temperature: Option<f32>,
    pub stream: bool,
}

impl Default for QueryConfig {
    fn default() -> Self {
        Self {
            model: "claude-sonnet-4-20250514".to_string(),
            max_tokens: 8192,
            system_prompt: String::new(),
            max_turns: 100,
            max_budget_usd: None,
            temperature: None,
            stream: true,
        }
    }
}

impl QueryConfig {
    pub fn from_runtime_config(config: &crate::config::RuntimeConfig) -> Self {
        Self {
            model: config.model.clone(),
            max_tokens: config.api.max_tokens,
            temperature: config.api.temperature,
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AssistantEvent {
    Text { text: String },
    ToolUseStart { id: String, name: String },
    ToolUseInputDelta { id: String, delta: String },
    ToolUseEnd { id: String },
    Thinking { text: String },
    Usage { usage: TokenUsage },
    Stop { reason: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SdkMessage {
    Assistant { text: String },
    ToolUse { id: String, name: String, input: serde_json::Value },
    ToolResult { tool_use_id: String, content: String, is_error: bool },
    Error { message: String },
    Usage { input_tokens: u32, output_tokens: u32 },
    End { reason: String },
}

pub trait ToolExecutor: Send + Sync {
    fn execute<'a>(
        &'a self,
        tool_name: &'a str,
        input: serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<String>> + Send + 'a>>;
}

pub trait ApiClient: Send + Sync {
    fn send_message(
        &self,
        messages: Vec<ConversationMessage>,
        system: &str,
        config: &QueryConfig,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<Vec<AssistantEvent>>> + Send + '_>>;

    fn stream_message(
        &self,
        messages: Vec<ConversationMessage>,
        system: &str,
        config: &QueryConfig,
    ) -> Pin<
        Box<
            dyn Future<
                    Output = Result<
                        Pin<Box<dyn Stream<Item = Result<AssistantEvent, anyhow::Error>> + Send>>,
                        anyhow::Error,
                    >,
                > + Send
                + '_,
        >,
    >;
}

pub struct ConversationRuntime {
    pub config: QueryConfig,
    pub messages: Vec<ConversationMessage>,
    pub permissions: crate::permissions::PermissionPolicy,
    pub api_client: Box<dyn ApiClient>,
    pub tool_executor: Box<dyn ToolExecutor>,
    pub total_usage: TokenUsage,
    pub turn_count: u32,
    pub compact_strategy: CompactStrategy,
}

impl ConversationRuntime {
    pub fn new(
        config: QueryConfig,
        permissions: crate::permissions::PermissionPolicy,
        api_client: Box<dyn ApiClient>,
        tool_executor: Box<dyn ToolExecutor>,
    ) -> Self {
        Self {
            config,
            messages: Vec::new(),
            permissions,
            api_client,
            tool_executor,
            total_usage: TokenUsage::default(),
            turn_count: 0,
            compact_strategy: CompactStrategy::default(),
        }
    }

    pub async fn submit_message(
        &mut self,
        user_message: impl Into<String>,
    ) -> anyhow::Result<Vec<SdkMessage>> {
        let text = user_message.into();
        let mut results = Vec::new();
        self.messages.push(ConversationMessage::user(&text));

        loop {
            if self.turn_count >= self.config.max_turns {
                results.push(SdkMessage::End {
                    reason: "max_turns".to_string(),
                });
                break;
            }

            if self.compact_strategy.needs_compaction(&self.messages) {
                self.messages = self.compact_strategy.compact(&self.messages);
            }

            self.turn_count += 1;

            let events = if self.config.stream {
                self.receive_streaming().await?
            } else {
                self.api_client
                    .send_message(
                        self.messages.clone(),
                        &self.config.system_prompt,
                        &self.config,
                    )
                    .await?
            };

            let mut tool_calls = Vec::new();
            let mut current_text = String::new();

            for event in &events {
                match event {
                    AssistantEvent::Text { text } => current_text.push_str(text),
                    AssistantEvent::Usage { usage } => {
                        self.total_usage.input_tokens += usage.input_tokens;
                        self.total_usage.output_tokens += usage.output_tokens;
                        self.total_usage.cache_creation_input_tokens += usage.cache_creation_input_tokens;
                        self.total_usage.cache_read_input_tokens += usage.cache_read_input_tokens;
                    }
                    _ => {}
                }
            }

            if !current_text.is_empty() {
                results.push(SdkMessage::Assistant { text: current_text });
            }

            for event in &events {
                if let AssistantEvent::ToolUseStart { id, name } = event {
                    tool_calls.push((id.clone(), name.clone()));
                }
            }

            if tool_calls.is_empty() {
                let stop_reason = events
                    .iter()
                    .find_map(|e| match e {
                        AssistantEvent::Stop { reason } => Some(reason.clone()),
                        _ => None,
                    })
                    .unwrap_or_else(|| "end_turn".to_string());

                results.push(SdkMessage::End { reason: stop_reason });
                break;
            }

            let mut assistant_content = Vec::new();
            let mut tool_inputs = Vec::new();

            for event in &events {
                match event {
                    AssistantEvent::Text { text } => {
                        assistant_content.push(ContentBlock::Text { text: text.clone() });
                    }
                    AssistantEvent::Thinking { text } => {
                        assistant_content.push(ContentBlock::Thinking {
                            thinking: text.clone(),
                        });
                    }
                    AssistantEvent::ToolUseStart { id, name } => {
                        tool_inputs.push((id.clone(), name.clone(), String::new()));
                    }
                    AssistantEvent::ToolUseInputDelta { id, delta } => {
                        if let Some(entry) = tool_inputs.iter_mut().find(|(i, _, _)| i == id) {
                            entry.2.push_str(delta);
                        }
                    }
                    _ => {}
                }
            }

            for (id, name, input_str) in &tool_inputs {
                let input: serde_json::Value =
                    serde_json::from_str(input_str).unwrap_or(serde_json::json!({}));
                assistant_content.push(ContentBlock::ToolUse {
                    id: id.clone(),
                    name: name.clone(),
                    input: input.clone(),
                });

                results.push(SdkMessage::ToolUse {
                    id: id.clone(),
                    name: name.clone(),
                    input: input.clone(),
                });
            }

            self.messages.push(ConversationMessage {
                role: MessageRole::Assistant,
                content: assistant_content,
                model: Some(self.config.model.clone()),
            });

            let mut tool_results = Vec::new();
            for (id, name, input_str) in &tool_inputs {
                let input: serde_json::Value =
                    serde_json::from_str(input_str).unwrap_or(serde_json::json!({}));
                let perm_result = self.permissions.evaluate(name);

                let result = match perm_result {
                    crate::permissions::PermissionResult::Allow => {
                        self.tool_executor.execute(name, input).await
                    }
                    crate::permissions::PermissionResult::Deny => {
                        Ok("Permission denied: tool is blocked by policy.".to_string())
                    }
                    crate::permissions::PermissionResult::Ask => {
                        Ok(format!("Tool '{}' requires user approval. (Ask mode)", name))
                    }
                    crate::permissions::PermissionResult::Passthrough => {
                        self.tool_executor.execute(name, input).await
                    }
                };

                let (content, is_error) = match result {
                    Ok(output) => (output, false),
                    Err(e) => (format!("Error: {e}"), true),
                };

                results.push(SdkMessage::ToolResult {
                    tool_use_id: id.clone(),
                    content: content.clone(),
                    is_error,
                });

                tool_results.push(ContentBlock::ToolResult {
                    tool_use_id: id.clone(),
                    content,
                    is_error: Some(is_error),
                });
            }

            self.messages.push(ConversationMessage {
                role: MessageRole::User,
                content: tool_results,
                model: None,
            });
        }

        Ok(results)
    }

    async fn receive_streaming(&self) -> anyhow::Result<Vec<AssistantEvent>> {
        let stream = self.api_client.stream_message(
            self.messages.clone(),
            &self.config.system_prompt,
            &self.config,
        ).await?;
        let mut events = Vec::new();
        futures::pin_mut!(stream);
        while let Some(result) = stream.next().await {
            match result {
                Ok(event) => events.push(event),
                Err(e) => return Err(e),
            }
        }
        Ok(events)
    }
}
