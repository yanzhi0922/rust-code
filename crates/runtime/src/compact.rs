use crate::extract_memories::{
    extract_memories, summarize_memories, ExtractMemoriesConfig, MemoryEntry, MemoryStore,
};
use crate::session::{ContentBlock, ConversationMessage, MessageRole};

const MAX_TOKENS_BEFORE_COMPACT: usize = 150_000;
const COMPACT_TO_TOKENS: usize = 80_000;
const MICRO_COMPACT_THRESHOLD: usize = 100_000;
const MICRO_COMPACT_TARGET: usize = 60_000;
const SNIP_MAX_LINES: usize = 50;
const SESSION_MEMORY_MAX_ENTRIES: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)]
pub enum CompactMode {
    Auto,
    Manual,
    Micro,
    Snip,
    SessionMemory,
}

impl Default for CompactMode {
    fn default() -> Self {
        Self::Auto
    }
}

pub struct CompactStrategy {
    pub max_tokens: usize,
    pub compact_to: usize,
    pub mode: CompactMode,
}

impl Default for CompactStrategy {
    fn default() -> Self {
        Self {
            max_tokens: MAX_TOKENS_BEFORE_COMPACT,
            compact_to: COMPACT_TO_TOKENS,
            mode: CompactMode::Auto,
        }
    }
}

impl CompactStrategy {
    pub fn new(mode: CompactMode) -> Self {
        Self {
            mode,
            ..Default::default()
        }
    }

    pub fn needs_compaction(&self, messages: &[ConversationMessage]) -> bool {
        self.estimate_tokens(messages) > self.max_tokens
    }

    pub fn needs_micro_compaction(&self, messages: &[ConversationMessage]) -> bool {
        self.estimate_tokens(messages) > MICRO_COMPACT_THRESHOLD
    }

    pub fn compact(&self, messages: &[ConversationMessage]) -> Vec<ConversationMessage> {
        match self.mode {
            CompactMode::Auto => self.auto_compact(messages),
            CompactMode::Micro => self.micro_compact(messages),
            CompactMode::Snip => self.snip_compact(messages),
            CompactMode::SessionMemory => self.session_memory_compact(messages),
            CompactMode::Manual => self.auto_compact(messages),
        }
    }

    pub fn auto_compact(&self, messages: &[ConversationMessage]) -> Vec<ConversationMessage> {
        if messages.len() <= 2 {
            return messages.to_vec();
        }

        let mut compacted = Vec::new();

        if let Some(first) = messages.first() {
            compacted.push(first.clone());
        }

        let mut summary = String::from("[Previous conversation summary]\n");
        let middle = &messages[1..messages.len().saturating_sub(1)];

        let mut turn_num = 0;
        let mut i = 0;
        while i < middle.len() {
            turn_num += 1;
            summary.push_str(&format!("--- Turn {turn_num} ---\n"));

            let msg = &middle[i];
            let role_str = match msg.role {
                MessageRole::System => "System",
                MessageRole::User => "User",
                MessageRole::Assistant => "Assistant",
            };

            let text = msg.text_content();
            if !text.is_empty() {
                let truncated = if text.len() > 500 {
                    format!(
                        "{}... (truncated, {} chars total)",
                        &text[..500],
                        text.len()
                    )
                } else {
                    text
                };
                summary.push_str(&format!("{role_str}: {truncated}\n"));
            }

            for tool_use in msg.tool_uses() {
                if let ContentBlock::ToolUse {
                    id, name, input, ..
                } = tool_use
                {
                    summary.push_str(&format!("  Tool call: {name} (id: {id})\n"));
                    let input_str = input.to_string();
                    if input_str.len() > 200 {
                        summary.push_str(&format!(
                            "    Input: {}... (truncated)\n",
                            &input_str[..200]
                        ));
                    } else {
                        summary.push_str(&format!("    Input: {input_str}\n"));
                    }
                }
            }

            if i + 1 < middle.len() {
                let next_msg = &middle[i + 1];
                if matches!(next_msg.role, MessageRole::User) {
                    for block in &next_msg.content {
                        if let ContentBlock::ToolResult {
                            tool_use_id,
                            content,
                            is_error,
                        } = block
                        {
                            let status =
                                is_error.unwrap_or(false).then_some(" ERROR").unwrap_or("");
                            let truncated_content = if content.len() > 300 {
                                format!(
                                    "{}... (truncated, {} chars total)",
                                    &content[..300],
                                    content.len()
                                )
                            } else {
                                content.clone()
                            };
                            summary.push_str(&format!(
                                "  Tool result ({tool_use_id}){status}: {truncated_content}\n"
                            ));
                        }
                    }
                    i += 1;
                }
            }

            i += 1;
        }

        let total_original = self.estimate_tokens(messages);
        let total_compacted = self.estimate_tokens_text(&summary);
        summary.push_str(&format!(
            "\n[Compacted from ~{total_original} to ~{total_compacted} estimated tokens]\n"
        ));

        compacted.push(ConversationMessage::system(summary));

        if let Some(last) = messages.last() {
            if messages.len() > 1 {
                compacted.push(last.clone());
            }
        }

        compacted
    }

