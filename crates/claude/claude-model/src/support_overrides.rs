//! Model support overrides for third-party providers.
//!
//! Allows 3P users to declare capability overrides for custom models via
//! environment variables.  This is useful when a custom model pinned via
//! `ANTHROPIC_DEFAULT_*_MODEL` supports capabilities that the default
//! model does not (e.g. extended thinking, effort levels).

use crate::providers::ModelProvider;

// ── Types ────────────────────────────────────────────────────────────────

/// Capabilities that can be overridden for 3P models.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelCapabilityOverride {
    /// Standard effort level support.
    Effort,
    /// Max effort level support.
    MaxEffort,
    /// Extended thinking (chain-of-thought) support.
    Thinking,
    /// Adaptive thinking support.
    AdaptiveThinking,
    /// Interleaved thinking support.
    InterleavedThinking,
}

impl ModelCapabilityOverride {
    /// Parse from a string (case-insensitive).
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "effort" => Some(Self::Effort),
            "max_effort" => Some(Self::MaxEffort),
            "thinking" => Some(Self::Thinking),
            "adaptive_thinking" => Some(Self::AdaptiveThinking),
            "interleaved_thinking" => Some(Self::InterleavedThinking),
            _ => None,
        }
    }

    /// All capability override variants.
    pub const ALL: &[Self] = &[
        Self::Effort,
        Self::MaxEffort,
        Self::Thinking,
        Self::AdaptiveThinking,
        Self::InterleavedThinking,
    ];
}

// ── Tier definitions ─────────────────────────────────────────────────────

/// A tier mapping a pinned model env var to its capabilities env var.
struct TierDef {
    /// Environment variable for the pinned model ID.
    model_env_var: &'static str,
    /// Environment variable for the supported capabilities (comma-separated).
    capabilities_env_var: &'static str,
}

const TIERS: &[TierDef] = &[
    TierDef {
        model_env_var: "ANTHROPIC_DEFAULT_OPUS_MODEL",
        capabilities_env_var: "ANTHROPIC_DEFAULT_OPUS_MODEL_SUPPORTED_CAPABILITIES",
    },
    TierDef {
        model_env_var: "ANTHROPIC_DEFAULT_SONNET_MODEL",
        capabilities_env_var: "ANTHROPIC_DEFAULT_SONNET_MODEL_SUPPORTED_CAPABILITIES",
    },
    TierDef {
        model_env_var: "ANTHROPIC_DEFAULT_HAIKU_MODEL",
        capabilities_env_var: "ANTHROPIC_DEFAULT_HAIKU_MODEL_SUPPORTED_CAPABILITIES",
    },
];

// ── Override check ───────────────────────────────────────────────────────

/// Context for support override checks.
/// Type alias for environment variable getter closure.
pub type EnvGetter = Box<dyn Fn(&str) -> Option<String>>;

pub struct SupportOverrideContext {
    /// The active API provider.
    pub provider: ModelProvider,
    /// Environment variable getter (injectable for testing).
    /// Maps env var name → value.
    pub env_getter: EnvGetter,
}

impl std::fmt::Debug for SupportOverrideContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SupportOverrideContext")
            .field("provider", &self.provider)
            .finish_non_exhaustive()
    }
}

impl Default for SupportOverrideContext {
    fn default() -> Self {
        Self {
            provider: ModelProvider::Anthropic,
            env_getter: Box::new(|key: &str| std::env::var(key).ok()),
        }
    }
}

/// Check whether a 3P model capability override is set for a model that
/// matches one of the pinned `ANTHROPIC_DEFAULT_*_MODEL` env vars.
///
/// Returns:
/// - `Some(true)` if the capability is explicitly listed
/// - `Some(false)` if the model matches but the capability is not listed
/// - `None` if the model doesn't match any pinned tier or the provider is
///   first-party
pub fn get_3p_model_capability_override(
    model: &str,
    capability: ModelCapabilityOverride,
    ctx: &SupportOverrideContext,
) -> Option<bool> {
    // First-party providers never have overrides.
    if matches!(ctx.provider, ModelProvider::Anthropic) {
        return None;
    }

    let get_env = &ctx.env_getter;
    let model_lower = model.to_lowercase();

    for tier in TIERS {
        let pinned = match get_env(tier.model_env_var) {
            Some(v) => v,
            None => continue,
        };
        let capabilities = match get_env(tier.capabilities_env_var) {
            Some(v) => v,
            None => continue,
        };

        if model_lower != pinned.to_lowercase() {
            continue;
        }

        // Parse the comma-separated capabilities list.
        let has_capability = capabilities
            .to_lowercase()
            .split(',')
            .filter_map(|s| ModelCapabilityOverride::parse(s.trim()))
            .any(|c| c == capability);

        return Some(has_capability);
    }

    None
}

// ── Bulk capability query ────────────────────────────────────────────────

