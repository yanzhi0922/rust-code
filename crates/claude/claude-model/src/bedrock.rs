//! AWS Bedrock model support.
//!
//! Provides utilities for working with Bedrock model IDs, including:
//! - Region prefix extraction and application for cross-region inference
//! - ARN parsing
//! - Foundation model detection

use crate::providers::ModelProvider;

// ── Region prefixes ──────────────────────────────────────────────────────

/// Valid Bedrock cross-region inference profile prefixes.
pub const BEDROCK_REGION_PREFIXES: &[&str] = &["us", "eu", "apac", "global"];

/// A validated Bedrock region prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BedrockRegionPrefix {
    Us,
    Eu,
    Apac,
    Global,
}

impl BedrockRegionPrefix {
    /// Return the string representation of this prefix.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Us => "us",
            Self::Eu => "eu",
            Self::Apac => "apac",
            Self::Global => "global",
        }
    }

    /// Parse a region prefix from a string.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "us" => Some(Self::Us),
            "eu" => Some(Self::Eu),
            "apac" => Some(Self::Apac),
            "global" => Some(Self::Global),
            _ => None,
        }
    }
}

impl std::fmt::Display for BedrockRegionPrefix {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ── Foundation model detection ───────────────────────────────────────────

/// Returns `true` when `model_id` is a Bedrock foundation model (starts with
/// `anthropic.`).
pub fn is_foundation_model(model_id: &str) -> bool {
    model_id.starts_with("anthropic.")
}

// ── ARN handling ─────────────────────────────────────────────────────────

/// Extract the model / inference-profile ID from a Bedrock ARN.
///
/// If the input is not an ARN, returns it unchanged.
///
/// # ARN formats handled
/// - `arn:aws:bedrock:<region>:<account>:inference-profile/<profile-id>`
/// - `arn:aws:bedrock:<region>:<account>:application-inference-profile/<profile-id>`
/// - `arn:aws:bedrock:<region>::foundation-model/<model-id>`
pub fn extract_model_id_from_arn(model_id: &str) -> &str {
    if !model_id.starts_with("arn:") {
        return model_id;
    }
    let last_slash = model_id.rfind('/');
    match last_slash {
        Some(pos) => &model_id[pos + 1..],
        None => model_id,
    }
}

// ── Region prefix extraction ─────────────────────────────────────────────

/// Extract the region prefix from a Bedrock cross-region inference model ID.
///
/// Handles both plain model IDs and full ARN format.
///
/// # Examples
/// - `"eu.anthropic.claude-sonnet-4-5-20250929-v1:0"` → `Some(Eu)`
/// - `"us.anthropic.claude-3-7-sonnet-20250219-v1:0"` → `Some(Us)`
/// - `"anthropic.claude-3-5-sonnet-20241022-v2:0"` → `None` (foundation model)
/// - `"claude-sonnet-4-5-20250929"` → `None` (first-party format)
pub fn get_bedrock_region_prefix(model_id: &str) -> Option<BedrockRegionPrefix> {
    let effective_id = extract_model_id_from_arn(model_id);

    for &prefix_str in BEDROCK_REGION_PREFIXES {
        let pattern = format!("{prefix_str}.anthropic.");
        if effective_id.starts_with(&pattern) {
            return BedrockRegionPrefix::parse(prefix_str);
        }
    }
    None
}

// ── Region prefix application ────────────────────────────────────────────

/// Apply a region prefix to a Bedrock model ID.
///
/// - If the model already has a different region prefix, it will be replaced.
/// - If the model is a foundation model (`anthropic.*`), the prefix will be
///   added.
/// - If the model is not a Bedrock model, it will be returned as-is.
///
/// # Examples
/// - `apply_bedrock_region_prefix("us.anthropic.claude-sonnet-4-5-v1:0", Eu)`
///   → `"eu.anthropic.claude-sonnet-4-5-v1:0"`
/// - `apply_bedrock_region_prefix("anthropic.claude-sonnet-4-5-v1:0", Eu)`
///   → `"eu.anthropic.claude-sonnet-4-5-v1:0"`
/// - `apply_bedrock_region_prefix("claude-sonnet-4-5-20250929", Eu)`
///   → `"claude-sonnet-4-5-20250929"` (not a Bedrock model)
pub fn apply_bedrock_region_prefix(model_id: &str, prefix: BedrockRegionPrefix) -> String {
    // Check if it already has a region prefix and replace it.
    if let Some(existing) = get_bedrock_region_prefix(model_id) {
        if existing != prefix {
            return model_id.replacen(&format!("{}.", existing), &format!("{}.", prefix), 1);
        }
        return model_id.to_owned();
    }

    // Check if it's a foundation model (anthropic.*) and add the prefix.
    if is_foundation_model(model_id) {
        return format!("{}.{}", prefix.as_str(), model_id);
    }

    // Not a Bedrock model format, return as-is.
    model_id.to_owned()
}

// ── Find first match ─────────────────────────────────────────────────────

/// Find the first profile in a list that contains the given substring.
pub fn find_first_match<'a>(profiles: &'a [String], substring: &str) -> Option<&'a str> {
    profiles
        .iter()
        .find(|p| p.contains(substring))
        .map(|s| s.as_str())
}

