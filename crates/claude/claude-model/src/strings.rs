//! Model string utilities.
//!
//! Maps each model version to its provider-specific ID string.  Provides
//! lookup and resolution functions for converting between canonical IDs
//! and provider-specific model strings.

use std::collections::HashMap;
use std::sync::LazyLock;

use crate::configs::{ALL_MODEL_CONFIGS, ModelKey, key_for_canonical_id, model_id_for_provider};
use crate::providers::ModelProvider;

// ── Model strings map ────────────────────────────────────────────────────

/// Maps each model key to its provider-specific ID string.
pub type ModelStrings = HashMap<ModelKey, String>;

/// Build the built-in model strings for a given provider.
pub fn get_builtin_model_strings(provider: &ModelProvider) -> ModelStrings {
    let mut out = HashMap::new();
    for entry in ALL_MODEL_CONFIGS.iter() {
        if let Some(id) = model_id_for_provider(entry.key, provider) {
            out.insert(entry.key, id.to_owned());
        }
    }
    out
}

// ── Static default strings ───────────────────────────────────────────────

/// Default model strings for first-party provider.
static DEFAULT_STRINGS: LazyLock<ModelStrings> =
    LazyLock::new(|| get_builtin_model_strings(&ModelProvider::Anthropic));

// ── Lookup helpers ───────────────────────────────────────────────────────

/// Get the model string for a given key using the default (first-party)
/// provider.
pub fn get_model_string(key: ModelKey) -> Option<&'static str> {
    model_id_for_provider(key, &ModelProvider::Anthropic)
}

/// Get the model string for a given key and provider.
pub fn get_model_string_for_provider(key: ModelKey, provider: &ModelProvider) -> Option<String> {
    model_id_for_provider(key, provider).map(|s| s.to_owned())
}

/// Get all model strings for the default provider.
pub fn get_default_model_strings() -> &'static ModelStrings {
    &DEFAULT_STRINGS
}

// ── Model override resolution ────────────────────────────────────────────

/// Apply user-configured model overrides on top of the built-in model
/// strings.  Overrides are keyed by canonical first-party model ID and map
/// to arbitrary provider-specific strings.
pub fn apply_model_overrides(
    strings: &ModelStrings,
    overrides: &HashMap<String, String>,
) -> ModelStrings {
    let mut out = strings.clone();
    for (canonical_id, override_value) in overrides {
        if let Some(key) = key_for_canonical_id(canonical_id) {
            out.insert(key, override_value.clone());
        }
    }
    out
}

/// Resolve an overridden model ID back to its canonical first-party model
/// ID.  If the input doesn't match any current override value, it is
/// returned unchanged.
pub fn resolve_overridden_model<'a>(
    model_id: &'a str,
    overrides: &'a HashMap<String, String>,
) -> &'a str {
    for (canonical_id, override_value) in overrides {
        if override_value == model_id {
            return canonical_id.as_str();
        }
    }
    model_id
}

// ── Convenience accessors ────────────────────────────────────────────────

/// Get the Sonnet 4.6 model string for the given provider.
pub fn sonnet46_string(provider: &ModelProvider) -> String {
    get_model_string_for_provider(ModelKey::Sonnet46, provider)
        .unwrap_or_else(|| "claude-sonnet-4-6".into())
}

/// Get the Opus 4.6 model string for the given provider.
pub fn opus46_string(provider: &ModelProvider) -> String {
    get_model_string_for_provider(ModelKey::Opus46, provider)
        .unwrap_or_else(|| "claude-opus-4-6".into())
}

/// Get the Haiku 4.5 model string for the given provider.
pub fn haiku45_string(provider: &ModelProvider) -> String {
    get_model_string_for_provider(ModelKey::Haiku45, provider)
        .unwrap_or_else(|| "claude-haiku-4-5-20251001".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_strings_anthropic() {
        let strings = get_builtin_model_strings(&ModelProvider::Anthropic);
        assert_eq!(
            strings.get(&ModelKey::Opus46),
            Some(&"claude-opus-4-7".to_string())
        );
        assert_eq!(
            strings.get(&ModelKey::Sonnet46),
            Some(&"claude-sonnet-4-6".to_string())
        );
    }

    #[test]
    fn builtin_strings_bedrock() {
        let strings = get_builtin_model_strings(&ModelProvider::AwsBedrock { region: None });
        assert_eq!(
            strings.get(&ModelKey::Opus46),
            Some(&"us.anthropic.claude-opus-4-7-v1".to_string())
        );
    }

    #[test]
    fn builtin_strings_vertex() {
        let strings = get_builtin_model_strings(&ModelProvider::GcpVertex { project: None });
        assert_eq!(
            strings.get(&ModelKey::Sonnet45),
            Some(&"claude-sonnet-4-5@20250929".to_string())
        );
    }

    #[test]
    fn model_string_lookup() {
        assert_eq!(get_model_string(ModelKey::Opus46), Some("claude-opus-4-7"));
        assert_eq!(
            get_model_string(ModelKey::Haiku45),
            Some("claude-haiku-4-5-20251001")
        );
    }

    #[test]
    fn apply_overrides() {
        let strings = get_builtin_model_strings(&ModelProvider::AwsBedrock { region: None });
        let mut overrides = HashMap::new();
        overrides.insert(
            "claude-opus-4-7".into(),
            "arn:aws:bedrock:us-east-1:123:inference-profile/custom-opus".into(),
        );
        let result = apply_model_overrides(&strings, &overrides);
        assert_eq!(
            result.get(&ModelKey::Opus46),
            Some(&"arn:aws:bedrock:us-east-1:123:inference-profile/custom-opus".to_string())
        );
        // Other keys should be unchanged.
        assert!(result.contains_key(&ModelKey::Sonnet46));
    }

    #[test]
    fn resolve_overridden() {
        let mut overrides = HashMap::new();
        overrides.insert(
            "claude-opus-4-7".into(),
            "arn:aws:bedrock:us-east-1:123:inference-profile/custom-opus".into(),
        );
        assert_eq!(
            resolve_overridden_model(
                "arn:aws:bedrock:us-east-1:123:inference-profile/custom-opus",
                &overrides
            ),
            "claude-opus-4-7"
        );
        // Non-overridden model returned unchanged.
        assert_eq!(
            resolve_overridden_model("claude-sonnet-4-6", &overrides),
            "claude-sonnet-4-6"
        );
    }

    #[test]
    fn resolve_overridden_empty() {
        let overrides = HashMap::new();
        assert_eq!(
            resolve_overridden_model("claude-opus-4-7", &overrides),
            "claude-opus-4-7"
        );
    }

    #[test]
    fn convenience_accessors() {
        assert_eq!(
            sonnet46_string(&ModelProvider::Anthropic),
            "claude-sonnet-4-6"
        );
        assert_eq!(opus46_string(&ModelProvider::Anthropic), "claude-opus-4-7");
        assert_eq!(
            haiku45_string(&ModelProvider::Anthropic),
            "claude-haiku-4-5-20251001"
        );
    }

    #[test]
    fn default_model_strings_populated() {
        let strings = get_default_model_strings();
        assert!(strings.len() >= 11);
    }
}
