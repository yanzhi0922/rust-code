//! Model deprecation warnings.
//!
//! Tracks deprecated models and their retirement dates by provider.
//! Provides warnings when a user selects a model that is scheduled for
//! retirement.

use std::sync::LazyLock;

use crate::providers::ModelProvider;

// ── Types ────────────────────────────────────────────────────────────────

/// Information about a deprecated model.
#[derive(Debug, Clone)]
pub struct DeprecatedModelInfo {
    /// Human-readable model name (e.g. `"Claude 3 Opus"`).
    pub model_name: &'static str,
    /// Retirement dates by provider.  `None` means not deprecated for that
    /// provider.
    pub retirement_dates: DeprecationDates,
}

/// Retirement dates keyed by provider type.
#[derive(Debug, Clone)]
pub struct DeprecationDates {
    /// Anthropic first-party retirement date.
    pub first_party: Option<&'static str>,
    /// AWS Bedrock retirement date.
    pub bedrock: Option<&'static str>,
    /// GCP Vertex AI retirement date.
    pub vertex: Option<&'static str>,
    /// Azure Foundry retirement date.
    pub foundry: Option<&'static str>,
}

/// Result of a deprecation check.
#[derive(Debug, Clone)]
pub enum DeprecationInfo {
    /// Model is deprecated and scheduled for retirement.
    Deprecated {
        model_name: String,
        retirement_date: String,
    },
    /// Model is not deprecated.
    NotDeprecated,
}

// ── Deprecated models table ──────────────────────────────────────────────

/// Deprecated models and their retirement dates.
///
/// Keys are substrings to match in model IDs (case-insensitive).
static DEPRECATED_MODELS: LazyLock<Vec<(&str, DeprecatedModelInfo)>> = LazyLock::new(|| {
    vec![
        (
            "claude-3-opus",
            DeprecatedModelInfo {
                model_name: "Claude 3 Opus",
                retirement_dates: DeprecationDates {
                    first_party: Some("January 5, 2026"),
                    bedrock: Some("January 15, 2026"),
                    vertex: Some("January 5, 2026"),
                    foundry: Some("January 5, 2026"),
                },
            },
        ),
        (
            "claude-3-7-sonnet",
            DeprecatedModelInfo {
                model_name: "Claude 3.7 Sonnet",
                retirement_dates: DeprecationDates {
                    first_party: Some("February 19, 2026"),
                    bedrock: Some("April 28, 2026"),
                    vertex: Some("May 11, 2026"),
                    foundry: Some("February 19, 2026"),
                },
            },
        ),
        (
            "claude-3-5-haiku",
            DeprecatedModelInfo {
                model_name: "Claude 3.5 Haiku",
                retirement_dates: DeprecationDates {
                    first_party: Some("February 19, 2026"),
                    bedrock: None,
                    vertex: None,
                    foundry: None,
                },
            },
        ),
    ]
});

// ── Deprecation check ────────────────────────────────────────────────────

/// Get the retirement date for a deprecated model on a specific provider.
fn get_retirement_date(
    info: &DeprecatedModelInfo,
    provider: &ModelProvider,
) -> Option<&'static str> {
    match provider {
        ModelProvider::Anthropic => info.retirement_dates.first_party,
        ModelProvider::AwsBedrock { .. } => info.retirement_dates.bedrock,
        ModelProvider::GcpVertex { .. } => info.retirement_dates.vertex,
        ModelProvider::OpenAiCompatible { .. } => info.retirement_dates.foundry,
    }
}

/// Check if a model is deprecated and get its deprecation info.
pub fn get_deprecation_info(model_id: &str, provider: &ModelProvider) -> DeprecationInfo {
    let lower = model_id.to_lowercase();

    for (key, info) in DEPRECATED_MODELS.iter() {
        if !lower.contains(key) {
            continue;
        }
        if let Some(date) = get_retirement_date(info, provider) {
            return DeprecationInfo::Deprecated {
                model_name: info.model_name.to_owned(),
                retirement_date: date.to_owned(),
            };
        }
    }

    DeprecationInfo::NotDeprecated
}