/// Get all capability overrides for a model.
///
/// Returns a map of capability → whether it's supported, or `None` if no
/// override is configured for this model.
pub fn get_all_capability_overrides(
    model: &str,
    ctx: &SupportOverrideContext,
) -> Option<Vec<(ModelCapabilityOverride, bool)>> {
    // First-party providers never have overrides.
    if matches!(ctx.provider, ModelProvider::Anthropic) {
        return None;
    }

    let get_env = &ctx.env_getter;
    let model_lower = model.to_lowercase();

    for tier in TIERS {
        let pinned = match get_env(tier.model_env_var) {
            Some(v) => v,
            None => continue,
        };
        let capabilities = match get_env(tier.capabilities_env_var) {
            Some(v) => v,
            None => continue,
        };

        if model_lower != pinned.to_lowercase() {
            continue;
        }

        let parsed: Vec<ModelCapabilityOverride> = capabilities
            .to_lowercase()
            .split(',')
            .filter_map(|s| ModelCapabilityOverride::parse(s.trim()))
            .collect();

        let result: Vec<(ModelCapabilityOverride, bool)> = ModelCapabilityOverride::ALL
            .iter()
            .map(|&cap| (cap, parsed.contains(&cap)))
            .collect();

        return Some(result);
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn mock_ctx(provider: ModelProvider, vars: HashMap<String, String>) -> SupportOverrideContext {
        SupportOverrideContext {
            provider,
            env_getter: Box::new(move |key: &str| vars.get(key).cloned()),
        }
    }

    #[test]
    fn first_party_never_overridden() {
        let ctx = SupportOverrideContext {
            provider: ModelProvider::Anthropic,
            env_getter: Box::new(|_| None),
        };
        assert_eq!(
            get_3p_model_capability_override("any-model", ModelCapabilityOverride::Thinking, &ctx),
            None
        );
    }

    #[test]
    fn override_with_matching_model() {
        let mut vars = HashMap::new();
        vars.insert(
            "ANTHROPIC_DEFAULT_OPUS_MODEL".into(),
            "my-custom-opus".into(),
        );
        vars.insert(
            "ANTHROPIC_DEFAULT_OPUS_MODEL_SUPPORTED_CAPABILITIES".into(),
            "thinking,effort".into(),
        );

        let ctx = mock_ctx(ModelProvider::AwsBedrock { region: None }, vars);

        assert_eq!(
            get_3p_model_capability_override(
                "my-custom-opus",
                ModelCapabilityOverride::Thinking,
                &ctx
            ),
            Some(true)
        );
        assert_eq!(
            get_3p_model_capability_override(
                "my-custom-opus",
                ModelCapabilityOverride::Effort,
                &ctx
            ),
            Some(true)
        );
        assert_eq!(
            get_3p_model_capability_override(
                "my-custom-opus",
                ModelCapabilityOverride::MaxEffort,
                &ctx
            ),
            Some(false)
        );
    }

    #[test]
    fn no_override_for_non_matching_model() {
        let mut vars = HashMap::new();
        vars.insert(
            "ANTHROPIC_DEFAULT_OPUS_MODEL".into(),
            "my-custom-opus".into(),
        );
        vars.insert(
            "ANTHROPIC_DEFAULT_OPUS_MODEL_SUPPORTED_CAPABILITIES".into(),
            "thinking".into(),
        );

        let ctx = mock_ctx(ModelProvider::AwsBedrock { region: None }, vars);

        assert_eq!(
            get_3p_model_capability_override(
                "other-model",
                ModelCapabilityOverride::Thinking,
                &ctx
            ),
            None
        );
    }

    #[test]
    fn no_override_without_capabilities_var() {
        let mut vars = HashMap::new();
        vars.insert(
            "ANTHROPIC_DEFAULT_OPUS_MODEL".into(),
            "my-custom-opus".into(),
        );
        // No capabilities var set.

        let ctx = mock_ctx(ModelProvider::AwsBedrock { region: None }, vars);

        assert_eq!(
            get_3p_model_capability_override(
                "my-custom-opus",
                ModelCapabilityOverride::Thinking,
                &ctx
            ),
            None
        );
    }

    #[test]
    fn case_insensitive_matching() {
        let mut vars = HashMap::new();
        vars.insert(
            "ANTHROPIC_DEFAULT_SONNET_MODEL".into(),
            "My-Custom-Sonnet".into(),
        );
        vars.insert(
            "ANTHROPIC_DEFAULT_SONNET_MODEL_SUPPORTED_CAPABILITIES".into(),
            "THINKING".into(),
        );

        let ctx = mock_ctx(ModelProvider::GcpVertex { project: None }, vars);

        assert_eq!(
            get_3p_model_capability_override(
                "my-custom-sonnet",
                ModelCapabilityOverride::Thinking,
                &ctx
            ),
            Some(true)
        );
    }

    #[test]
    fn all_capabilities_override() {
        let mut vars = HashMap::new();
        vars.insert("ANTHROPIC_DEFAULT_OPUS_MODEL".into(), "my-opus".into());
        vars.insert(
            "ANTHROPIC_DEFAULT_OPUS_MODEL_SUPPORTED_CAPABILITIES".into(),
            "effort,thinking".into(),
        );

        let ctx = mock_ctx(ModelProvider::AwsBedrock { region: None }, vars);

        let all = get_all_capability_overrides("my-opus", &ctx).expect("overrides should exist");
        assert_eq!(all.len(), 5);
        assert_eq!(all[0], (ModelCapabilityOverride::Effort, true));
        assert_eq!(all[2], (ModelCapabilityOverride::Thinking, true));
        assert_eq!(all[1], (ModelCapabilityOverride::MaxEffort, false));
    }

    #[test]
    fn capability_parse() {
        assert_eq!(
            ModelCapabilityOverride::parse("thinking"),
            Some(ModelCapabilityOverride::Thinking)
        );
        assert_eq!(
            ModelCapabilityOverride::parse("MAX_EFFORT"),
            Some(ModelCapabilityOverride::MaxEffort)
        );
        assert_eq!(ModelCapabilityOverride::parse("unknown"), None);
    }

    #[test]
    fn all_capabilities_listed() {
        assert_eq!(ModelCapabilityOverride::ALL.len(), 5);
    }
}
