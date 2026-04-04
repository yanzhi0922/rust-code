use crate::remote::RemoteContext;
use crate::session::{ContentBlock, ConversationMessage, MessageRole};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::ops::AddAssign;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

static MAX_SUGGESTIONS: usize = 5;
static MIN_HISTORY_FOR_PREDICTION: usize = 2;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptSuggestionConfig {
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default = "default_max_suggestions")]
    pub max_suggestions: usize,
    #[serde(default)]
    pub min_history: usize,
    #[serde(default)]
    pub remote_enabled: bool,
    #[serde(default)]
    pub analytics_enabled: bool,
}

fn default_enabled() -> bool {
    true
}

fn default_max_suggestions() -> usize {
    5
}

impl Default for PromptSuggestionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_suggestions: MAX_SUGGESTIONS,
            min_history: MIN_HISTORY_FOR_PREDICTION,
            remote_enabled: true,
            analytics_enabled: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Suggestion {
    pub text: String,
    pub confidence: f64,
    pub source: SuggestionSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SuggestionSource {
    ToolPattern,
    HistoryPattern,
    Remote,
    Fallback,
}

#[derive(Debug, Default)]
pub struct SuggestionAnalytics {
    pub total_generated: AtomicU64,
    pub total_accepted: AtomicU64,
    pub total_rejected: AtomicU64,
    pub by_source: std::sync::Mutex<HashMap<String, AtomicU64>>,
}

impl SuggestionAnalytics {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn record_generated(&self, source: &SuggestionSource) {
        self.total_generated.fetch_add(1, Ordering::Relaxed);
        let key = format!("{:?}", source);
        let map = self.by_source.lock().unwrap();
        if let Some(counter) = map.get(&key) {
            counter.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn record_accepted(&self) {
        self.total_accepted.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_rejected(&self) {
        self.total_rejected.fetch_add(1, Ordering::Relaxed);
    }

    pub fn acceptance_rate(&self) -> f64 {
        let total = self.total_accepted.load(Ordering::Relaxed)
            + self.total_rejected.load(Ordering::Relaxed);
        if total == 0 {
            0.0
        } else {
            self.total_accepted.load(Ordering::Relaxed) as f64 / total as f64
        }
    }
}

#[derive(Debug, Clone)]
struct ToolUsagePattern {
    tool_name: String,
    frequency: u32,
    common_followups: Vec<String>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct ExtractedMemory {
    tool_sequence: Vec<String>,
    user_intents: Vec<String>,
    file_patterns: Vec<String>,
}

pub struct PromptSuggestionEngine {
    pub config: PromptSuggestionConfig,
    pub enabled: bool,
    analytics: Arc<SuggestionAnalytics>,
    remote_context: RemoteContext,
}

impl PromptSuggestionEngine {
    pub fn new(config: PromptSuggestionConfig) -> Self {
        let enabled = config.enabled;
        Self {
            config,
            enabled,
            analytics: SuggestionAnalytics::new(),
            remote_context: RemoteContext::default(),
        }
    }

    pub fn with_remote(mut self, remote: RemoteContext) -> Self {
        self.remote_context = remote;
        self
    }

    pub fn analytics(&self) -> &Arc<SuggestionAnalytics> {
        &self.analytics
    }

    pub fn suggest(&self, history: &[ConversationMessage]) -> Vec<Suggestion> {
        if !self.enabled || history.len() < self.config.min_history {
            return Vec::new();
        }

        let memories = self.extract_memories(history);
        let mut suggestions = Vec::new();

        self.suggest_from_tool_patterns(&memories, &mut suggestions);
        self.suggest_from_history(history, &mut suggestions);
        self.suggest_from_remote(history, &mut suggestions);

        if suggestions.is_empty() {
            self.suggest_fallbacks(history, &mut suggestions);
        }

        suggestions.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        suggestions.truncate(self.config.max_suggestions);

        for s in &suggestions {
            self.analytics.record_generated(&s.source);
        }

        suggestions
    }

    fn extract_memories(&self, history: &[ConversationMessage]) -> ExtractedMemory {
        let mut tool_sequence = Vec::new();
        let mut user_intents = Vec::new();
        let mut file_patterns = Vec::new();

        for msg in history {
            for block in &msg.content {
                match block {
                    ContentBlock::ToolUse { name, input, .. } => {
                        tool_sequence.push(name.clone());
                        if name == "ReadFile" || name == "WriteFile" || name == "EditFile" {
                            if let Some(path) = input.get("path").and_then(|v| v.as_str()) {
                                file_patterns.push(path.to_string());
                            }
                        }
                    }
                    ContentBlock::Text { text } if msg.role == MessageRole::User => {
                        let trimmed = text.trim();
                        if !trimmed.is_empty() {
                            user_intents.push(trimmed.to_string());
                        }
                    }
                    _ => {}
                }
            }
        }

        ExtractedMemory {
            tool_sequence,
            user_intents,
            file_patterns,
        }
    }

    fn suggest_from_tool_patterns(
        &self,
        memories: &ExtractedMemory,
        suggestions: &mut Vec<Suggestion>,
    ) {
        let patterns = self.analyze_tool_patterns(&memories.tool_sequence);

        for pattern in &patterns {
            if pattern.frequency < 2 {
                continue;
            }

            for followup in &pattern.common_followups {
                let text = match pattern.tool_name.as_str() {
                    "ReadFile" => format!("Read the contents of {}", followup),
                    "WriteFile" => format!("Write changes to {}", followup),
                    "Bash" => format!("Run: {}", followup),
                    "Grep" => format!("Search for: {}", followup),
                    "EditFile" => format!("Edit {}", followup),
                    _ => format!("Use {} with {}", pattern.tool_name, followup),
                };
                suggestions.push(Suggestion {
                    text,
                    confidence: (0.5 + (pattern.frequency as f64 * 0.1).min(0.4)),
                    source: SuggestionSource::ToolPattern,
                    tool_name: Some(pattern.tool_name.clone()),
                });
            }
        }
    }

    fn analyze_tool_patterns(&self, tool_sequence: &[String]) -> Vec<ToolUsagePattern> {
        let mut freq: HashMap<String, u32> = HashMap::new();
        let mut followups: HashMap<String, HashMap<String, u32>> = HashMap::new();

        for tool in tool_sequence {
            *freq.entry(tool.clone()).or_default() += 1;
        }

        for window in tool_sequence.windows(2) {
            followups
                .entry(window[0].clone())
                .or_default()
                .entry(window[1].clone())
                .or_default()
                .add_assign(1);
        }

        let mut patterns = Vec::new();
        for (tool, count) in freq {
            let mut common: Vec<String> = followups
                .get(&tool)
                .map(|m| {
                    let mut entries: Vec<_> = m.iter().collect();
                    entries.sort_by(|a, b| b.1.cmp(a.1));
                    entries
                        .into_iter()
                        .take(3)
                        .map(|(k, _)| k.clone())
                        .collect()
                })
                .unwrap_or_default();

            common.dedup();
            patterns.push(ToolUsagePattern {
                tool_name: tool,
                frequency: count,
                common_followups: common,
            });
        }

        patterns.sort_by(|a, b| b.frequency.cmp(&a.frequency));
        patterns
    }

    fn suggest_from_history(
        &self,
        history: &[ConversationMessage],
        suggestions: &mut Vec<Suggestion>,
    ) {
        let recent_user_msgs: Vec<&ConversationMessage> = history
            .iter()
            .filter(|m| m.role == MessageRole::User)
            .rev()
            .take(3)
            .collect();

        for msg in &recent_user_msgs {
            let text = msg.text_content();
            if text.is_empty() {
                continue;
            }

            let tool_uses = msg.tool_uses();
            if !tool_uses.is_empty() {
                continue;
            }

            let lower = text.to_lowercase();
            let suggestion_text = if lower.contains("fix") || lower.contains("bug") {
                "Run tests to verify the fix".to_string()
            } else if lower.contains("add") || lower.contains("implement") {
                "Read the relevant source files first".to_string()
            } else if lower.contains("refactor") {
                "Search for all usages before refactoring".to_string()
            } else if lower.contains("test") {
                "Run the test suite".to_string()
            } else {
                continue;
            };

            suggestions.push(Suggestion {
                text: suggestion_text,
                confidence: 0.4,
                source: SuggestionSource::HistoryPattern,
                tool_name: None,
            });
        }
    }

    fn suggest_from_remote(
        &self,
        history: &[ConversationMessage],
        suggestions: &mut Vec<Suggestion>,
    ) {
        if !self.config.remote_enabled || !self.remote_context.is_remote {
            return;
        }

        let last_assistant = history
            .iter()
            .rev()
            .find(|m| m.role == MessageRole::Assistant);

        if let Some(msg) = last_assistant {
            let has_tool_use = msg
                .content
                .iter()
                .any(|b| matches!(b, ContentBlock::ToolUse { .. }));
            if has_tool_use {
                suggestions.push(Suggestion {
                    text: "Review and approve pending tool operations".to_string(),
                    confidence: 0.6,
                    source: SuggestionSource::Remote,
                    tool_name: None,
                });
            }
        }

        if let Some(ref session_id) = self.remote_context.session_id {
            if !session_id.is_empty() {
                suggestions.push(Suggestion {
                    text: "Sync remote session state".to_string(),
                    confidence: 0.3,
                    source: SuggestionSource::Remote,
                    tool_name: None,
                });
            }
        }
    }

    fn suggest_fallbacks(
        &self,
        history: &[ConversationMessage],
        suggestions: &mut Vec<Suggestion>,
    ) {
        let last_user = history.iter().rev().find(|m| m.role == MessageRole::User);

        if let Some(msg) = last_user {
            let text = msg.text_content();
            if !text.is_empty() {
                suggestions.push(Suggestion {
                    text: format!("Continue working on: {}", truncate(&text, 50)),
                    confidence: 0.2,
                    source: SuggestionSource::Fallback,
                    tool_name: None,
                });
            }
        }

        suggestions.push(Suggestion {
            text: "Show me what files changed".to_string(),
            confidence: 0.1,
            source: SuggestionSource::Fallback,
            tool_name: None,
        });
    }
}

fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        let boundary = s
            .char_indices()
            .take_while(|(i, _)| *i < max_len)
            .last()
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(max_len.min(s.len()));
        format!("{}...", &s[..boundary])
    }
}

pub struct VoiceStub;

impl VoiceStub {
    pub fn transcribe(_audio_data: &[u8]) -> anyhow::Result<String> {
        Ok(String::new())
    }

    pub fn synthesize(_text: &str) -> anyhow::Result<Vec<u8>> {
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::ConversationMessage;

    fn make_history_with_tools() -> Vec<ConversationMessage> {
        vec![
            ConversationMessage::user("fix the bug in main.rs"),
            ConversationMessage::assistant_tool_use(
                "t1",
                "ReadFile",
                serde_json::json!({"path": "src/main.rs"}),
            ),
            ConversationMessage::tool_result("t1", "fn main() {}"),
            ConversationMessage::assistant_tool_use(
                "t2",
                "Grep",
                serde_json::json!({"pattern": "TODO"}),
            ),
            ConversationMessage::tool_result("t2", "no matches"),
            ConversationMessage::assistant_tool_use(
                "t3",
                "ReadFile",
                serde_json::json!({"path": "src/lib.rs"}),
            ),
            ConversationMessage::tool_result("t3", "pub fn lib() {}"),
            ConversationMessage::user("now fix it"),
        ]
    }

    fn make_history_text_only() -> Vec<ConversationMessage> {
        vec![
            ConversationMessage::user("hello"),
            ConversationMessage::assistant_text("hi there"),
            ConversationMessage::user("fix the bug in parser"),
        ]
    }

    #[test]
    fn test_config_default() {
        let config = PromptSuggestionConfig::default();
        assert!(config.enabled);
        assert_eq!(config.max_suggestions, 5);
        assert_eq!(config.min_history, 2);
    }

    #[test]
    fn test_engine_disabled() {
        let config = PromptSuggestionConfig {
            enabled: false,
            ..Default::default()
        };
        let engine = PromptSuggestionEngine::new(config);
        let history = make_history_with_tools();
        let suggestions = engine.suggest(&history);
        assert!(suggestions.is_empty());
    }

    #[test]
    fn test_engine_insufficient_history() {
        let engine = PromptSuggestionEngine::new(PromptSuggestionConfig::default());
        let history = vec![ConversationMessage::user("hello")];
        let suggestions = engine.suggest(&history);
        assert!(suggestions.is_empty());
    }

    #[test]
    fn test_suggest_from_tool_patterns() {
        let engine = PromptSuggestionEngine::new(PromptSuggestionConfig::default());
        let history = make_history_with_tools();
        let suggestions = engine.suggest(&history);
        assert!(!suggestions.is_empty());

        let has_tool_pattern = suggestions
            .iter()
            .any(|s| s.source == SuggestionSource::ToolPattern);
        assert!(has_tool_pattern);
    }

    #[test]
    fn test_suggest_from_history() {
        let engine = PromptSuggestionEngine::new(PromptSuggestionConfig::default());
        let history = make_history_text_only();
        let suggestions = engine.suggest(&history);
        assert!(!suggestions.is_empty());

        let has_history = suggestions
            .iter()
            .any(|s| s.source == SuggestionSource::HistoryPattern);
        assert!(has_history);
    }

    #[test]
    fn test_suggest_respects_max() {
        let config = PromptSuggestionConfig {
            max_suggestions: 2,
            ..Default::default()
        };
        let engine = PromptSuggestionEngine::new(config);
        let history = make_history_with_tools();
        let suggestions = engine.suggest(&history);
        assert!(suggestions.len() <= 2);
    }

    #[test]
    fn test_suggestions_sorted_by_confidence() {
        let engine = PromptSuggestionEngine::new(PromptSuggestionConfig::default());
        let history = make_history_with_tools();
        let suggestions = engine.suggest(&history);
        for window in suggestions.windows(2) {
            assert!(window[0].confidence >= window[1].confidence);
        }
    }

    #[test]
    fn test_remote_suggestions() {
        let remote = RemoteContext {
            is_remote: true,
            upstream_url: Some("wss://example.com".to_string()),
            session_id: Some("sess-123".to_string()),
        };
        let engine =
            PromptSuggestionEngine::new(PromptSuggestionConfig::default()).with_remote(remote);
        let history = vec![
            ConversationMessage::user("hello"),
            ConversationMessage::assistant_tool_use(
                "t1",
                "Bash",
                serde_json::json!({"command": "ls"}),
            ),
        ];
        let suggestions = engine.suggest(&history);
        let has_remote = suggestions
            .iter()
            .any(|s| s.source == SuggestionSource::Remote);
        assert!(has_remote);
    }

    #[test]
    fn test_remote_disabled() {
        let config = PromptSuggestionConfig {
            remote_enabled: false,
            ..Default::default()
        };
        let remote = RemoteContext {
            is_remote: true,
            upstream_url: Some("wss://example.com".to_string()),
            session_id: Some("sess-123".to_string()),
        };
        let engine = PromptSuggestionEngine::new(config).with_remote(remote);
        let history = vec![
            ConversationMessage::user("hello"),
            ConversationMessage::assistant_tool_use(
                "t1",
                "Bash",
                serde_json::json!({"command": "ls"}),
            ),
        ];
        let suggestions = engine.suggest(&history);
        let has_remote = suggestions
            .iter()
            .any(|s| s.source == SuggestionSource::Remote);
        assert!(!has_remote);
    }

    #[test]
    fn test_fallback_suggestions() {
        let engine = PromptSuggestionEngine::new(PromptSuggestionConfig::default());
        let history = vec![
            ConversationMessage::user("something"),
            ConversationMessage::assistant_text("response"),
        ];
        let suggestions = engine.suggest(&history);
        let has_fallback = suggestions
            .iter()
            .any(|s| s.source == SuggestionSource::Fallback);
        assert!(has_fallback);
    }

    #[test]
    fn test_extract_memories() {
        let engine = PromptSuggestionEngine::new(PromptSuggestionConfig::default());
        let history = make_history_with_tools();
        let memories = engine.extract_memories(&history);
        assert!(memories.tool_sequence.contains(&"ReadFile".to_string()));
        assert!(memories.tool_sequence.contains(&"Grep".to_string()));
        assert!(memories.file_patterns.contains(&"src/main.rs".to_string()));
        assert!(memories.file_patterns.contains(&"src/lib.rs".to_string()));
    }

    #[test]
    fn test_analytics() {
        let analytics = SuggestionAnalytics::new();
        analytics.record_generated(&SuggestionSource::ToolPattern);
        analytics.record_generated(&SuggestionSource::ToolPattern);
        analytics.record_generated(&SuggestionSource::Remote);
        analytics.record_accepted();
        analytics.record_rejected();

        assert_eq!(analytics.total_generated.load(Ordering::Relaxed), 3);
        assert_eq!(analytics.total_accepted.load(Ordering::Relaxed), 1);
        assert_eq!(analytics.total_rejected.load(Ordering::Relaxed), 1);
        assert!((analytics.acceptance_rate() - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_voice_stub() {
        assert!(VoiceStub::transcribe(&[]).is_ok());
        assert!(VoiceStub::synthesize("hello").is_ok());
    }

    #[test]
    fn test_truncate() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello world this is long", 11), "hello world...");
    }

    #[test]
    fn test_suggestion_serialization() {
        let s = Suggestion {
            text: "test".to_string(),
            confidence: 0.8,
            source: SuggestionSource::ToolPattern,
            tool_name: Some("ReadFile".to_string()),
        };
        let json = serde_json::to_string(&s).unwrap();
        let deserialized: Suggestion = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.text, "test");
        assert_eq!(deserialized.source, SuggestionSource::ToolPattern);
    }
}
