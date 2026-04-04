use crate::error::ApiError;
use claude_runtime::conversation::AssistantEvent;
use claude_runtime::session::TokenUsage;
use futures::StreamExt;
use reqwest;
use std::pin::Pin;

#[derive(Debug)]
pub struct SseStreamParser {
    buffer: String,
    current_tool_id: String,
}

impl SseStreamParser {
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
            current_tool_id: String::new(),
        }
    }

    pub fn feed(&mut self, data: &str) -> Vec<AssistantEvent> {
        let mut events = Vec::new();
        for line in data.lines() {
            let line = line.trim();
            let Some(data) = line.strip_prefix("data: ") else {
                continue;
            };
            if data == "[DONE]" {
                continue;
            }
            if data.is_empty() {
                continue;
            }

            self.buffer.clear();
            self.buffer.push_str(data);

            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&self.buffer) {
                let event_type = parsed.get("type").and_then(|t| t.as_str()).unwrap_or("");
                match event_type {
                    "content_block_start" => {
                        if let Some(block) = parsed.get("content_block") {
                            let bt = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
                            match bt {
                                "tool_use" => {
                                    self.current_tool_id = block
                                        .get("id")
                                        .and_then(|i| i.as_str())
                                        .unwrap_or("")
                                        .to_string();
                                    let name = block
                                        .get("name")
                                        .and_then(|n| n.as_str())
                                        .unwrap_or("")
                                        .to_string();
                                    events.push(AssistantEvent::ToolUseStart {
                                        id: self.current_tool_id.clone(),
                                        name,
                                    });
                                }
                                "thinking" => {
                                    if let Some(t) = block.get("thinking").and_then(|t| t.as_str())
                                    {
                                        events.push(AssistantEvent::Thinking {
                                            text: t.to_string(),
                                        });
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    "content_block_delta" => {
                        if let Some(delta) = parsed.get("delta") {
                            if let Some(text) = delta.get("text").and_then(|t| t.as_str()) {
                                events.push(AssistantEvent::Text {
                                    text: text.to_string(),
                                });
                            }
                            if let Some(json_str) =
                                delta.get("partial_json").and_then(|j| j.as_str())
                            {
                                events.push(AssistantEvent::ToolUseInputDelta {
                                    id: self.current_tool_id.clone(),
                                    delta: json_str.to_string(),
                                });
                            }
                            if let Some(t) = delta.get("thinking").and_then(|t| t.as_str()) {
                                events.push(AssistantEvent::Thinking {
                                    text: t.to_string(),
                                });
                            }
                        }
                    }
                    "content_block_stop" => {
                        if !self.current_tool_id.is_empty() {
                            events.push(AssistantEvent::ToolUseEnd {
                                id: self.current_tool_id.clone(),
                            });
                        }
                    }
                    "message_delta" => {
                        if let Some(usage) = parsed.get("usage") {
                            let out = usage
                                .get("output_tokens")
                                .and_then(|t| t.as_u64())
                                .unwrap_or(0) as u32;
                            events.push(AssistantEvent::Usage {
                                usage: TokenUsage {
                                    output_tokens: out,
                                    ..Default::default()
                                },
                            });
                        }
                    }
                    "message_start" => {
                        if let Some(msg) = parsed.get("message") {
                            if let Some(usage) = msg.get("usage") {
                                let inp = usage
                                    .get("input_tokens")
                                    .and_then(|t| t.as_u64())
                                    .unwrap_or(0) as u32;
                                let cc = usage
                                    .get("cache_creation_input_tokens")
                                    .and_then(|t| t.as_u64())
                                    .unwrap_or(0) as u32;
                                let cr = usage
                                    .get("cache_read_input_tokens")
                                    .and_then(|t| t.as_u64())
                                    .unwrap_or(0) as u32;
                                events.push(AssistantEvent::Usage {
                                    usage: TokenUsage {
                                        input_tokens: inp,
                                        cache_creation_input_tokens: cc,
                                        cache_read_input_tokens: cr,
                                        ..Default::default()
                                    },
                                });
                            }
                        }
                    }
                    "message_stop" => {
                        events.push(AssistantEvent::Stop {
                            reason: "end_turn".to_string(),
                        });
                    }
                    _ => {}
                }
            }
        }
        events
    }
}

pub fn create_sse_stream(
    response: reqwest::Response,
) -> Pin<Box<dyn futures::Stream<Item = Result<AssistantEvent, ApiError>> + Send>> {
    let parser = std::sync::Mutex::new(SseStreamParser::new());
    let stream = response
        .bytes_stream()
        .filter_map(|result: Result<bytes::Bytes, reqwest::Error>| async move {
            result.ok()
        })
        .flat_map(move |bytes: bytes::Bytes| {
            let text = String::from_utf8_lossy(&bytes).to_string();
            let mut parser_guard = parser.lock().unwrap();
            let events = parser_guard.feed(&text);
            futures::stream::iter(events.into_iter().map(Ok))
        });
    Box::pin(stream)
}

pub fn parse_sse_events(data: &str) -> Result<Vec<AssistantEvent>, ApiError> {
    let mut parser = SseStreamParser::new();
    Ok(parser.feed(data))
}
