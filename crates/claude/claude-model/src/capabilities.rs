//! Model capability queries.
//!
//! Each known model has an associated set of capabilities (image support,
//! extended thinking, 1M context, etc.) that can be queried at runtime.

use std::sync::LazyLock;

use serde::{Deserialize, Serialize};

// Re-export EffortLevel from the canonical definition in claude-context.
pub use claude_context::effort::EffortLevel;

/// Capability set for a specific model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelCapabilities {
    /// Whether the model can accept image inputs.
    pub supports_images: bool,
    /// Whether the model supports tool/function calling.
    pub supports_tool_use: bool,
    /// Whether the model supports extended thinking (chain-of-thought).
    pub supports_extended_thinking: bool,
    /// Whether the model supports a 1M-token context window.
    pub supports_1m_context: bool,
    /// Whether the model supports the effort-level parameter.
    pub supports_effort_level: bool,
    /// Whether the model supports `Max` effort level.
    pub supports_max_effort: bool,
    /// Maximum output tokens per request.
    pub max_output_tokens: u32,
    /// Standard context window size (without 1M).
    pub context_window: u32,
    /// Default effort level for this model.
    pub default_effort: EffortLevel,
}

// ── Capability table ────────────────────────────────────────────────────
// Keyed on *canonical* first-party model ID (lowercase).

static CAPABILITY_TABLE: LazyLock<Vec<(&str, ModelCapabilities)>> = LazyLock::new(|| {
    vec![
        // ── Opus 4.7 ──────────────────────────────────────────────────
        (
            "claude-opus-4-7",
            ModelCapabilities {
                supports_images: true,
                supports_tool_use: true,
                supports_extended_thinking: true,
                supports_1m_context: true,
                supports_effort_level: true,
                supports_max_effort: true,
                max_output_tokens: 32_768,
                context_window: 200_000,
                default_effort: EffortLevel::High,
            },
        ),
        // ── Opus 4.6 (legacy snapshot compatibility) ──────────────────
        (
            "claude-opus-4-6",
            ModelCapabilities {
                supports_images: true,
                supports_tool_use: true,
                supports_extended_thinking: true,
                supports_1m_context: true,
                supports_effort_level: true,
                supports_max_effort: true,
                max_output_tokens: 32_768,
                context_window: 200_000,
                default_effort: EffortLevel::High,
            },
        ),
        // ── Opus 4.5 ──────────────────────────────────────────────────
        (
            "claude-opus-4-5-20251101",
            ModelCapabilities {
                supports_images: true,
                supports_tool_use: true,
                supports_extended_thinking: true,
                supports_1m_context: true,
                supports_effort_level: true,
                supports_max_effort: true,
                max_output_tokens: 32_768,
                context_window: 200_000,
                default_effort: EffortLevel::High,
            },
        ),
        // ── Opus 4.1 ──────────────────────────────────────────────────
        (
            "claude-opus-4-1-20250805",
            ModelCapabilities {
                supports_images: true,
                supports_tool_use: true,
                supports_extended_thinking: true,
                supports_1m_context: false,
                supports_effort_level: true,
                supports_max_effort: true,
                max_output_tokens: 32_768,
                context_window: 200_000,
                default_effort: EffortLevel::High,
            },
        ),
        // ── Opus 4 ────────────────────────────────────────────────────
        (
            "claude-opus-4-20250514",
            ModelCapabilities {
                supports_images: true,
                supports_tool_use: true,
                supports_extended_thinking: true,
                supports_1m_context: false,
                supports_effort_level: true,
                supports_max_effort: true,
                max_output_tokens: 32_768,
                context_window: 200_000,
                default_effort: EffortLevel::High,
            },
        ),
        // ── Sonnet 4.6 ────────────────────────────────────────────────
        (
            "claude-sonnet-4-6",
            ModelCapabilities {
                supports_images: true,
                supports_tool_use: true,
                supports_extended_thinking: true,
                supports_1m_context: true,
                supports_effort_level: true,
                supports_max_effort: true,
                max_output_tokens: 16_384,
                context_window: 200_000,
                default_effort: EffortLevel::Medium,
            },
        ),
        // ── Sonnet 4.5 ────────────────────────────────────────────────
        (
            "claude-sonnet-4-5-20250929",
            ModelCapabilities {
                supports_images: true,
                supports_tool_use: true,
                supports_extended_thinking: true,
                supports_1m_context: true,
                supports_effort_level: true,
                supports_max_effort: true,
                max_output_tokens: 16_384,
                context_window: 200_000,
                default_effort: EffortLevel::Medium,
            },
        ),
        // ── Sonnet 4 ──────────────────────────────────────────────────
        (
            "claude-sonnet-4-20250514",
            ModelCapabilities {
                supports_images: true,
                supports_tool_use: true,
                supports_extended_thinking: true,
                supports_1m_context: true,
                supports_effort_level: true,
                supports_max_effort: true,
                max_output_tokens: 16_384,
                context_window: 200_000,
                default_effort: EffortLevel::Medium,
            },
        ),
        // ── Sonnet 3.7 ────────────────────────────────────────────────
        (
            "claude-3-7-sonnet-20250219",
            ModelCapabilities {
                supports_images: true,
                supports_tool_use: true,
                supports_extended_thinking: true,
                supports_1m_context: false,
                supports_effort_level: true,
                supports_max_effort: false,
                max_output_tokens: 8_192,
                context_window: 200_000,
                default_effort: EffortLevel::Medium,
            },
        ),
        // ── Sonnet 3.5 v2 ─────────────────────────────────────────────
        (
            "claude-3-5-sonnet-20241022",
            ModelCapabilities {
                supports_images: true,
                supports_tool_use: true,
                supports_extended_thinking: false,
                supports_1m_context: false,
                supports_effort_level: false,
                supports_max_effort: false,
                max_output_tokens: 8_192,
                context_window: 200_000,
                default_effort: EffortLevel::Medium,
            },
        ),
        // ── Haiku 4.5 ─────────────────────────────────────────────────
        (
            "claude-haiku-4-5-20251001",
            ModelCapabilities {
                supports_images: true,
                supports_tool_use: true,
                supports_extended_thinking: false,
                supports_1m_context: false,
                supports_effort_level: false,
                supports_max_effort: false,
                max_output_tokens: 8_192,
                context_window: 200_000,
                default_effort: EffortLevel::Medium,
            },
        ),
        // ── Haiku 3.5 ─────────────────────────────────────────────────
        (
            "claude-3-5-haiku-20241022",
            ModelCapabilities {
                supports_images: true,
                supports_tool_use: true,
                supports_extended_thinking: false,
                supports_1m_context: false,
                supports_effort_level: false,
                supports_max_effort: false,
                max_output_tokens: 8_192,
                context_window: 200_000,
                default_effort: EffortLevel::Medium,
            },
        ),
    ]
});

