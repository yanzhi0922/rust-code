//! Beta headers and extra body parameters for API requests.
//!
//! Manages the `anthropic-beta` header and extra body parameters that enable
//! experimental API features.  Includes special handling for Bedrock and
//! Vertex AI providers.
//!
//! Based on upstream Claude Code's `getExtraBodyParams`, `getMergedBetas`,
//! and `getBedrockExtraBodyParamsBetas` in `utils/betas.ts` and
//! `services/api/claude.ts`.

use reqwest::header::{HeaderName, HeaderValue};
use serde_json::{Value, json};
use std::collections::BTreeSet;

// ---------------------------------------------------------------------------
// Beta header constants
// ---------------------------------------------------------------------------

/// Prompt caching beta.
pub const PROMPT_CACHING_BETA: &str = "prompt-caching-2024-07-31";

/// Claude Code request-shaping beta.
pub const CLAUDE_CODE_BETA: &str = "claude-code-20250219";

/// PDF support beta.
pub const PDFS_BETA: &str = "pdfs-2024-09-25";

/// Extended thinking beta.
pub const INTERLEAVED_THINKING_BETA: &str = "interleaved-thinking-2025-05-14";

/// Native context-management beta.
pub const CONTEXT_MANAGEMENT_BETA: &str = "context-management-2025-06-27";

/// 1M context beta.
pub const CONTEXT_1M_BETA: &str = "context-1m-2025-08-07";

/// Structured outputs beta.
pub const STRUCTURED_OUTPUTS_BETA: &str = "structured-outputs-2025-12-15";

/// Effort beta.
pub const EFFORT_BETA: &str = "effort-2025-11-24";

/// API-side task budget beta.
pub const TASK_BUDGETS_BETA: &str = "task-budgets-2026-03-13";

/// Prompt cache global-scope beta.
pub const PROMPT_CACHING_SCOPE_BETA: &str = "prompt-caching-scope-2026-01-05";

/// Fast mode beta.
pub const FAST_MODE_BETA: &str = "fast-mode-2026-02-01";

/// Redacted thinking beta.
pub const REDACT_THINKING_BETA: &str = "redact-thinking-2026-02-12";

/// Token-efficient tool use beta.
pub const TOKEN_EFFICIENT_TOOLS_BETA: &str = "token-efficient-tools-2026-03-28";

/// Advisor tool beta.
pub const ADVISOR_BETA: &str = "advisor-tool-2026-03-01";

/// Tool search beta for first-party Anthropic-style providers.
pub const TOOL_SEARCH_BETA_1P: &str = "advanced-tool-use-2025-11-20";

/// Tool search beta for Bedrock / Vertex Anthropic endpoints.
pub const TOOL_SEARCH_BETA_3P: &str = "tool-search-tool-2025-10-19";

/// Web search beta.
pub const WEB_SEARCH_BETA: &str = "web-search-2025-03-05";

/// Summarize connector text beta.
pub const SUMMARIZE_CONNECTOR_TEXT_BETA: &str = "summarize-connector-text-2026-03-13";

/// Default beta headers for Anthropic first-party requests.
///
/// Only `claude-code-20250219` is always-on in the TS reference.  The old
/// `pdfs-2024-09-25` and `prompt-caching-2024-07-31` betas have been
/// graduated and are no longer sent by the official CLI.
pub const DEFAULT_BETA_HEADERS: &[&str] = &[CLAUDE_CODE_BETA];

// ---------------------------------------------------------------------------
// get_extra_body_params
// ---------------------------------------------------------------------------

/// Assemble extra body parameters for the API request.
///
/// Parses the `CLAUDE_CODE_EXTRA_BODY` environment variable (if present) as
/// a JSON object and merges it with any beta headers.
///
/// # Arguments
///
/// * `beta_headers` — Optional list of beta header strings to include.
///
/// # Returns
///
/// A JSON object representing the extra body parameters.
#[must_use]
pub fn get_extra_body_params(beta_headers: Option<&[String]>) -> Value {
    let mut result = json!({});

    // Parse user-supplied extra body parameters.
    if let Ok(extra_body_str) = std::env::var("CLAUDE_CODE_EXTRA_BODY")
        && !extra_body_str.is_empty()
        && let Ok(parsed) = serde_json::from_str::<Value>(&extra_body_str)
        && parsed.is_object()
    {
        // Shallow clone — we don't want to mutate the original.
        result = parsed;
    }

    // Merge beta headers into anthropic_beta array.
    if let Some(headers) = beta_headers
        && !headers.is_empty()
    {
        let existing: Vec<String> = result
            .get("anthropic_beta")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(Value::as_str)
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();

        let merged: Vec<String> = existing
            .into_iter()
            .chain(headers.iter().cloned())
            .collect();

        result["anthropic_beta"] = json!(merged);
    }

    result
}

// ---------------------------------------------------------------------------
// get_beta_headers
// ---------------------------------------------------------------------------

