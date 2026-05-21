//! Effort level parameter configuration for API requests.
//!
//! Maps the user-facing effort level (low / medium / high / max) to the
//! Anthropic `output_config.effort` field and the corresponding
//! `anthropic-beta` header.
//!
//! Based on upstream Claude Code's `configureEffortParams` and
//! `modelSupportsEffort` in `utils/effort.ts`.

use serde_json::{Value, json};

// Re-export EffortLevel from the canonical definition in claude-context.
pub use claude_context::effort::EffortLevel;

// ---------------------------------------------------------------------------
// Beta header constant
// ---------------------------------------------------------------------------

/// The beta header that enables the effort parameter.
pub const EFFORT_BETA_HEADER: &str = crate::beta_headers::EFFORT_BETA;

// ---------------------------------------------------------------------------
// Model support check
// ---------------------------------------------------------------------------

/// Models known to support the effort parameter.
const EFFORT_SUPPORTED_PREFIXES: &[&str] = &[
    "claude-sonnet-4-6",
    "claude-opus-4-6",
    "glm-5",
    "glm-4-plus",
    "minimax-m2",
];

/// Check whether a model supports the effort parameter.
///
/// Uses prefix matching so that versioned model names (e.g.
/// `claude-sonnet-4-20250514`) are correctly identified.
#[must_use]
pub fn model_supports_effort(model: &str) -> bool {
    let model_lower = model.to_ascii_lowercase();
    if std::env::var("CLAUDE_CODE_ALWAYS_ENABLE_EFFORT")
        .ok()
        .as_deref()
        .is_some_and(|value| matches!(value, "1" | "true" | "yes" | "on"))
    {
        return true;
    }
    // Strip provider prefix (e.g. "anthropic/" or "openai/") before matching.
    let model_name = model_lower
        .rsplit_once('/')
        .map_or(model_lower.as_str(), |(_, name)| name);
    EFFORT_SUPPORTED_PREFIXES
        .iter()
        .any(|prefix| model_name.starts_with(prefix))
}

// ---------------------------------------------------------------------------
// Configure effort params
// ---------------------------------------------------------------------------

/// Configure effort parameters on the API request body and beta headers.
///
/// If the model does not support effort, this is a no-op.
///
/// # Arguments
///
/// * `body` — The mutable API request body (`serde_json::Value`).
/// * `betas` — The mutable list of beta headers to extend.
/// * `model` — The model identifier.
/// * `effort_level` — Optional effort level string ("low", "medium", "high").
pub fn configure_effort_params(
    body: &mut Value,
    betas: &mut Vec<String>,
    model: &str,
    effort_level: Option<&str>,
) {
    if !model_supports_effort(model) {
        return;
    }

    // If effort is already set in output_config, don't override.
    if body
        .get("output_config")
        .and_then(|oc| oc.get("effort"))
        .is_some()
    {
        return;
    }

    match effort_level {
        Some(level) => {
            let level_lower = level.to_ascii_lowercase();
            let normalized = level_lower.as_str();
            if !matches!(normalized, "low" | "medium" | "high" | "max") {
                return;
            }

            // Set output_config.effort on the body.
            if body.get("output_config").is_none() {
                body["output_config"] = json!({});
            }
            if let Some(oc) = body.get_mut("output_config") {
                oc["effort"] = json!(normalized);
            }

            // Ensure the effort beta header is included.
            if !betas.contains(&EFFORT_BETA_HEADER.to_owned()) {
                betas.push(EFFORT_BETA_HEADER.to_owned());
            }
        }
        None => {
            // No explicit effort level — just enable the beta so the API
            // uses its default effort.
            if !betas.contains(&EFFORT_BETA_HEADER.to_owned()) {
                betas.push(EFFORT_BETA_HEADER.to_owned());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_supports_effort_for_known_models() {
        assert!(model_supports_effort("claude-sonnet-4-6-20260401"));
        assert!(model_supports_effort("claude-opus-4-6-20260401"));
        assert!(model_supports_effort("anthropic/claude-sonnet-4-6"));
        assert!(model_supports_effort("glm-5"));
        assert!(model_supports_effort("minimax-m2.7"));
    }

    #[test]
    fn model_does_not_support_effort_for_unknown() {
        assert!(!model_supports_effort("gpt-4o"));
        assert!(!model_supports_effort("claude-3-haiku"));
        assert!(!model_supports_effort("claude-sonnet-4-20250514"));
        assert!(!model_supports_effort("unknown-model"));
    }

    #[test]
    fn configure_effort_params_sets_output_config() {
        let mut body = json!({});
        let mut betas = Vec::new();
        configure_effort_params(&mut body, &mut betas, "claude-sonnet-4-6", Some("high"));
        assert_eq!(body["output_config"]["effort"], "high");
        assert!(betas.contains(&EFFORT_BETA_HEADER.to_owned()));
    }

    #[test]
    fn configure_effort_params_noop_for_unsupported_model() {
        let mut body = json!({});
        let mut betas = Vec::new();
        configure_effort_params(&mut body, &mut betas, "gpt-4o", Some("high"));
        assert!(body.get("output_config").is_none());
        assert!(betas.is_empty());
    }

    #[test]
    fn configure_effort_params_does_not_override_existing() {
        let mut body = json!({"output_config": {"effort": "low"}});
        let mut betas = Vec::new();
        configure_effort_params(&mut body, &mut betas, "claude-sonnet-4-6", Some("high"));
        // Should not override the existing effort.
        assert_eq!(body["output_config"]["effort"], "low");
    }

    #[test]
    fn configure_effort_params_default_enables_beta() {
        let mut body = json!({});
        let mut betas = Vec::new();
        configure_effort_params(&mut body, &mut betas, "claude-sonnet-4-6", None);
        assert!(betas.contains(&EFFORT_BETA_HEADER.to_owned()));
        // output_config.effort should NOT be set when no explicit level.
        assert!(body.get("output_config").is_none());
    }
}