/// Default capabilities returned for unknown / custom models.
fn default_capabilities() -> ModelCapabilities {
    ModelCapabilities {
        supports_images: true,
        supports_tool_use: true,
        supports_extended_thinking: false,
        supports_1m_context: false,
        supports_effort_level: false,
        supports_max_effort: false,
        max_output_tokens: 8_192,
        context_window: 200_000,
        default_effort: EffortLevel::Medium,
    }
}

/// Strip date suffix and provider-specific decorations to obtain a canonical
/// prefix for table lookup.
fn canonicalise_for_lookup(model_id: &str) -> String {
    let lower = model_id.to_lowercase();
    // Remove common Bedrock cross-region inference prefixes
    let stripped = lower
        .strip_prefix("us.anthropic.")
        .or_else(|| lower.strip_prefix("eu.anthropic."))
        .or_else(|| lower.strip_prefix("apac.anthropic."))
        .or_else(|| lower.strip_prefix("global.anthropic."))
        .unwrap_or(&lower);

    // Remove `-v1:0` or `-v1` suffix (Bedrock)
    stripped
        .strip_suffix("-v1:0")
        .unwrap_or(stripped)
        .strip_suffix("-v1")
        .unwrap_or(stripped)
        .to_owned()
}

/// Retrieve capabilities for a model.
///
/// The lookup is fuzzy: it first tries an exact match on the canonical table,
/// then falls back to substring matching (so that dated variants like
/// `claude-opus-4-6-20260101` still match the `claude-opus-4-6` entry).
/// Returns a sensible default for completely unknown models.
pub fn get_capabilities(model_id: &str) -> ModelCapabilities {
    let key = canonicalise_for_lookup(model_id);

    // Exact match
    for (id, caps) in CAPABILITY_TABLE.iter() {
        if key == **id {
            return caps.clone();
        }
    }

    // Substring / prefix match (longest ID first, which is how the table is
    // roughly ordered for dated models).
    for (id, caps) in CAPABILITY_TABLE.iter() {
        if key.contains(*id) || id.contains(key.as_str()) {
            return caps.clone();
        }
    }

    default_capabilities()
}

/// Returns `true` when the model supports a 1M-token context window.
pub fn model_supports_1m(model_id: &str) -> bool {
    get_capabilities(model_id).supports_1m_context
}

/// Returns `true` when the model supports extended thinking.
pub fn model_supports_thinking(model_id: &str) -> bool {
    get_capabilities(model_id).supports_extended_thinking
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_model_capabilities() {
        let caps = get_capabilities("claude-opus-4-7");
        assert!(caps.supports_1m_context);
        assert!(caps.supports_extended_thinking);
        assert!(caps.supports_max_effort);
        assert_eq!(caps.default_effort, EffortLevel::High);
    }

    #[test]
    fn sonnet_46_capabilities() {
        let caps = get_capabilities("claude-sonnet-4-6");
        assert!(caps.supports_1m_context);
        assert!(caps.supports_extended_thinking);
        assert_eq!(caps.default_effort, EffortLevel::Medium);
    }

    #[test]
    fn unknown_model_gets_defaults() {
        let caps = get_capabilities("my-custom-model-v1");
        assert!(!caps.supports_1m_context);
        assert!(!caps.supports_extended_thinking);
        assert!(caps.supports_images);
    }

    #[test]
    fn bedrock_id_lookup() {
        let caps = get_capabilities("us.anthropic.claude-opus-4-7-v1");
        assert!(caps.supports_1m_context);
    }

    #[test]
    fn model_supports_1m_check() {
        assert!(model_supports_1m("claude-opus-4-7"));
        assert!(!model_supports_1m("claude-haiku-4-5-20251001"));
    }
}