/// Build the list of beta header strings for the API request.
///
/// Includes the default betas and any model-specific betas.  For Bedrock and
/// Vertex AI providers, additional betas may be included.
///
/// # Arguments
///
/// * `model` — The model identifier.
/// * `is_bedrock` — Whether the request targets Amazon Bedrock.
/// * `is_vertex` — Whether the request targets Google Vertex AI.
/// * `enable_caching` — Whether prompt caching is enabled.
/// * `enable_thinking` — Whether extended thinking is enabled.
///
/// # Returns
///
/// A vector of beta header strings.
#[must_use]
pub fn get_beta_headers(
    model: &str,
    is_bedrock: bool,
    is_vertex: bool,
    enable_caching: bool,
    enable_thinking: bool,
) -> Vec<String> {
    let mut betas = BTreeSet::new();

    // Default betas.
    betas.insert(CLAUDE_CODE_BETA.to_owned());
    if enable_caching {
        betas.insert(PROMPT_CACHING_BETA.to_owned());
    }
    betas.insert(PDFS_BETA.to_owned());

    // Model-specific betas.
    let model_lower = model.to_ascii_lowercase();

    // Extended thinking for Claude models.
    if enable_thinking && model_lower.contains("claude") {
        betas.insert(INTERLEAVED_THINKING_BETA.to_owned());
        betas.insert(CONTEXT_MANAGEMENT_BETA.to_owned());
    }

    // Token-efficient tools for newer Claude models.
    if model_lower.contains("claude-sonnet-4")
        || model_lower.contains("claude-opus-4")
        || model_lower.contains("claude-3-7")
        || model_lower.contains("claude-3.7")
    {
        betas.insert(TOKEN_EFFICIENT_TOOLS_BETA.to_owned());
    }

    // Structured outputs for Claude models.
    if model_lower.contains("claude") {
        betas.insert(STRUCTURED_OUTPUTS_BETA.to_owned());
    }

    // Web search for Claude models.
    if model_lower.contains("claude") {
        betas.insert(WEB_SEARCH_BETA.to_owned());
    }

    // Bedrock-specific betas.
    if is_bedrock {
        // Bedrock may need additional betas for compatibility.
        if enable_caching {
            betas.insert(PROMPT_CACHING_BETA.to_owned());
        }
    }

    // Vertex AI-specific betas.
    if is_vertex {
        // Vertex AI may need additional betas for compatibility.
        if enable_caching {
            betas.insert(PROMPT_CACHING_BETA.to_owned());
        }
    }

    betas.into_iter().collect()
}

/// Merge explicit user opt-in Anthropic betas from `ANTHROPIC_BETAS`.
///
/// The reference CLI treats this as an additive escape hatch regardless of
/// model/provider. Keep ordering stable and avoid duplicates so snapshot tests
/// and proxies see deterministic headers.
pub fn merge_env_anthropic_betas(betas: &mut Vec<String>) {
    if let Ok(raw) = std::env::var("ANTHROPIC_BETAS") {
        for beta in raw
            .split(',')
            .map(str::trim)
            .filter(|beta| !beta.is_empty())
        {
            push_beta_once(betas, beta);
        }
    }
}

/// Insert a beta header once.
pub fn push_beta_once(betas: &mut Vec<String>, beta: &str) {
    if !betas.iter().any(|existing| existing == beta) {
        betas.push(beta.to_owned());
    }
}

/// Build the `anthropic-beta` header value from a list of beta strings.
///
/// # Errors
///
/// Returns an error if the header value contains invalid bytes.
pub fn build_beta_header_value(
    betas: &[String],
) -> Result<HeaderValue, reqwest::header::InvalidHeaderValue> {
    let value = betas.join(",");
    HeaderValue::from_str(&value)
}

/// Build the `anthropic-beta` header as a `(HeaderName, HeaderValue)` pair.
///
/// # Errors
///
/// Returns an error if the header value contains invalid bytes.
pub fn build_beta_header_pair(
    betas: &[String],
) -> Result<(HeaderName, HeaderValue), reqwest::header::InvalidHeaderValue> {
    let name = HeaderName::from_static("anthropic-beta");
    let value = build_beta_header_value(betas)?;
    Ok((name, value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_beta_headers_includes_defaults() {
        let betas = get_beta_headers("claude-sonnet-4", false, false, true, false);
        assert!(betas.contains(&PROMPT_CACHING_BETA.to_owned()));
        assert!(betas.contains(&PDFS_BETA.to_owned()));
    }

    #[test]
    fn get_beta_headers_includes_thinking_for_claude() {
        let betas = get_beta_headers("claude-sonnet-4", false, false, true, true);
        assert!(betas.contains(&INTERLEAVED_THINKING_BETA.to_owned()));
    }

    #[test]
    fn get_beta_headers_no_thinking_for_non_claude() {
        let betas = get_beta_headers("gpt-4o", false, false, true, true);
        assert!(!betas.contains(&INTERLEAVED_THINKING_BETA.to_owned()));
    }

    #[test]
    fn get_beta_headers_deduplicates() {
        let betas = get_beta_headers("claude-sonnet-4", true, false, true, false);
        let count = betas.iter().filter(|b| **b == PROMPT_CACHING_BETA).count();
        assert_eq!(count, 1);
    }

    #[test]
    fn get_extra_body_params_without_env_returns_betas() {
        // Test that without CLAUDE_CODE_EXTRA_BODY, only betas are merged.
        let betas = vec!["test-beta-1".to_owned(), "test-beta-2".to_owned()];
        let params = get_extra_body_params(Some(&betas));
        let anthropic_beta = params
            .get("anthropic_beta")
            .and_then(Value::as_array)
            .expect("should have anthropic_beta");
        assert_eq!(anthropic_beta.len(), 2);
    }

    #[test]
    fn get_extra_body_params_none_betas_returns_empty() {
        // When no betas and no env var, result should be empty or have no anthropic_beta.
        let params = get_extra_body_params(None);
        assert!(
            params
                .get("anthropic_beta")
                .and_then(Value::as_array)
                .is_none_or(|a| a.is_empty())
        );
    }

    #[test]
    fn build_beta_header_value_joins_with_comma() {
        let betas = vec!["beta-a".to_owned(), "beta-b".to_owned()];
        let value = build_beta_header_value(&betas).expect("should build");
        assert_eq!(value.to_str().expect("utf8"), "beta-a,beta-b");
    }
}
