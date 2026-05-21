//! Context window management.
//!
//! Provides functions for determining context window sizes, 1M context support,
//! usage percentage calculation, and max output token limits per model.
//!
//! Ported from `claude-code-rev/src/utils/context.ts`.

use serde::{Deserialize, Serialize};

// ── Constants ───────────────────────────────────────────────────────────

/// Default context window size for all models (200k tokens).
pub const MODEL_CONTEXT_WINDOW_DEFAULT: u32 = 200_000;

/// Maximum output tokens for compact operations.
pub const COMPACT_MAX_OUTPUT_TOKENS: u32 = 20_000;

/// Default max output tokens.
const MAX_OUTPUT_TOKENS_DEFAULT: u32 = 32_000;

/// Upper limit for max output tokens.
const MAX_OUTPUT_TOKENS_UPPER_LIMIT: u32 = 64_000;

/// Capped default for slot-reservation optimization.
pub const CAPPED_DEFAULT_MAX_TOKENS: u32 = 8_000;

/// Escalated max tokens after a capped request hits the limit.
pub const ESCALATED_MAX_TOKENS: u32 = 64_000;

/// 1M context window size.
const CONTEXT_WINDOW_1M: u32 = 1_000_000;

// ── Types ───────────────────────────────────────────────────────────────

/// Context window information for a model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextWindow {
    /// Total context window size in tokens.
    pub total_tokens: u32,
    /// Maximum output tokens for the model.
    pub max_output_tokens: u32,
    /// Whether the model supports 1M context.
    pub supports_1m: bool,
}

/// Context usage percentages.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextPercentages {
    /// Percentage of context window used (0-100).
    pub used_percent: f64,
    /// Percentage of context window remaining (0-100).
    pub remaining_percent: f64,
}

/// Max output token configuration for a model.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct MaxOutputTokens {
    /// Default max output tokens.
    pub default: u32,
    /// Upper limit for max output tokens.
    pub upper_limit: u32,
}

/// Token usage breakdown for context percentage calculation.
#[derive(Debug, Clone, Default)]
pub struct TokenUsage {
    /// Input tokens.
    pub input_tokens: u64,
    /// Cache creation input tokens.
    pub cache_creation_input_tokens: u64,
    /// Cache read input tokens.
    pub cache_read_input_tokens: u64,
}

/// Configuration for context window resolution.
///
/// This struct replaces the implicit dependency on environment variables
/// and global config in the original TypeScript implementation.
#[derive(Debug, Clone, Default)]
pub struct ContextConfig {
    /// Whether 1M context is explicitly disabled.
    pub disable_1m_context: bool,
    /// Optional override for context window size (takes precedence over all).
    pub max_context_tokens_override: Option<u32>,
    /// Whether the user is an internal "ant" user.
    pub is_ant_user: bool,
}

// ── Functions ───────────────────────────────────────────────────────────

/// Check if the model name has the `[1m]` tag indicating 1M context.
///
/// Equivalent to `has1mContext()` in context.ts.
pub fn has_1m_context(model: &str) -> bool {
    model.to_ascii_lowercase().contains("[1m]")
}

/// Check if a model supports 1M context window.
///
/// Equivalent to `modelSupports1M()` in context.ts.
/// Currently supported: claude-sonnet-4, opus-4-6.
pub fn model_supports_1m(model: &str, config: &ContextConfig) -> bool {
    if config.disable_1m_context {
        return false;
    }
    let canonical = model.to_ascii_lowercase();
    canonical.contains("claude-sonnet-4") || canonical.contains("opus-4-6")
}

/// Get the context window size for a model.
///
/// Equivalent to `getContextWindowForModel()` in context.ts.
///
/// Resolution order:
/// 1. Explicit override via `config.max_context_tokens_override` (ant users only)
/// 2. `[1m]` suffix in model name
/// 3. Model capability lookup
/// 4. Default (200k)
pub fn get_context_window_for_model(model: &str, config: &ContextConfig) -> u32 {
    // 1. Ant-user override
    if config.is_ant_user
        && let Some(override_val) = config.max_context_tokens_override
        && override_val > 0
    {
        return override_val;
    }

    // 2. [1m] suffix — explicit client-side opt-in
    if has_1m_context(model) && !config.disable_1m_context {
        return CONTEXT_WINDOW_1M;
    }

    // 3. Model-specific context windows
    let canonical = model.to_ascii_lowercase();

    // Check for models with known larger context windows
    if canonical.contains("opus-4-6") || canonical.contains("sonnet-4-6") {
        // These models support 1M but default to 200k unless explicitly opted in
        return MODEL_CONTEXT_WINDOW_DEFAULT;
    }

    // 4. Default
    MODEL_CONTEXT_WINDOW_DEFAULT
}

