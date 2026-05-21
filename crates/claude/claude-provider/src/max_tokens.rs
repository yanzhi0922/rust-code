//! Max output token logic for API requests.
//!
//! Provides [`get_max_output_tokens_for_model`] for looking up the maximum
//! output token limit per model and [`adjust_params_for_non_streaming`] for
//! capping tokens when falling back to non-streaming mode.
//!
//! Based on upstream Claude Code's `getMaxOutputTokensForModel` and
//! `adjustParamsForNonStreaming` in `services/api/claude.ts`.

use crate::model_info::get_model_info;
use serde_json::Value;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum tokens for non-streaming requests.
///
/// Non-streaming requests have a 10-minute max per the Anthropic docs.
/// We cap at 64 000 tokens to stay well within that boundary.
pub const MAX_NON_STREAMING_TOKENS: u32 = 64_000;

/// Default max output tokens when model info is unavailable.
const DEFAULT_MAX_OUTPUT_TOKENS: u32 = 8192;

/// Environment variable for overriding max output tokens.
const ENV_MAX_OUTPUT_TOKENS: &str = "CLAUDE_CODE_MAX_OUTPUT_TOKENS";

// ---------------------------------------------------------------------------
// get_max_output_tokens_for_model
// ---------------------------------------------------------------------------

/// Get the maximum output tokens for a model.
///
/// Looks up the model in the [`model_info`] database and returns the
/// configured maximum.  The value can be overridden via the
/// `CLAUDE_CODE_MAX_OUTPUT_TOKENS` environment variable.
#[must_use]
pub fn get_max_output_tokens_for_model(model: &str) -> u32 {
    let default_tokens = get_model_info(model).max_output as u32;

    // Allow environment variable override.
    if let Ok(env_val) = std::env::var(ENV_MAX_OUTPUT_TOKENS)
        && let Ok(parsed) = env_val.parse::<u32>()
        && parsed > 0
    {
        return parsed;
    }

    default_tokens
}

// ---------------------------------------------------------------------------
// adjust_params_for_non_streaming
// ---------------------------------------------------------------------------

/// Adjust API request parameters for non-streaming fallback.
///
/// When a streaming request fails and we fall back to non-streaming mode,
/// the `max_tokens` must be capped at [`MAX_NON_STREAMING_TOKENS`].
/// Additionally, if extended thinking is enabled, the thinking budget must
/// remain less than the capped `max_tokens`.
///
/// # Arguments
///
/// * `body` — The mutable API request body.
pub fn adjust_params_for_non_streaming(body: &mut Value) {
    let capped = cap_max_tokens(body);
    adjust_thinking_budget(body, capped);
}

/// Cap `max_tokens` in the request body to [`MAX_NON_STREAMING_TOKENS`].
///
/// Returns the effective capped value as `u64`.
fn cap_max_tokens(body: &mut Value) -> u64 {
    let current = body
        .get("max_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(u64::from(DEFAULT_MAX_OUTPUT_TOKENS));

    let capped = current.min(u64::from(MAX_NON_STREAMING_TOKENS));
    body["max_tokens"] = serde_json::json!(capped);
    capped
}

/// Ensure thinking budget is less than the capped `max_tokens`.
///
/// The Anthropic API requires `max_tokens > thinking.budget_tokens`.
fn adjust_thinking_budget(body: &mut Value, capped_max_tokens: u64) {
    let thinking = match body.get_mut("thinking") {
        Some(t) => t,
        None => return,
    };

    // Only adjust if thinking is enabled with a budget.
    if thinking.get("type").and_then(Value::as_str) != Some("enabled") {
        return;
    }

    if let Some(budget_val) = thinking.get_mut("budget_tokens") {
        let budget = budget_val.as_u64().unwrap_or(0);
        if budget >= capped_max_tokens {
            // Must be at least 1 less than max_tokens.
            let adjusted = capped_max_tokens.saturating_sub(1);
            // Only update budget_tokens, preserving other fields in the thinking object.
            if let Some(obj) = thinking.as_object_mut() {
                obj.insert("budget_tokens".to_owned(), serde_json::json!(adjusted));
            } else {
                *thinking = serde_json::json!({
                    "type": "enabled",
                    "budget_tokens": adjusted,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn get_max_tokens_for_known_model() {
        let tokens = get_max_output_tokens_for_model("claude-sonnet-4-20250514");
        assert!(tokens > 0);
    }

    #[test]
    fn get_max_tokens_for_unknown_model_returns_model_info_default() {
        // get_model_info returns a fallback ModelInfo with max_output = 4096
        // for unknown models.
        let tokens = get_max_output_tokens_for_model("completely-unknown-model-xyz");
        assert!(tokens > 0, "should return a positive token count");
    }

    #[test]
    fn adjust_params_caps_max_tokens() {
        let mut body = json!({
            "max_tokens": 128_000,
            "model": "test",
        });
        adjust_params_for_non_streaming(&mut body);
        assert_eq!(
            body.get("max_tokens").and_then(Value::as_u64),
            Some(u64::from(MAX_NON_STREAMING_TOKENS))
        );
    }

    #[test]
    fn adjust_params_does_not_increase_max_tokens() {
        let mut body = json!({
            "max_tokens": 1024,
            "model": "test",
        });
        adjust_params_for_non_streaming(&mut body);
        assert_eq!(body.get("max_tokens").and_then(Value::as_u64), Some(1024));
    }

    #[test]
    fn adjust_params_caps_thinking_budget() {
        let mut body = json!({
            "max_tokens": 128_000,
            "thinking": {
                "type": "enabled",
                "budget_tokens": 100_000,
            },
        });
        adjust_params_for_non_streaming(&mut body);
        let budget = body
            .get("thinking")
            .and_then(|t| t.get("budget_tokens"))
            .and_then(Value::as_u64)
            .expect("budget_tokens should exist");
        // Budget should be capped to max_tokens - 1.
        let max_tokens = body
            .get("max_tokens")
            .and_then(Value::as_u64)
            .expect("max_tokens should exist");
        assert!(budget < max_tokens);
    }

    #[test]
    fn adjust_params_noop_when_no_thinking() {
        let mut body = json!({
            "max_tokens": 4096,
        });
        adjust_params_for_non_streaming(&mut body);
        assert_eq!(body.get("max_tokens").and_then(Value::as_u64), Some(4096));
        assert!(body.get("thinking").is_none());
    }
}
