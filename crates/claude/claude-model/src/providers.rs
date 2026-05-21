//! Provider detection.
//!
//! Determines which API provider (Anthropic first-party, AWS Bedrock, GCP
//! Vertex, or an OpenAI-compatible endpoint) should serve a given model
//! request.

use serde::{Deserialize, Serialize};

/// The API provider that will handle model requests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelProvider {
    /// Anthropic first-party API (`api.anthropic.com`).
    Anthropic,
    /// AWS Bedrock — optionally carries the configured region.
    AwsBedrock { region: Option<String> },
    /// Google Cloud Vertex AI — optionally carries the GCP project ID.
    GcpVertex { project: Option<String> },
    /// Azure Foundry or any OpenAI-compatible endpoint.
    OpenAiCompatible { base_url: String },
}

impl std::fmt::Display for ModelProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Anthropic => write!(f, "firstParty"),
            Self::AwsBedrock { .. } => write!(f, "bedrock"),
            Self::GcpVertex { .. } => write!(f, "vertex"),
            Self::OpenAiCompatible { .. } => write!(f, "openai_compatible"),
        }
    }
}

/// Configuration that influences provider detection.
#[derive(Debug, Clone, Default)]
pub struct ProviderConfig {
    /// `CLAUDE_CODE_USE_BEDROCK` / settings flag.
    pub use_bedrock: bool,
    /// `CLAUDE_CODE_USE_VERTEX` / settings flag.
    pub use_vertex: bool,
    /// Explicit provider string from settings (`"bedrock"`, `"vertex"`, etc.).
    pub provider: Option<String>,
    /// Base URL for OpenAI-compatible providers.
    pub openai_base_url: Option<String>,
}

/// Detect which provider should be used based on configuration.
///
/// Priority:
/// 1. Explicit `use_bedrock` flag
/// 2. Explicit `use_vertex` flag
/// 3. Configured provider string
/// 4. OpenAI-compatible base URL
/// 5. Default to Anthropic first-party
pub fn detect_provider(config: &ProviderConfig) -> ModelProvider {
    if config.use_bedrock {
        return ModelProvider::AwsBedrock { region: None };
    }
    if config.use_vertex {
        return ModelProvider::GcpVertex { project: None };
    }
    if let Some(ref provider) = config.provider {
        match provider.to_lowercase().as_str() {
            "bedrock" => return ModelProvider::AwsBedrock { region: None },
            "vertex" => return ModelProvider::GcpVertex { project: None },
            "foundry" | "openai_compatible" => {
                return ModelProvider::OpenAiCompatible {
                    base_url: config.openai_base_url.clone().unwrap_or_default(),
                };
            }
            _ => {}
        }
    }
    if let Some(ref url) = config.openai_base_url {
        return ModelProvider::OpenAiCompatible {
            base_url: url.clone(),
        };
    }
    ModelProvider::Anthropic
}

/// Returns `true` when `canonical_id` ends with a date suffix (`-YYYYMMDD`).
fn has_date_suffix(canonical_id: &str) -> bool {
    let len = canonical_id.len();
    if len < 9 {
        return false;
    }
    let suffix_start = len - 8;
    canonical_id.as_bytes().get(suffix_start - 1) == Some(&b'-')
        && canonical_id[suffix_start..]
            .bytes()
            .all(|b| b.is_ascii_digit())
}

/// Returns the provider-specific model ID for a given canonical first-party
/// model ID.
pub fn provider_model_id(canonical_id: &str, provider: &ModelProvider) -> String {
    match provider {
        ModelProvider::Anthropic => canonical_id.to_owned(),
        ModelProvider::AwsBedrock { .. } => {
            // Bedrock uses `us.anthropic.<canonical>-v1:0` for cross-region
            // inference profiles, or the ARN pattern for custom profiles.
            if has_date_suffix(canonical_id) {
                // Dated models: us.anthropic.claude-xxx-YYYYMMDD-v1:0
                format!("us.anthropic.{canonical_id}-v1:0")
            } else {
                // Undated models: us.anthropic.claude-xxx-v1
                format!("us.anthropic.{canonical_id}-v1")
            }
        }
        ModelProvider::GcpVertex { .. } => {
            // Vertex uses `claude-xxx@YYYYMMDD` for dated models.
            if let Some(date_pos) = canonical_id.rfind("-20") {
                let date_part = &canonical_id[date_pos + 1..];
                let base = &canonical_id[..date_pos];
                // Convert dashes in the model family to `@` separator
                format!("{base}@{date_part}")
            } else {
                canonical_id.to_owned()
            }
        }
        ModelProvider::OpenAiCompatible { .. } => {
            // OpenAI-compatible providers typically use the canonical ID as-is.
            canonical_id.to_owned()
        }
    }
}

