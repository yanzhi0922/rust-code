//! Teammate model selection and fallback.
//!
//! Provides logic for selecting the appropriate LLM model
//! for a teammate, with fallback chain support.

use crate::error::SwarmResult;

/// Default model for teammates.
pub const DEFAULT_MODEL: &str = "claude-sonnet-4-20250514";

/// Fallback model chain.
pub const FALLBACK_MODELS: &[&str] = &[
    "claude-sonnet-4-20250514",
    "claude-3-5-sonnet-20241022",
    "claude-3-haiku-20240307",
];

/// Model selection configuration.
#[derive(Debug, Clone)]
pub struct ModelConfig {
    /// Preferred model.
    pub preferred: Option<String>,
    /// Whether to allow fallback.
    pub allow_fallback: bool,
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            preferred: None,
            allow_fallback: true,
        }
    }
}

impl ModelConfig {
    /// Create a new model config with a preferred model.
    #[must_use]
    pub fn with_preferred(model: impl Into<String>) -> Self {
        Self {
            preferred: Some(model.into()),
            allow_fallback: true,
        }
    }

    /// Create a config that only uses the default model.
    #[must_use]
    pub fn default_only() -> Self {
        Self {
            preferred: None,
            allow_fallback: false,
        }
    }
}

/// Select a model for a teammate.
///
/// Uses the preferred model if specified, otherwise falls back
/// through the fallback chain.
pub fn select_model(config: &ModelConfig) -> String {
    if let Some(ref model) = config.preferred {
        return model.clone();
    }
    DEFAULT_MODEL.to_owned()
}

/// Get the fallback model chain for a given starting model.
///
/// Returns the fallback chain starting from the given model,
/// or the default chain if the model is not in the chain.
pub fn fallback_chain(model: &str) -> Vec<String> {
    let chain: Vec<String> = FALLBACK_MODELS
        .iter()
        .skip_while(|m| **m != model)
        .map(|m| (*m).to_owned())
        .collect();

    if chain.is_empty() {
        // Model not in chain, return default chain.
        FALLBACK_MODELS.iter().map(|m| (*m).to_owned()).collect()
    } else {
        chain
    }
}

/// Validate a model name.
///
/// A valid model name is non-empty and contains only
/// alphanumeric characters, hyphens, underscores, and dots.
pub fn validate_model_name(model: &str) -> SwarmResult<()> {
    if model.is_empty() {
        return Err(crate::error::SwarmError::Config(
            "model name cannot be empty".to_owned(),
        ));
    }
    for c in model.chars() {
        if !c.is_alphanumeric() && c != '-' && c != '_' && c != '.' {
            return Err(crate::error::SwarmError::Config(format!(
                "invalid character '{c}' in model name"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_model_default() {
        let config = ModelConfig::default();
        let model = select_model(&config);
        assert_eq!(model, DEFAULT_MODEL);
    }

    #[test]
    fn select_model_preferred() {
        let config = ModelConfig::with_preferred("gpt-4");
        let model = select_model(&config);
        assert_eq!(model, "gpt-4");
    }

    #[test]
    fn select_model_default_only() {
        let config = ModelConfig::default_only();
        let model = select_model(&config);
        assert_eq!(model, DEFAULT_MODEL);
    }

    #[test]
    fn fallback_chain_from_first() {
        let chain = fallback_chain(FALLBACK_MODELS[0]);
        assert_eq!(chain.len(), FALLBACK_MODELS.len());
        assert_eq!(chain[0], FALLBACK_MODELS[0]);
    }

    #[test]
    fn fallback_chain_from_middle() {
        let chain = fallback_chain(FALLBACK_MODELS[1]);
        assert_eq!(chain.len(), FALLBACK_MODELS.len() - 1);
        assert_eq!(chain[0], FALLBACK_MODELS[1]);
    }

    #[test]
    fn fallback_chain_unknown_model() {
        let chain = fallback_chain("unknown-model");
        assert_eq!(chain.len(), FALLBACK_MODELS.len());
    }

    #[test]
    fn validate_model_name_valid() {
        assert!(validate_model_name("claude-sonnet-4-20250514").is_ok());
        assert!(validate_model_name("gpt-4").is_ok());
        assert!(validate_model_name("model_v2.1").is_ok());
    }

    #[test]
    fn validate_model_name_empty() {
        assert!(validate_model_name("").is_err());
    }

    #[test]
    fn validate_model_name_invalid_chars() {
        assert!(validate_model_name("my model").is_err());
        assert!(validate_model_name("my/model").is_err());
    }

    #[test]
    fn default_model_is_in_fallback_chain() {
        assert!(FALLBACK_MODELS.contains(&DEFAULT_MODEL));
    }

    #[test]
    fn model_config_default_allows_fallback() {
        let config = ModelConfig::default();
        assert!(config.allow_fallback);
    }
}