    pub fn micro_compact(&self, messages: &[ConversationMessage]) -> Vec<ConversationMessage> {
        if messages.len() <= 2 {
            return messages.to_vec();
        }

        let mut compacted: Vec<ConversationMessage> = Vec::new();
        let mut current_text = String::new();
        let mut text_messages: Vec<ConversationMessage> = Vec::new();

        for msg in messages {
            let text = msg.text_content();
            let has_tool_use = !msg.tool_uses().is_empty();
            let has_tool_result = msg
                .content
                .iter()
                .any(|b| matches!(b, ContentBlock::ToolResult { .. }));

            if has_tool_use || has_tool_result {
                if !current_text.is_empty() {
                    let compacted_msg = ConversationMessage {
                        role: if text_messages
                            .first()
                            .map(|m| matches!(m.role, MessageRole::Assistant))
                            .unwrap_or(false)
                        {
                            MessageRole::Assistant
                        } else {
                            MessageRole::User
                        },
                        content: vec![ContentBlock::Text {
                            text: self.truncate_text(&current_text, MICRO_COMPACT_TARGET / 4),
                        }],
                        model: text_messages.first().and_then(|m| m.model.clone()),
                    };
                    compacted.push(compacted_msg);
                    current_text.clear();
                    text_messages.clear();
                }
                compacted.push(self.truncate_tool_message(msg));
            } else if !text.is_empty() {
                if text_messages.len() == 1 && text_messages[0].role != msg.role {
                    let compacted_msg = ConversationMessage {
                        role: text_messages[0].role.clone(),
                        content: vec![ContentBlock::Text {
                            text: self.truncate_text(&current_text, MICRO_COMPACT_TARGET / 4),
                        }],
                        model: text_messages[0].model.clone(),
                    };
                    compacted.push(compacted_msg);
                    current_text = text;
                    text_messages = vec![msg.clone()];
                } else {
                    current_text.push_str(&text);
                    current_text.push('\n');
                    text_messages.push(msg.clone());
                }
            } else {
                compacted.push(msg.clone());
            }
        }

        if !current_text.is_empty() {
            let compacted_msg = ConversationMessage {
                role: if text_messages
                    .first()
                    .map(|m| matches!(m.role, MessageRole::Assistant))
                    .unwrap_or(false)
                {
                    MessageRole::Assistant
                } else {
                    MessageRole::User
                },
                content: vec![ContentBlock::Text {
                    text: self.truncate_text(&current_text, MICRO_COMPACT_TARGET / 4),
                }],
                model: text_messages.first().and_then(|m| m.model.clone()),
            };
            compacted.push(compacted_msg);
        }

        compacted
    }

