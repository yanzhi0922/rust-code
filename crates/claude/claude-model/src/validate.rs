//! Model ID validation.
//!
//! Validates that a model ID is well-formed and (optionally) that it is a
//! known model.  Full API-level validation (probing the endpoint) is out of
//! scope for this crate — that belongs in the provider layer.

use thiserror::Error;

/// Validation error kinds.
#[derive(Debug, Error)]
pub enum ValidationError {
    /// The model name is empty or whitespace-only.
    #[error("Model name cannot be empty")]
    Empty,
    /// The model is not in the configured allowlist.
    #[error("Model '{0}' is not in the list of available models")]
    NotAllowed(String),
    /// The model ID format is invalid.
    #[error("Invalid model ID format: '{0}'")]
    InvalidFormat(String),
}

/// Validate a model ID for basic correctness.
///
/// Checks:
/// 1. Non-empty after trimming.
/// 2. Does not contain obviously invalid characters.
/// 3. (Optional) Is present in the allowlist.
pub fn validate_model_id(id: &str, allowlist: Option<&[String]>) -> Result<(), ValidationError> {
    let trimmed = id.trim();

    if trimmed.is_empty() {
        return Err(ValidationError::Empty);
    }

    // Reject control characters and whitespace inside the ID.
    if trimmed.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(ValidationError::InvalidFormat(trimmed.to_owned()));
    }

    // Check allowlist if configured.
    if let Some(list) = allowlist
        && !crate::allowlist::is_model_allowed(trimmed, Some(list))
    {
        return Err(ValidationError::NotAllowed(trimmed.to_owned()));
    }

    Ok(())
}

/// Returns a human-readable display name for a known model, or `None` for
/// unknown / custom models.
pub fn get_public_model_display_name(model_id: &str) -> Option<String> {
    let lower = model_id.to_lowercase();
    let lower = lower.strip_suffix("[1m]").unwrap_or(&lower).trim();

    // Order matters: more specific patterns first.
    if lower.contains("claude-opus-4-7") {
        return Some("Opus 4.7".into());
    }
    if lower.contains("claude-opus-4-6") {
        return Some("Opus 4.6".into());
    }
    if lower.contains("claude-opus-4-5") {
        return Some("Opus 4.5".into());
    }
    if lower.contains("claude-opus-4-1") {
        return Some("Opus 4.1".into());
    }
    if lower.contains("claude-opus-4") && !lower.contains("claude-opus-4-") {
        return Some("Opus 4".into());
    }
    if lower.contains("claude-sonnet-4-6") {
        return Some("Sonnet 4.6".into());
    }
    if lower.contains("claude-sonnet-4-5") {
        return Some("Sonnet 4.5".into());
    }
    if lower.contains("claude-sonnet-4") && !lower.contains("claude-sonnet-4-") {
        return Some("Sonnet 4".into());
    }
    if lower.contains("claude-3-7-sonnet") {
        return Some("Claude 3.7 Sonnet".into());
    }
    if lower.contains("claude-3-5-sonnet") {
        return Some("Claude 3.5 Sonnet".into());
    }
    if lower.contains("claude-haiku-4-5") {
        return Some("Haiku 4.5".into());
    }
    if lower.contains("claude-3-5-haiku") {
        return Some("Claude 3.5 Haiku".into());
    }
    None
}

/// Returns a canonical short name for a model ID by stripping date suffixes
/// and provider-specific decorations.
pub fn get_canonical_name(model_id: &str) -> String {
    let lower = model_id.to_lowercase();

    // Order matters: more specific patterns first.
    if lower.contains("claude-opus-4-7") {
        return "claude-opus-4-7".into();
    }
    if lower.contains("claude-opus-4-6") {
        return "claude-opus-4-6".into();
    }
    if lower.contains("claude-opus-4-5") {
        return "claude-opus-4-5".into();
    }
    if lower.contains("claude-opus-4-1") {
        return "claude-opus-4-1".into();
    }
    if lower.contains("claude-opus-4") {
        return "claude-opus-4".into();
    }
    if lower.contains("claude-sonnet-4-6") {
        return "claude-sonnet-4-6".into();
    }
    if lower.contains("claude-sonnet-4-5") {
        return "claude-sonnet-4-5".into();
    }
    if lower.contains("claude-sonnet-4") {
        return "claude-sonnet-4".into();
    }
    if lower.contains("claude-haiku-4-5") {
        return "claude-haiku-4-5".into();
    }
    if lower.contains("claude-3-7-sonnet") {
        return "claude-3-7-sonnet".into();
    }
    if lower.contains("claude-3-5-sonnet") {
        return "claude-3-5-sonnet".into();
    }
    if lower.contains("claude-3-5-haiku") {
        return "claude-3-5-haiku".into();
    }
    if lower.contains("claude-3-opus") {
        return "claude-3-opus".into();
    }
    if lower.contains("claude-3-sonnet") {
        return "claude-3-sonnet".into();
    }
    if lower.contains("claude-3-haiku") {
        return "claude-3-haiku".into();
    }
    lower
}

/// Strip `[1m]` / `[2m]` suffixes from a model string for API calls.
pub fn normalize_model_string_for_api(model: &str) -> String {
    let lower = model.to_lowercase();
    if lower.ends_with("[1m]") || lower.ends_with("[2m]") {
        model[..model.len() - 4].to_owned()
    } else {
        model.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_model_rejected() {
        assert!(matches!(
            validate_model_id("", None),
            Err(ValidationError::Empty)
        ));
        assert!(matches!(
            validate_model_id("   ", None),
            Err(ValidationError::Empty)
        ));
    }

    #[test]
    fn valid_model_accepted() {
        assert!(validate_model_id("claude-opus-4-6", None).is_ok());
        assert!(validate_model_id("my-custom-model", None).is_ok());
    }

    #[test]
    fn whitespace_in_id_rejected() {
        assert!(matches!(
            validate_model_id("claude opus", None),
            Err(ValidationError::InvalidFormat(_))
        ));
    }

    #[test]
    fn allowlist_enforced() {
        let list = vec!["claude-opus-4-6".into()];
        assert!(validate_model_id("claude-opus-4-6", Some(&list)).is_ok());
        assert!(matches!(
            validate_model_id("claude-sonnet-4-6", Some(&list)),
            Err(ValidationError::NotAllowed(_))
        ));
    }

    #[test]
    fn display_names() {
        assert_eq!(
            get_public_model_display_name("claude-opus-4-7"),
            Some("Opus 4.7".into())
        );
        assert_eq!(
            get_public_model_display_name("claude-sonnet-4-5-20250929"),
            Some("Sonnet 4.5".into())
        );
        assert_eq!(get_public_model_display_name("unknown-model"), None);
    }

    #[test]
    fn canonical_names() {
        assert_eq!(get_canonical_name("claude-opus-4-7"), "claude-opus-4-7");
        assert_eq!(
            get_canonical_name("claude-opus-4-5-20251101"),
            "claude-opus-4-5"
        );
    }

    #[test]
    fn normalize_strips_1m() {
        assert_eq!(
            normalize_model_string_for_api("claude-opus-4-6[1m]"),
            "claude-opus-4-6"
        );
        assert_eq!(
            normalize_model_string_for_api("claude-opus-4-6"),
            "claude-opus-4-6"
        );
    }
}