/// Returns `true` when the given base URL points to a first-party Anthropic
/// API endpoint.
pub fn is_first_party_base_url(base_url: &str) -> bool {
    // Simple host extraction without the `url` crate.
    let trimmed = base_url
        .strip_prefix("https://")
        .or_else(|| base_url.strip_prefix("http://"))
        .unwrap_or(base_url);
    let host = trimmed.split('/').next().unwrap_or(trimmed);
    // Strip port if present.
    let host = host.split(':').next().unwrap_or(host);
    host == "api.anthropic.com" || host == "api-staging.anthropic.com"
}

/// Returns the default Opus model for the given provider.
///
/// Third-party providers may lag behind first-party availability.
pub fn default_opus_model(provider: &ModelProvider) -> &'static str {
    match provider {
        ModelProvider::Anthropic => "claude-opus-4-7",
        // 3P providers may not have 4.7 yet — keep on latest confirmed.
        ModelProvider::AwsBedrock { .. } => "claude-opus-4-7",
        ModelProvider::GcpVertex { .. } => "claude-opus-4-7",
        ModelProvider::OpenAiCompatible { .. } => "claude-opus-4-7",
    }
}

/// Returns the default Sonnet model for the given provider.
pub fn default_sonnet_model(provider: &ModelProvider) -> &'static str {
    match provider {
        ModelProvider::Anthropic => "claude-sonnet-4-6",
        // 3P providers default to 4.5 since they may not have 4.6 yet.
        ModelProvider::AwsBedrock { .. } => "claude-sonnet-4-5-20250929",
        ModelProvider::GcpVertex { .. } => "claude-sonnet-4-5-20250929",
        ModelProvider::OpenAiCompatible { .. } => "claude-sonnet-4-5-20250929",
    }
}

/// Returns the default Haiku model for the given provider.
pub fn default_haiku_model(_provider: &ModelProvider) -> &'static str {
    // Haiku 4.5 is available on all platforms.
    "claude-haiku-4-5-20251001"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_default_anthropic() {
        let config = ProviderConfig::default();
        assert_eq!(detect_provider(&config), ModelProvider::Anthropic);
    }

    #[test]
    fn detect_bedrock() {
        let config = ProviderConfig {
            use_bedrock: true,
            ..Default::default()
        };
        assert_eq!(
            detect_provider(&config),
            ModelProvider::AwsBedrock { region: None }
        );
    }

    #[test]
    fn detect_vertex() {
        let config = ProviderConfig {
            use_vertex: true,
            ..Default::default()
        };
        assert_eq!(
            detect_provider(&config),
            ModelProvider::GcpVertex { project: None }
        );
    }

    #[test]
    fn detect_openai_compatible() {
        let config = ProviderConfig {
            openai_base_url: Some("https://my-llm.example.com/v1".into()),
            ..Default::default()
        };
        assert_eq!(
            detect_provider(&config),
            ModelProvider::OpenAiCompatible {
                base_url: "https://my-llm.example.com/v1".into(),
            }
        );
    }

    #[test]
    fn provider_model_id_bedrock() {
        let provider = ModelProvider::AwsBedrock { region: None };
        assert_eq!(
            provider_model_id("claude-sonnet-4-5-20250929", &provider),
            "us.anthropic.claude-sonnet-4-5-20250929-v1:0"
        );
        assert_eq!(
            provider_model_id("claude-opus-4-6", &provider),
            "us.anthropic.claude-opus-4-6-v1"
        );
    }

    #[test]
    fn first_party_url_check() {
        assert!(is_first_party_base_url("https://api.anthropic.com/v1"));
        assert!(is_first_party_base_url(
            "https://api-staging.anthropic.com/v1"
        ));
        assert!(!is_first_party_base_url("https://my-proxy.example.com"));
    }
}