    pub fn snip_compact(&self, messages: &[ConversationMessage]) -> Vec<ConversationMessage> {
        if messages.len() <= 2 {
            return messages.to_vec();
        }

        let mut compacted: Vec<ConversationMessage> = Vec::new();
        let mut skipped = 0usize;

        for (i, msg) in messages.iter().enumerate() {
            let is_first = i == 0;
            let is_last = i == messages.len() - 1;
            let has_tools = !msg.tool_uses().is_empty()
                || msg
                    .content
                    .iter()
                    .any(|b| matches!(b, ContentBlock::ToolResult { .. }));

            if is_first || is_last || has_tools {
                compacted.push(msg.clone());
                continue;
            }

            let text = msg.text_content();
            let line_count = text.lines().count();

            if line_count > SNIP_MAX_LINES {
                let snipped = text
                    .lines()
                    .take(SNIP_MAX_LINES / 2)
                    .collect::<Vec<_>>()
                    .join("\n");
                let snipped = format!(
                    "{snipped}\n\n... ({snipped_count} lines snipped) ...\n",
                    snipped_count = line_count - SNIP_MAX_LINES
                );
                compacted.push(ConversationMessage {
                    role: msg.role.clone(),
                    content: vec![ContentBlock::Text { text: snipped }],
                    model: msg.model.clone(),
                });
                skipped += line_count - SNIP_MAX_LINES;
            } else {
                compacted.push(msg.clone());
            }
        }

        if skipped > 0 {
            compacted.insert(
                1,
                ConversationMessage::system(format!(
                    "[Context snipped: {skipped} lines removed from long messages]"
                )),
            );
        }

        compacted
    }

    pub fn session_memory_compact(
        &self,
        messages: &[ConversationMessage],
    ) -> Vec<ConversationMessage> {
        if messages.len() <= 2 {
            return messages.to_vec();
        }

        let mut compacted: Vec<ConversationMessage> = Vec::new();

        if let Some(first) = messages.first() {
            compacted.push(first.clone());
        }

        let config = ExtractMemoriesConfig::default();
        let memories = extract_memories(messages, &config);

        let truncated: Vec<MemoryEntry> = memories
            .into_iter()
            .take(SESSION_MEMORY_MAX_ENTRIES)
            .collect();

        if !truncated.is_empty() {
            let summary = summarize_memories(&truncated);
            compacted.push(ConversationMessage::system(summary));
        }

        if let Some(last) = messages.last() {
            if messages.len() > 1 {
                compacted.push(last.clone());
            }
        }

        compacted
    }

    pub fn session_memory_compact_with_store(
        &self,
        messages: &[ConversationMessage],
        store: &mut MemoryStore,
    ) -> Vec<ConversationMessage> {
        if messages.len() <= 2 {
            return messages.to_vec();
        }

        let mut compacted: Vec<ConversationMessage> = Vec::new();

        if let Some(first) = messages.first() {
            compacted.push(first.clone());
        }

        let config = ExtractMemoriesConfig::default();
        let entries = extract_memories(messages, &config);
        store.add_many(entries.clone());
        store.consolidate();

        let store_entries: Vec<MemoryEntry> = store
            .memories
            .iter()
            .take(SESSION_MEMORY_MAX_ENTRIES)
            .cloned()
            .collect();

        if !store_entries.is_empty() {
            let summary = summarize_memories(&store_entries);
            compacted.push(ConversationMessage::system(summary));
        }

        if let Some(last) = messages.last() {
            if messages.len() > 1 {
                compacted.push(last.clone());
            }
        }

        compacted
    }

    fn truncate_tool_message(&self, msg: &ConversationMessage) -> ConversationMessage {
        let content: Vec<ContentBlock> = msg
            .content
            .iter()
            .map(|block| match block {
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => {
                    let truncated = self.truncate_text(content, 500);
                    ContentBlock::ToolResult {
                        tool_use_id: tool_use_id.clone(),
                        content: truncated,
                        is_error: *is_error,
                    }
                }
                other => other.clone(),
            })
            .collect();

        ConversationMessage {
            content,
            ..msg.clone()
        }
    }

    fn truncate_text(&self, text: &str, max_chars: usize) -> String {
        if text.len() <= max_chars {
            return text.to_string();
        }
        let truncated = &text[..max_chars];
        if let Some(last_newline) = truncated.rfind('\n') {
            format!(
                "{}\n... (truncated from {} chars total) ...",
                &text[..last_newline],
                text.len()
            )
        } else {
            format!("{}... (truncated)", truncated)
        }
    }

