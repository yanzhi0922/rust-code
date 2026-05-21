//! Token estimation utilities for conversation context management.
//!
//! Provides fast, local token-count approximations without requiring a
//! tokenizer.  Uses character-based heuristics (~4 chars/token for English,
//! ~2 chars/token for CJK text).

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for token estimation heuristics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenEstimationConfig {
    /// Characters per token for Latin/ASCII text.
    pub chars_per_token_latin: f64,
    /// Characters per token for CJK text.
    pub chars_per_token_cjk: f64,
    /// Per-message overhead tokens (role markers, separators, etc.).
    pub message_overhead_tokens: u64,
    /// System-prompt overhead tokens.
    pub system_prompt_overhead_tokens: u64,
}

impl Default for TokenEstimationConfig {
    fn default() -> Self {
        Self {
            chars_per_token_latin: 4.0,
            chars_per_token_cjk: 2.0,
            message_overhead_tokens: 4,
            system_prompt_overhead_tokens: 10,
        }
    }
}

impl TokenEstimationConfig {
    /// Create a new config with custom char-per-token ratios.
    #[must_use]
    pub fn new(chars_per_token_latin: f64, chars_per_token_cjk: f64) -> Self {
        Self {
            chars_per_token_latin,
            chars_per_token_cjk,
            ..Self::default()
        }
    }
}

// ---------------------------------------------------------------------------
// Core estimation
// ---------------------------------------------------------------------------

/// A simplified message representation for token estimation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EstimatedMessage {
    /// Role of the message author (e.g. "user", "assistant", "system").
    pub role: String,
    /// Text content of the message.
    pub content: String,
}

impl EstimatedMessage {
    /// Create a new estimated message.
    #[must_use]
    pub fn new(role: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            content: content.into(),
        }
    }
}

/// A simplified conversation for token estimation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EstimatedConversation {
    /// Optional system prompt.
    pub system_prompt: Option<String>,
    /// Messages in the conversation.
    pub messages: Vec<EstimatedMessage>,
}

impl EstimatedConversation {
    /// Create a new conversation without a system prompt.
    #[must_use]
    pub fn new(messages: Vec<EstimatedMessage>) -> Self {
        Self {
            system_prompt: None,
            messages,
        }
    }

    /// Create a conversation with a system prompt.
    #[must_use]
    pub fn with_system_prompt(
        system_prompt: impl Into<String>,
        messages: Vec<EstimatedMessage>,
    ) -> Self {
        Self {
            system_prompt: Some(system_prompt.into()),
            messages,
        }
    }
}

// ---------------------------------------------------------------------------
// Estimation functions
// ---------------------------------------------------------------------------

/// Estimate the number of tokens in a text string.
///
/// Uses a simple heuristic:
/// - CJK characters count as ~2 chars/token
/// - Latin/other characters count as ~4 chars/token
///
/// # Arguments
///
/// * `text` — The text to estimate tokens for.
/// * `config` — Estimation configuration (uses default if `None`).
///
/// # Returns
///
/// Estimated token count (rounded up).
#[must_use]
pub fn estimate_tokens(text: &str, config: Option<&TokenEstimationConfig>) -> u64 {
    let default_cfg = TokenEstimationConfig::default();
    let cfg = config.unwrap_or(&default_cfg);

    let mut cjk_chars: u64 = 0;
    let mut latin_chars: u64 = 0;

    for ch in text.chars() {
        if is_cjk(ch) {
            cjk_chars += 1;
        } else {
            latin_chars += 1;
        }
    }

    let cjk_tokens = if cfg.chars_per_token_cjk > 0.0 {
        (f64::from(cjk_chars as u32) / cfg.chars_per_token_cjk).ceil() as u64
    } else {
        0
    };

    let latin_tokens = if cfg.chars_per_token_latin > 0.0 {
        (f64::from(latin_chars as u32) / cfg.chars_per_token_latin).ceil() as u64
    } else {
        0
    };

    cjk_tokens + latin_tokens
}

/// Estimate the number of tokens consumed by a single message.
///
/// Includes the content tokens plus per-message overhead.
#[must_use]
pub fn estimate_message_tokens(
    message: &EstimatedMessage,
    config: Option<&TokenEstimationConfig>,
) -> u64 {
    let default_cfg = TokenEstimationConfig::default();
    let cfg = config.unwrap_or(&default_cfg);
    let content_tokens = estimate_tokens(&message.content, Some(cfg));
    let role_tokens = estimate_tokens(&message.role, Some(cfg));
    content_tokens + role_tokens + cfg.message_overhead_tokens
}