/// Calculate context window usage percentages.
///
/// Equivalent to `calculateContextPercentages()` in context.ts.
///
/// Returns `None` if `usage` is `None`.
pub fn calculate_context_percentages(
    usage: Option<&TokenUsage>,
    context_window_size: u32,
) -> Option<ContextPercentages> {
    let usage = usage?;

    let total_input_tokens =
        usage.input_tokens + usage.cache_creation_input_tokens + usage.cache_read_input_tokens;

    let context_window_size = context_window_size as f64;
    let used_percentage = if context_window_size > 0.0 {
        (total_input_tokens as f64 / context_window_size) * 100.0
    } else {
        0.0
    };

    // Clamp to 0-100
    let clamped_used = used_percentage.clamp(0.0, 100.0);
    let clamped_used = clamped_used.round().min(100.0);

    Some(ContextPercentages {
        used_percent: clamped_used,
        remaining_percent: 100.0 - clamped_used,
    })
}

/// Get the max output tokens configuration for a model.
///
/// Equivalent to `getModelMaxOutputTokens()` in context.ts.
pub fn get_model_max_output_tokens(model: &str) -> MaxOutputTokens {
    let m = model.to_ascii_lowercase();

    let (default, upper_limit) = if m.contains("opus-4-6") {
        (64_000, 128_000)
    } else if m.contains("sonnet-4-6") {
        (32_000, 128_000)
    } else if m.contains("opus-4-5") || m.contains("sonnet-4-") || m.contains("haiku-4") {
        (32_000, 64_000)
    } else if m.contains("opus-4-1") || m.contains("opus-4-2") {
        (32_000, 32_000)
    } else if m.contains("claude-3-opus") {
        (4_096, 4_096)
    } else if m.contains("claude-3-sonnet") {
        (8_192, 8_192)
    } else if m.contains("claude-3-haiku") {
        (4_096, 4_096)
    } else if m.contains("3-5-sonnet") || m.contains("3-5-haiku") {
        (8_192, 8_192)
    } else if m.contains("3-7-sonnet") {
        (32_000, 64_000)
    } else if m.contains("gpt-4.1") {
        (32_768, 32_768)
    } else if m.contains("gpt-4o") {
        (16_384, 16_384)
    } else {
        (MAX_OUTPUT_TOKENS_DEFAULT, MAX_OUTPUT_TOKENS_UPPER_LIMIT)
    };

    MaxOutputTokens {
        default,
        upper_limit,
    }
}

/// Get the max thinking tokens for a model.
///
/// Equivalent to `getMaxThinkingTokensForModel()` in context.ts.
/// Returns `upper_limit - 1` since thinking tokens must be strictly less
/// than max output tokens.
pub fn get_max_thinking_tokens_for_model(model: &str) -> u32 {
    get_model_max_output_tokens(model)
        .upper_limit
        .saturating_sub(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_has_1m_context() {
        assert!(has_1m_context("opus[1m]"));
        assert!(has_1m_context("claude[1M]"));
        assert!(!has_1m_context("opus"));
        assert!(!has_1m_context("sonnet-4-6"));
    }

    #[test]
    fn test_model_supports_1m() {
        let config = ContextConfig::default();
        assert!(model_supports_1m("claude-sonnet-4-6", &config));
        assert!(model_supports_1m("claude-opus-4-6", &config));
        assert!(!model_supports_1m("claude-haiku-4-5", &config));
    }

    #[test]
    fn test_model_supports_1m_disabled() {
        let config = ContextConfig {
            disable_1m_context: true,
            ..Default::default()
        };
        assert!(!model_supports_1m("claude-sonnet-4-6", &config));
    }

    #[test]
    fn test_context_window_default() {
        let config = ContextConfig::default();
        assert_eq!(
            get_context_window_for_model("claude-haiku-4-5", &config),
            200_000
        );
    }

    #[test]
    fn test_context_window_1m_suffix() {
        let config = ContextConfig::default();
        assert_eq!(get_context_window_for_model("opus[1m]", &config), 1_000_000);
    }

    #[test]
    fn test_context_window_ant_override() {
        let config = ContextConfig {
            is_ant_user: true,
            max_context_tokens_override: Some(50_000),
            ..Default::default()
        };
        assert_eq!(get_context_window_for_model("opus[1m]", &config), 50_000);
    }

    #[test]
    fn test_calculate_context_percentages_none() {
        assert!(calculate_context_percentages(None, 200_000).is_none());
    }

    #[test]
    fn test_calculate_context_percentages() {
        let usage = TokenUsage {
            input_tokens: 100_000,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
        };
        let result = calculate_context_percentages(Some(&usage), 200_000)
            .expect("should return percentages");
        assert!((result.used_percent - 50.0).abs() < 0.01);
        assert!((result.remaining_percent - 50.0).abs() < 0.01);
    }

    #[test]
    fn test_get_model_max_output_tokens_opus46() {
        let tokens = get_model_max_output_tokens("claude-opus-4-6");
        assert_eq!(tokens.default, 64_000);
        assert_eq!(tokens.upper_limit, 128_000);
    }

    #[test]
    fn test_get_model_max_output_tokens_sonnet46() {
        let tokens = get_model_max_output_tokens("claude-sonnet-4-6");
        assert_eq!(tokens.default, 32_000);
        assert_eq!(tokens.upper_limit, 128_000);
    }

    #[test]
    fn test_get_model_max_output_tokens_unknown() {
        let tokens = get_model_max_output_tokens("some-unknown-model");
        assert_eq!(tokens.default, 32_000);
        assert_eq!(tokens.upper_limit, 64_000);
    }
}