// ── Bedrock model ID construction ────────────────────────────────────────

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

/// Build a Bedrock model ID from a canonical first-party ID, optionally with
/// a region prefix.
pub fn build_bedrock_model_id(
    canonical_id: &str,
    region_prefix: Option<BedrockRegionPrefix>,
) -> String {
    let base = if has_date_suffix(canonical_id) {
        format!("anthropic.{}-v1:0", canonical_id)
    } else {
        format!("anthropic.{}-v1", canonical_id)
    };

    match region_prefix {
        Some(prefix) => format!("{}.{}", prefix.as_str(), base),
        None => base,
    }
}

/// Check if a model ID looks like a Bedrock model ID.
pub fn is_bedrock_model_id(model_id: &str) -> bool {
    model_id.starts_with("anthropic.")
        || model_id.contains(".anthropic.")
        || model_id.starts_with("arn:aws:bedrock:")
}

/// Get the provider for Bedrock model detection.
/// Returns `true` if the given provider is AWS Bedrock.
pub fn is_bedrock_provider(provider: &ModelProvider) -> bool {
    matches!(provider, ModelProvider::AwsBedrock { .. })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn foundation_model_detection() {
        assert!(is_foundation_model("anthropic.claude-sonnet-4-5-v1:0"));
        assert!(!is_foundation_model("us.anthropic.claude-sonnet-4-5-v1:0"));
        assert!(!is_foundation_model("claude-sonnet-4-5-20250929"));
    }

    #[test]
    fn extract_from_arn() {
        assert_eq!(
            extract_model_id_from_arn(
                "arn:aws:bedrock:us-east-1:123:inference-profile/us.anthropic.claude-opus-4-6-v1"
            ),
            "us.anthropic.claude-opus-4-6-v1"
        );
        assert_eq!(
            extract_model_id_from_arn(
                "arn:aws:bedrock:ap-northeast-2:123:inference-profile/global.anthropic.claude-opus-4-6-v1"
            ),
            "global.anthropic.claude-opus-4-6-v1"
        );
        // Not an ARN — returned unchanged.
        assert_eq!(
            extract_model_id_from_arn("us.anthropic.claude-opus-4-6-v1"),
            "us.anthropic.claude-opus-4-6-v1"
        );
    }

    #[test]
    fn region_prefix_extraction() {
        assert_eq!(
            get_bedrock_region_prefix("eu.anthropic.claude-sonnet-4-5-20250929-v1:0"),
            Some(BedrockRegionPrefix::Eu)
        );
        assert_eq!(
            get_bedrock_region_prefix("us.anthropic.claude-3-7-sonnet-20250219-v1:0"),
            Some(BedrockRegionPrefix::Us)
        );
        assert_eq!(
            get_bedrock_region_prefix("anthropic.claude-3-5-sonnet-20241022-v2:0"),
            None
        );
        assert_eq!(
            get_bedrock_region_prefix("claude-sonnet-4-5-20250929"),
            None
        );
    }

    #[test]
    fn region_prefix_from_arn() {
        assert_eq!(
            get_bedrock_region_prefix(
                "arn:aws:bedrock:ap-northeast-2:123:inference-profile/global.anthropic.claude-opus-4-6-v1"
            ),
            Some(BedrockRegionPrefix::Global)
        );
    }

    #[test]
    fn apply_prefix_replace() {
        assert_eq!(
            apply_bedrock_region_prefix(
                "us.anthropic.claude-sonnet-4-5-v1:0",
                BedrockRegionPrefix::Eu
            ),
            "eu.anthropic.claude-sonnet-4-5-v1:0"
        );
    }

    #[test]
    fn apply_prefix_add_to_foundation() {
        assert_eq!(
            apply_bedrock_region_prefix(
                "anthropic.claude-sonnet-4-5-v1:0",
                BedrockRegionPrefix::Eu
            ),
            "eu.anthropic.claude-sonnet-4-5-v1:0"
        );
    }

    #[test]
    fn apply_prefix_non_bedrock_unchanged() {
        assert_eq!(
            apply_bedrock_region_prefix("claude-sonnet-4-5-20250929", BedrockRegionPrefix::Eu),
            "claude-sonnet-4-5-20250929"
        );
    }

    #[test]
    fn apply_prefix_same_noop() {
        assert_eq!(
            apply_bedrock_region_prefix(
                "us.anthropic.claude-sonnet-4-5-v1:0",
                BedrockRegionPrefix::Us
            ),
            "us.anthropic.claude-sonnet-4-5-v1:0"
        );
    }

    #[test]
    fn find_first_match_works() {
        let profiles = vec![
            "us.anthropic.claude-opus-4-6-v1".to_string(),
            "eu.anthropic.claude-sonnet-4-6".to_string(),
        ];
        assert_eq!(
            find_first_match(&profiles, "claude-opus-4-6"),
            Some("us.anthropic.claude-opus-4-6-v1")
        );
        assert_eq!(find_first_match(&profiles, "nonexistent"), None);
    }

    #[test]
    fn build_bedrock_model_id_dated() {
        let id = build_bedrock_model_id("claude-opus-4-5-20251101", None);
        assert_eq!(id, "anthropic.claude-opus-4-5-20251101-v1:0");
    }

    #[test]
    fn build_bedrock_model_id_undated() {
        let id = build_bedrock_model_id("claude-opus-4-6", None);
        assert_eq!(id, "anthropic.claude-opus-4-6-v1");
    }

    #[test]
    fn build_bedrock_model_id_with_prefix() {
        let id = build_bedrock_model_id("claude-opus-4-6", Some(BedrockRegionPrefix::Eu));
        assert_eq!(id, "eu.anthropic.claude-opus-4-6-v1");
    }

    #[test]
    fn is_bedrock_model_id_check() {
        assert!(is_bedrock_model_id("anthropic.claude-sonnet-4-5-v1:0"));
        assert!(is_bedrock_model_id("us.anthropic.claude-sonnet-4-5-v1:0"));
        assert!(is_bedrock_model_id(
            "arn:aws:bedrock:us-east-1:123:inference-profile/test"
        ));
        assert!(!is_bedrock_model_id("claude-sonnet-4-5-20250929"));
    }

    #[test]
    fn region_prefix_roundtrip() {
        for prefix in [
            BedrockRegionPrefix::Us,
            BedrockRegionPrefix::Eu,
            BedrockRegionPrefix::Apac,
            BedrockRegionPrefix::Global,
        ] {
            assert_eq!(BedrockRegionPrefix::parse(prefix.as_str()), Some(prefix));
        }
    }
}