    pub fn estimate_tokens(&self, messages: &[ConversationMessage]) -> usize {
        let mut total = 0usize;
        for msg in messages {
            for block in &msg.content {
                match block {
                    ContentBlock::Text { text } => total += crate::usage::estimate_tokens(text),
                    ContentBlock::ToolUse { name, input, .. } => {
                        total += crate::usage::estimate_tokens(name);
                        total += crate::usage::estimate_tokens(&input.to_string());
                    }
                    ContentBlock::ToolResult { content, .. } => {
                        total += crate::usage::estimate_tokens(content);
                    }
                    ContentBlock::Thinking { thinking } => {
                        total += crate::usage::estimate_tokens(thinking);
                    }
                    ContentBlock::Image { source } => {
                        total += source.data.len() / 4;
                    }
                }
            }
        }
        total
    }

    fn estimate_tokens_text(&self, text: &str) -> usize {
        crate::usage::estimate_tokens(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_user_msg(text: &str) -> ConversationMessage {
        ConversationMessage::user(text)
    }

    fn make_assistant_msg(text: &str) -> ConversationMessage {
        ConversationMessage::assistant_text(text)
    }

    #[test]
    fn test_needs_compaction_short_conversation() {
        let strategy = CompactStrategy::default();
        let messages = vec![make_user_msg("hello"), make_assistant_msg("hi")];
        assert!(!strategy.needs_compaction(&messages));
    }

    #[test]
    fn test_auto_compact_preserves_first_and_last() {
        let strategy = CompactStrategy::default();
        let messages = vec![
            make_user_msg("first"),
            make_assistant_msg("middle"),
            make_user_msg("second middle"),
            make_assistant_msg("last"),
        ];
        let compacted = strategy.auto_compact(&messages);
        assert_eq!(compacted.len(), 3);
        assert_eq!(compacted[0].text_content(), "first");
        assert_eq!(compacted[2].text_content(), "last");
    }

    #[test]
    fn test_snip_compact() {
        let strategy = CompactStrategy::new(CompactMode::Snip);
        let long_text: String = (0..100)
            .map(|i| format!("Line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let messages = vec![
            make_user_msg("first"),
            make_assistant_msg(&long_text),
            make_user_msg("last"),
        ];
        let compacted = strategy.snip_compact(&messages);
        assert!(compacted.len() >= 3);
        let snipped_text = compacted[1].text_content();
        assert!(snipped_text.contains("snipped"));
    }

    #[test]
    fn test_session_memory_extract() {
        let strategy = CompactStrategy::new(CompactMode::SessionMemory);
        let messages = vec![
            make_user_msg("first"),
            make_assistant_msg("NOTE: The parser uses regex for tokenization\nDECISION: Keep using regex\nTODO: Fix edge case in tokenizer"),
            make_user_msg("last"),
        ];
        let compacted = strategy.session_memory_compact(&messages);
        let memory_msg = compacted
            .iter()
            .find(|m| m.role == MessageRole::System && m.text_content().contains("Memory Summary"));
        assert!(memory_msg.is_some());
        let text = memory_msg.unwrap().text_content();
        assert!(text.contains("NOTE: The parser uses regex"));
        assert!(text.contains("DECISION: Keep using regex"));
        assert!(text.contains("TODO: Fix edge case"));
    }

    #[test]
    fn test_session_memory_compact_with_store() {
        let strategy = CompactStrategy::new(CompactMode::SessionMemory);
        let messages = vec![
            make_user_msg("first"),
            make_assistant_msg("NOTE: important insight\nDECISION: use async runtime"),
            make_user_msg("last"),
        ];
        let mut store = MemoryStore::new();
        let compacted = strategy.session_memory_compact_with_store(&messages, &mut store);
        assert!(compacted.len() >= 2);
        assert!(store.len() >= 2);
        let memory_msg = compacted
            .iter()
            .find(|m| m.role == MessageRole::System && m.text_content().contains("Memory Summary"));
        assert!(memory_msg.is_some());
    }
}
