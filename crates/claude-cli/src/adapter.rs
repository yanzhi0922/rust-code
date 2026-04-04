use claude_api::client::ProviderClient;
use claude_api::error::ApiError;
use claude_api::types::ResponseContentBlock;
use claude_runtime::conversation::{ApiClient, AssistantEvent, QueryConfig, ToolExecutor};
use claude_runtime::session::ConversationMessage;
use claude_tools::{ToolContext, ToolRegistry};
use futures::StreamExt;
use std::future::Future;
use std::pin::Pin;

pub struct ApiClientAdapter<T: ProviderClient> {
    client: T,
}

impl<T: ProviderClient> ApiClientAdapter<T> {
    pub fn new(client: T) -> Self {
        Self { client }
    }
}

fn api_error_to_anyhow(e: ApiError) -> anyhow::Error {
    anyhow::anyhow!("{e}")
}

impl<T: ProviderClient + 'static> ApiClient for ApiClientAdapter<T> {
    fn send_message(
        &self,
        messages: Vec<ConversationMessage>,
        system: &str,
        config: &QueryConfig,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<Vec<AssistantEvent>>> + Send + '_>> {
        let client = self.client.create_message(messages, system.to_string(), config.clone());
        Box::pin(async move {
            let response = client.await.map_err(api_error_to_anyhow)?;
            let mut events = Vec::new();

            for block in &response.content {
                match block {
                    ResponseContentBlock::Text { text } => {
                        events.push(AssistantEvent::Text { text: text.clone() });
                    }
                    ResponseContentBlock::ToolUse { id, name, input } => {
                        events.push(AssistantEvent::ToolUseStart {
                            id: id.clone(),
                            name: name.clone(),
                        });
                        events.push(AssistantEvent::ToolUseInputDelta {
                            id: id.clone(),
                            delta: serde_json::to_string(input).unwrap_or_default(),
                        });
                        events.push(AssistantEvent::ToolUseEnd { id: id.clone() });
                    }
                }
            }

            events.push(AssistantEvent::Usage {
                usage: response.usage.into(),
            });

            events.push(AssistantEvent::Stop {
                reason: response.stop_reason.clone().unwrap_or_else(|| "end_turn".to_string()),
            });

            Ok(events)
        })
    }

    fn stream_message(
        &self,
        messages: Vec<ConversationMessage>,
        system: &str,
        config: &QueryConfig,
    ) -> Pin<
        Box<
            dyn Future<
                    Output = Result<
                        Pin<Box<dyn futures::Stream<Item = Result<AssistantEvent, anyhow::Error>> + Send>>,
                        anyhow::Error,
                    >,
                > + Send
                + '_,
        >,
    > {
        let client = self.client.stream_message(messages, system.to_string(), config.clone());
        Box::pin(async move {
            let stream = client.await.map_err(api_error_to_anyhow)?;
            let mapped: Pin<Box<dyn futures::Stream<Item = Result<AssistantEvent, anyhow::Error>> + Send>> =
                Box::pin(stream.map(|result| result.map_err(api_error_to_anyhow)));
            Ok(mapped)
        })
    }
}

pub struct ToolExecutorAdapter {
    registry: ToolRegistry,
    ctx: ToolContext,
}

impl ToolExecutorAdapter {
    pub fn new(registry: ToolRegistry, ctx: ToolContext) -> Self {
        Self { registry, ctx }
    }
}

impl ToolExecutor for ToolExecutorAdapter {
    fn execute<'a>(
        &'a self,
        tool_name: &'a str,
        input: serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<String>> + Send + 'a>> {
        let result = self.registry.execute(tool_name, input, &self.ctx);
        Box::pin(async move {
            let tool_result = result.await;
            Ok(tool_result.content)
        })
    }
}