/// Get a deprecation warning message for a model, or `None` if not
/// deprecated.
pub fn get_model_deprecation_warning(model_id: &str, provider: &ModelProvider) -> Option<String> {
    match get_deprecation_info(model_id, provider) {
        DeprecationInfo::Deprecated {
            model_name,
            retirement_date,
        } => Some(format!(
            "⚠ {model_name} will be retired on {retirement_date}. Consider switching to a newer model."
        )),
        DeprecationInfo::NotDeprecated => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_3_opus_deprecated_first_party() {
        let info = get_deprecation_info("claude-3-opus-20240229", &ModelProvider::Anthropic);
        match info {
            DeprecationInfo::Deprecated { model_name, .. } => {
                assert_eq!(model_name, "Claude 3 Opus");
            }
            DeprecationInfo::NotDeprecated => panic!("Expected deprecated"),
        }
    }

    #[test]
    fn claude_3_opus_deprecated_bedrock() {
        let info = get_deprecation_info(
            "anthropic.claude-3-opus-20240229-v1:0",
            &ModelProvider::AwsBedrock { region: None },
        );
        match info {
            DeprecationInfo::Deprecated {
                retirement_date, ..
            } => {
                assert_eq!(retirement_date, "January 15, 2026");
            }
            DeprecationInfo::NotDeprecated => panic!("Expected deprecated"),
        }
    }

    #[test]
    fn claude_3_5_haiku_not_deprecated_on_bedrock() {
        let info = get_deprecation_info(
            "anthropic.claude-3-5-haiku-20241022-v1:0",
            &ModelProvider::AwsBedrock { region: None },
        );
        assert!(matches!(info, DeprecationInfo::NotDeprecated));
    }

    #[test]
    fn claude_3_5_haiku_deprecated_first_party() {
        let info = get_deprecation_info("claude-3-5-haiku-20241022", &ModelProvider::Anthropic);
        assert!(matches!(info, DeprecationInfo::Deprecated { .. }));
    }

    #[test]
    fn current_model_not_deprecated() {
        let info = get_deprecation_info("claude-opus-4-6", &ModelProvider::Anthropic);
        assert!(matches!(info, DeprecationInfo::NotDeprecated));
    }

    #[test]
    fn sonnet_4_6_not_deprecated() {
        let info = get_deprecation_info("claude-sonnet-4-6", &ModelProvider::Anthropic);
        assert!(matches!(info, DeprecationInfo::NotDeprecated));
    }

    #[test]
    fn warning_message_format() {
        let msg =
            get_model_deprecation_warning("claude-3-opus-20240229", &ModelProvider::Anthropic);
        assert!(msg.is_some());
        let m = msg.expect("deprecation warning should exist");
        assert!(m.starts_with("⚠"));
        assert!(m.contains("Claude 3 Opus"));
        assert!(m.contains("January 5, 2026"));
    }

    #[test]
    fn no_warning_for_current_model() {
        let msg = get_model_deprecation_warning("claude-opus-4-6", &ModelProvider::Anthropic);
        assert!(msg.is_none());
    }

    #[test]
    fn case_insensitive_matching() {
        let info = get_deprecation_info("CLAUDE-3-OPUS-20240229", &ModelProvider::Anthropic);
        assert!(matches!(info, DeprecationInfo::Deprecated { .. }));
    }

    #[test]
    fn vertex_deprecation_dates() {
        let info = get_deprecation_info(
            "claude-3-7-sonnet-20250219",
            &ModelProvider::GcpVertex { project: None },
        );
        match info {
            DeprecationInfo::Deprecated {
                retirement_date, ..
            } => {
                assert_eq!(retirement_date, "May 11, 2026");
            }
            DeprecationInfo::NotDeprecated => panic!("Expected deprecated"),
        }
    }
}
