//! Model alias definitions and resolution.
//!
//! Maps user-facing short names (e.g. `"sonnet"`, `"opus"`, `"haiku"`) to
//! concrete model IDs.  Family aliases (`"sonnet"`, `"opus"`, `"haiku"`)
//! also serve as wildcards in the [`crate::allowlist`] module.

use std::sync::LazyLock;

/// Alias name → resolved model ID (first-party canonical form).
///
/// The map is ordered by specificity: longer aliases first so that a
/// `BTreeMap` lookup prefers the most specific match.
pub static MODEL_ALIASES: LazyLock<Vec<(&str, &str)>> = LazyLock::new(|| {
    vec![
        // Family aliases — resolve to the current default for each family.
        ("sonnet", "claude-sonnet-4-6"),
        ("opus", "claude-opus-4-7"),
        ("haiku", "claude-haiku-4-5-20251001"),
        // Convenience aliases
        ("best", "claude-opus-4-7"),
        // 1M-tagged aliases (the `[1m]` suffix is preserved by the caller)
        ("sonnet[1m]", "claude-sonnet-4-6"),
        ("opus[1m]", "claude-opus-4-7"),
        // Composite alias: opus in plan mode, sonnet otherwise
        ("opusplan", "claude-sonnet-4-6"),
    ]
});

/// Bare model family aliases that act as wildcards in the allowlist.
pub const MODEL_FAMILY_ALIASES: &[&str] = &["sonnet", "opus", "haiku"];

/// Returns `true` if `name` is a recognised model alias.
pub fn is_model_alias(name: &str) -> bool {
    MODEL_ALIASES.iter().any(|(alias, _)| *alias == name)
}

/// Returns `true` if `name` is a model family alias (`sonnet`, `opus`, `haiku`).
pub fn is_model_family_alias(name: &str) -> bool {
    MODEL_FAMILY_ALIASES.contains(&name)
}

/// Resolve an alias to its canonical model ID.
///
/// Returns `None` when `name` is not a known alias.
pub fn resolve_alias(name: &str) -> Option<&'static str> {
    MODEL_ALIASES
        .iter()
        .find(|(alias, _)| *alias == name)
        .map(|(_, id)| *id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn family_aliases() {
        assert!(is_model_family_alias("sonnet"));
        assert!(is_model_family_alias("opus"));
        assert!(is_model_family_alias("haiku"));
        assert!(!is_model_family_alias("best"));
    }

    #[test]
    fn alias_resolution() {
        assert_eq!(resolve_alias("sonnet"), Some("claude-sonnet-4-6"));
        assert_eq!(resolve_alias("opus"), Some("claude-opus-4-7"));
        assert_eq!(resolve_alias("haiku"), Some("claude-haiku-4-5-20251001"));
        assert_eq!(resolve_alias("best"), Some("claude-opus-4-7"));
        assert_eq!(resolve_alias("unknown"), None);
    }

    #[test]
    fn is_alias_check() {
        assert!(is_model_alias("sonnet"));
        assert!(is_model_alias("opus[1m]"));
        assert!(!is_model_alias("claude-sonnet-4-6"));
    }
}
