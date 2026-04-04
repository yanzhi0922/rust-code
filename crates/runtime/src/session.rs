use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    System,
    User,
    Assistant,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
    },
    Thinking {
        thinking: String,
    },
    Image {
        source: ImageSource,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ImageSource {
    #[serde(rename = "type")]
    pub source_type: String,
    pub media_type: String,
    pub data: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationMessage {
    pub role: MessageRole,
    pub content: Vec<ContentBlock>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

impl ConversationMessage {
    pub fn system(text: impl Into<String>) -> Self {
        Self {
            role: MessageRole::System,
            content: vec![ContentBlock::Text { text: text.into() }],
            model: None,
        }
    }

    pub fn user(text: impl Into<String>) -> Self {
        Self {
            role: MessageRole::User,
            content: vec![ContentBlock::Text { text: text.into() }],
            model: None,
        }
    }

    pub fn assistant_text(text: impl Into<String>) -> Self {
        Self {
            role: MessageRole::Assistant,
            content: vec![ContentBlock::Text { text: text.into() }],
            model: None,
        }
    }

    pub fn assistant_tool_use(id: &str, name: &str, input: serde_json::Value) -> Self {
        Self {
            role: MessageRole::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: id.to_string(),
                name: name.to_string(),
                input,
            }],
            model: None,
        }
    }

    pub fn tool_result(tool_use_id: &str, content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: tool_use_id.to_string(),
                content: content.into(),
                is_error: None,
            }],
            model: None,
        }
    }

    pub fn tool_result_error(tool_use_id: &str, content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: tool_use_id.to_string(),
                content: content.into(),
                is_error: Some(true),
            }],
            model: None,
        }
    }

    pub fn text_content(&self) -> String {
        self.content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }

    pub fn tool_uses(&self) -> Vec<&ContentBlock> {
        self.content
            .iter()
            .filter(|block| matches!(block, ContentBlock::ToolUse { .. }))
            .collect()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Session {
    pub id: String,
    pub messages: Vec<ConversationMessage>,
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

impl Session {
    pub fn new() -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            messages: Vec::new(),
            metadata: HashMap::new(),
        }
    }

    pub fn add_message(&mut self, message: ConversationMessage) {
        self.messages.push(message);
    }

    pub fn clear(&mut self) {
        self.messages.clear();
    }

    pub fn fork(&self) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            messages: self.messages.clone(),
            metadata: self.metadata.clone(),
        }
    }

    pub fn save_to_file(&self, path: &std::path::Path) -> anyhow::Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    pub fn load_from_file(path: &std::path::Path) -> anyhow::Result<Self> {
        let json = std::fs::read_to_string(path)?;
        let session: Session = serde_json::from_str(&json)?;
        Ok(session)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamEvent {
    #[serde(rename = "type")]
    pub event_type: StreamEventType,
    pub index: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delta: Option<ContentBlock>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_block: Option<ContentBlock>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<AssistantMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<TokenUsage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum StreamEventType {
    MessageStart,
    ContentBlockStart,
    ContentBlockDelta,
    ContentBlockStop,
    MessageDelta,
    MessageStop,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssistantMessage {
    pub id: String,
    #[serde(rename = "type")]
    pub msg_type: String,
    pub role: String,
    pub content: Vec<ContentBlock>,
    pub model: String,
    #[serde(default)]
    pub stop_reason: Option<String>,
    #[serde(default)]
    pub usage: TokenUsage,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    #[serde(default)]
    pub input_tokens: u32,
    #[serde(default)]
    pub output_tokens: u32,
    #[serde(default)]
    pub cache_creation_input_tokens: u32,
    #[serde(default)]
    pub cache_read_input_tokens: u32,
}

impl TokenUsage {
    pub fn total_input(&self) -> u32 {
        self.input_tokens + self.cache_creation_input_tokens + self.cache_read_input_tokens
    }

    pub fn total(&self) -> u32 {
        self.total_input() + self.output_tokens
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_user_message_creation() {
        let msg = ConversationMessage::user("hello world");
        assert_eq!(msg.role, MessageRole::User);
        assert_eq!(msg.content.len(), 1);
        match &msg.content[0] {
            ContentBlock::Text { text } => assert_eq!(text, "hello world"),
            _ => panic!("Expected Text block"),
        }
        assert!(msg.model.is_none());
    }

    #[test]
    fn test_assistant_tool_use_message() {
        let msg = ConversationMessage::assistant_tool_use(
            "tool-123",
            "BashTool",
            serde_json::json!({"command": "ls"}),
        );
        assert_eq!(msg.role, MessageRole::Assistant);
        assert_eq!(msg.content.len(), 1);
        match &msg.content[0] {
            ContentBlock::ToolUse { id, name, input } => {
                assert_eq!(id, "tool-123");
                assert_eq!(name, "BashTool");
                assert_eq!(input["command"], "ls");
            }
            _ => panic!("Expected ToolUse block"),
        }
    }

    #[test]
    fn test_tool_result_message() {
        let err = ConversationMessage::tool_result_error("tool-123", "file not found");
        assert_eq!(err.role, MessageRole::User);
        match &err.content[0] {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                assert_eq!(tool_use_id, "tool-123");
                assert_eq!(content, "file not found");
                assert_eq!(is_error, &Some(true));
            }
            _ => panic!("Expected ToolResult block"),
        }

        let ok = ConversationMessage::tool_result("tool-123", "success");
        match &ok.content[0] {
            ContentBlock::ToolResult { is_error, .. } => {
                assert_eq!(is_error, &None);
            }
            _ => panic!("Expected ToolResult block"),
        }
    }

    #[test]
    fn test_text_content_extraction() {
        let msg =
            ConversationMessage::assistant_tool_use("t1", "Bash", serde_json::json!({"cmd": "ls"}));
        assert!(msg.text_content().is_empty());

        let text_msg = ConversationMessage::user("hello");
        assert_eq!(text_msg.text_content(), "hello");
    }

    #[test]
    fn test_tool_uses_extraction() {
        let text_msg = ConversationMessage::user("hello");
        assert!(text_msg.tool_uses().is_empty());

        let tool_msg = ConversationMessage::assistant_tool_use(
            "t1",
            "ReadFile",
            serde_json::json!({"path": "/tmp"}),
        );
        let uses = tool_msg.tool_uses();
        assert_eq!(uses.len(), 1);
    }

    #[test]
    fn test_session_create_and_fork() {
        let mut session = Session::new();
        session.add_message(ConversationMessage::user("hello"));
        assert_eq!(session.messages.len(), 1);

        let mut forked = session.fork();
        assert_ne!(forked.id, session.id);
        assert_eq!(forked.messages.len(), session.messages.len());

        forked.add_message(ConversationMessage::assistant_text("hi"));
        assert_eq!(forked.messages.len(), 2);
        assert_eq!(session.messages.len(), 1);
    }

    #[test]
    fn test_session_save_and_load() {
        let mut session = Session::new();
        session.add_message(ConversationMessage::user("test message"));
        let dir = std::env::temp_dir().join("claude_test_session");
        std::fs::create_dir_all(&dir).ok();
        let path = dir.join("session.json");

        session.save_to_file(&path).expect("save failed");
        let loaded = Session::load_from_file(&path).expect("load failed");
        assert_eq!(loaded.id, session.id);
        assert_eq!(loaded.messages.len(), 1);
        assert_eq!(loaded.messages[0].text_content(), "test message");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_token_usage_total() {
        let usage = TokenUsage {
            input_tokens: 100,
            output_tokens: 50,
            cache_creation_input_tokens: 20,
            cache_read_input_tokens: 30,
        };
        assert_eq!(usage.total_input(), 150);
        assert_eq!(usage.total(), 200);

        let default = TokenUsage::default();
        assert_eq!(default.total(), 0);
    }
}