/// Estimate the total number of tokens for an entire conversation.
///
/// Includes system prompt tokens (if present), all message tokens,
/// and system-prompt overhead.
#[must_use]
pub fn estimate_conversation_tokens(
    conversation: &EstimatedConversation,
    config: Option<&TokenEstimationConfig>,
) -> u64 {
    let default_cfg = TokenEstimationConfig::default();
    let cfg = config.unwrap_or(&default_cfg);

    let mut total: u64 = 0;

    // System prompt
    if let Some(ref prompt) = conversation.system_prompt {
        total += estimate_tokens(prompt, Some(cfg));
        total += cfg.system_prompt_overhead_tokens;
    }

    // Messages
    for msg in &conversation.messages {
        total += estimate_message_tokens(msg, Some(cfg));
    }

    total
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Check if a character is a CJK ideograph.
fn is_cjk(ch: char) -> bool {
    let cp = ch as u32;
    // CJK Unified Ideographs: U+4E00..U+9FFF
    // CJK Unified Ideographs Extension A: U+3400..U+4DBF
    // CJK Unified Ideographs Extension B..H: U+20000..U+323AF
    // Hiragana: U+3040..U+309F
    // Katakana: U+30A0..U+30FF
    // Hangul Syllables: U+AC00..U+D7AF
    matches!(
        cp,
        0x3040..=0x309F
        | 0x30A0..=0x30FF
        | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF
        | 0xAC00..=0xD7AF
        | 0x20000..=0x323AF
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- is_cjk ----------------------------------------------------------------

    #[test]
    fn is_cjk_chinese() {
        assert!(is_cjk('中'));
        assert!(is_cjk('文'));
    }

    #[test]
    fn is_cjk_japanese_hiragana() {
        assert!(is_cjk('あ'));
    }

    #[test]
    fn is_cjk_japanese_katakana() {
        assert!(is_cjk('ア'));
    }

    #[test]
    fn is_cjk_korean() {
        assert!(is_cjk('한'));
    }

    #[test]
    fn is_cjk_latin() {
        assert!(!is_cjk('A'));
        assert!(!is_cjk('z'));
    }

    #[test]
    fn is_cjk_digit() {
        assert!(!is_cjk('0'));
    }

    #[test]
    fn is_cjk_space() {
        assert!(!is_cjk(' '));
    }

    // -- estimate_tokens -------------------------------------------------------

    #[test]
    fn estimate_tokens_empty() {
        assert_eq!(estimate_tokens("", None), 0);
    }

    #[test]
    fn estimate_tokens_english() {
        // "Hello world" = 11 chars, ~4 chars/token => ceil(11/4) = 3
        let tokens = estimate_tokens("Hello world", None);
        assert_eq!(tokens, 3);
    }

    #[test]
    fn estimate_tokens_exact_multiple() {
        // 8 chars / 4 = 2 tokens
        assert_eq!(estimate_tokens("abcdabcd", None), 2);
    }

    #[test]
    fn estimate_tokens_cjk() {
        // "你好世界" = 4 CJK chars, ~2 chars/token => 2 tokens
        let tokens = estimate_tokens("你好世界", None);
        assert_eq!(tokens, 2);
    }

    #[test]
    fn estimate_tokens_mixed() {
        // "Hello你好" = 5 latin + 2 CJK
        // latin: ceil(5/4) = 2, cjk: ceil(2/2) = 1 => total 3
        let tokens = estimate_tokens("Hello你好", None);
        assert_eq!(tokens, 3);
    }

    #[test]
    fn estimate_tokens_custom_config() {
        let cfg = TokenEstimationConfig::new(2.0, 1.0);
        // "abcd" = 4 latin chars / 2.0 = 2 tokens
        assert_eq!(estimate_tokens("abcd", Some(&cfg)), 2);
    }

    #[test]
    fn estimate_tokens_single_char() {
        // 1 char / 4 = 0.25 => ceil => 1
        assert_eq!(estimate_tokens("a", None), 1);
    }

    #[test]
    fn estimate_tokens_whitespace() {
        // "   " = 3 chars / 4 = 0.75 => ceil => 1
        assert_eq!(estimate_tokens("   ", None), 1);
    }

    #[test]
    fn estimate_tokens_long_text() {
        let text = "a".repeat(100);
        // 100 / 4 = 25
        assert_eq!(estimate_tokens(&text, None), 25);
    }

    // -- estimate_message_tokens -----------------------------------------------

    #[test]
    fn estimate_message_tokens_basic() {
        let msg = EstimatedMessage::new("user", "Hello");
        // role "user" = 1 token, content "Hello" = 2 tokens, overhead = 4
        let tokens = estimate_message_tokens(&msg, None);
        assert!(tokens >= 7); // at least role + content + overhead
    }

    #[test]
    fn estimate_message_tokens_empty_content() {
        let msg = EstimatedMessage::new("user", "");
        let tokens = estimate_message_tokens(&msg, None);
        // role "user" = 1 token + overhead 4 = 5
        assert!(tokens >= 5);
    }

    #[test]
    fn estimate_message_tokens_cjk_content() {
        let msg = EstimatedMessage::new("assistant", "你好世界你好世界");
        let tokens = estimate_message_tokens(&msg, None);
        assert!(tokens > 0);
    }

    // -- estimate_conversation_tokens ------------------------------------------

    #[test]
    fn estimate_conversation_empty() {
        let conv = EstimatedConversation::new(vec![]);
        assert_eq!(estimate_conversation_tokens(&conv, None), 0);
    }

    #[test]
    fn estimate_conversation_with_system_prompt() {
        let conv = EstimatedConversation::with_system_prompt(
            "You are helpful.",
            vec![EstimatedMessage::new("user", "Hi")],
        );
        let tokens = estimate_conversation_tokens(&conv, None);
        // system prompt + overhead + user message
        assert!(tokens > 10);
    }

    #[test]
    fn estimate_conversation_multiple_messages() {
        let conv = EstimatedConversation::new(vec![
            EstimatedMessage::new("user", "Hello"),
            EstimatedMessage::new("assistant", "Hi there!"),
            EstimatedMessage::new("user", "How are you?"),
        ]);
        let tokens = estimate_conversation_tokens(&conv, None);
        assert!(tokens > 0);
    }

    #[test]
    fn estimate_conversation_grows_with_messages() {
        let conv_small = EstimatedConversation::new(vec![EstimatedMessage::new("user", "Hi")]);
        let conv_large = EstimatedConversation::new(vec![
            EstimatedMessage::new("user", "Hi"),
            EstimatedMessage::new("assistant", "Hello! How can I help you today?"),
            EstimatedMessage::new("user", "Tell me a long story please."),
        ]);
        let small = estimate_conversation_tokens(&conv_small, None);
        let large = estimate_conversation_tokens(&conv_large, None);
        assert!(large > small);
    }

    // -- TokenEstimationConfig -------------------------------------------------

    #[test]
    fn config_default() {
        let cfg = TokenEstimationConfig::default();
        assert!((cfg.chars_per_token_latin - 4.0).abs() < f64::EPSILON);
        assert!((cfg.chars_per_token_cjk - 2.0).abs() < f64::EPSILON);
        assert_eq!(cfg.message_overhead_tokens, 4);
        assert_eq!(cfg.system_prompt_overhead_tokens, 10);
    }

    #[test]
    fn config_custom() {
        let cfg = TokenEstimationConfig::new(3.0, 1.5);
        assert!((cfg.chars_per_token_latin - 3.0).abs() < f64::EPSILON);
        assert!((cfg.chars_per_token_cjk - 1.5).abs() < f64::EPSILON);
    }

    #[test]
    fn config_serialization() {
        let cfg = TokenEstimationConfig::default();
        let json = serde_json::to_string(&cfg).expect("serialize");
        let back: TokenEstimationConfig = serde_json::from_str(&json).expect("deserialize");
        assert!((back.chars_per_token_latin - cfg.chars_per_token_latin).abs() < f64::EPSILON);
    }

    // -- EstimatedMessage ------------------------------------------------------

    #[test]
    fn estimated_message_new() {
        let msg = EstimatedMessage::new("user", "test");
        assert_eq!(msg.role, "user");
        assert_eq!(msg.content, "test");
    }

    #[test]
    fn estimated_message_serialization() {
        let msg = EstimatedMessage::new("assistant", "response");
        let json = serde_json::to_string(&msg).expect("serialize");
        let back: EstimatedMessage = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(msg.role, back.role);
        assert_eq!(msg.content, back.content);
    }

    // -- EstimatedConversation -------------------------------------------------

    #[test]
    fn conversation_new() {
        let conv = EstimatedConversation::new(vec![EstimatedMessage::new("user", "hi")]);
        assert!(conv.system_prompt.is_none());
        assert_eq!(conv.messages.len(), 1);
    }

    #[test]
    fn conversation_with_system_prompt() {
        let conv = EstimatedConversation::with_system_prompt(
            "Be helpful",
            vec![EstimatedMessage::new("user", "hi")],
        );
        assert_eq!(conv.system_prompt.as_deref(), Some("Be helpful"));
    }

    #[test]
    fn conversation_serialization() {
        let conv = EstimatedConversation::with_system_prompt(
            "sys",
            vec![EstimatedMessage::new("user", "hello")],
        );
        let json = serde_json::to_string(&conv).expect("serialize");
        let back: EstimatedConversation = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(conv.system_prompt, back.system_prompt);
        assert_eq!(conv.messages.len(), back.messages.len());
    }
}
