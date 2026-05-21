//! Model allowlist (enterprise model-access control).
//!
//! When an `available_models` list is configured, only models matching an
//! entry in the list are permitted.  Three matching tiers are supported:
//!
//! 1. **Family aliases** (`"opus"`, `"sonnet"`, `"haiku"`) — wildcard for
//!    the entire family, *unless* more specific entries for that family also
//!    exist (e.g. `"opus-4-5"`), in which case only the specific entries
//!    apply.
//! 2. **Version prefixes** (`"opus-4-5"`, `"claude-opus-4-5"`) — any build
//!    of that version.
//! 3. **Full model IDs** (`"claude-opus-4-5-20251101"`) — exact match only.

use crate::aliases::{is_model_alias, is_model_family_alias};
use crate::model::parse_user_specified_model;

/// Returns `true` when `model` is permitted by the given `allowlist`.
///
/// An empty / `None` allowlist means *all* models are allowed.
pub fn is_model_allowed(model: &str, allowlist: Option<&[String]>) -> bool {
    let Some(list) = allowlist else {
        return true; // No restrictions.
    };
    if list.is_empty() {
        return false; // Empty allowlist blocks everything.
    }

    let resolved = parse_user_specified_model(model);
    let normalized = resolved.to_lowercase();
    let normalized_list: Vec<String> = list.iter().map(|s| s.trim().to_lowercase()).collect();
    let normalized_refs: Vec<&str> = normalized_list.iter().map(|s| s.as_str()).collect();

    // 1. Direct match (but skip family aliases that have been narrowed).
    if normalized_refs.contains(&normalized.as_str())
        && (!is_model_family_alias(&normalized)
            || !family_has_specific_entries(&normalized, &normalized_refs))
    {
        return true;
    }

    // 2. Family-level aliases in the allowlist match any model in that family
    //    (only if no more specific entries exist for that family).
    for entry in &normalized_refs {
        if is_model_family_alias(entry)
            && !family_has_specific_entries(entry, &normalized_refs)
            && model_belongs_to_family(&normalized, entry)
        {
            return true;
        }
    }

    // 3. Bidirectional alias resolution.
    if is_model_alias(&normalized) {
        let resolved_model = parse_user_specified_model(&normalized).to_lowercase();
        if normalized_refs.contains(&resolved_model.as_str()) {
            return true;
        }
    }

    for entry in &normalized_refs {
        if !is_model_family_alias(entry) && is_model_alias(entry) {
            let resolved_entry = parse_user_specified_model(entry).to_lowercase();
            if resolved_entry == normalized {
                return true;
            }
        }
    }

    // 4. Version-prefix matching.
    for entry in &normalized_refs {
        if !is_model_family_alias(entry)
            && !is_model_alias(entry)
            && model_matches_version_prefix(&normalized, entry)
        {
            return true;
        }
    }

    false
}

/// Check if a model belongs to a family by checking substring containment,
/// with alias resolution.
fn model_belongs_to_family(model: &str, family: &str) -> bool {
    if model.contains(family) {
        return true;
    }
    if is_model_alias(model) {
        let resolved = parse_user_specified_model(model).to_lowercase();
        return resolved.contains(family);
    }
    false
}

/// Check if `prefix` matches `model_name` at a segment boundary.
fn prefix_matches_model(model_name: &str, prefix: &str) -> bool {
    if !model_name.starts_with(prefix) {
        return false;
    }
    model_name.len() == prefix.len() || model_name.as_bytes()[prefix.len()] == b'-'
}

/// Check if a model matches a version-prefix entry in the allowlist.
fn model_matches_version_prefix(model: &str, entry: &str) -> bool {
    let resolved_model = if is_model_alias(model) {
        parse_user_specified_model(model).to_lowercase()
    } else {
        model.to_owned()
    };

    if prefix_matches_model(&resolved_model, entry) {
        return true;
    }
    // Try with "claude-" prefix.
    if !entry.starts_with("claude-") {
        let prefixed = format!("claude-{entry}");
        if prefix_matches_model(&resolved_model, &prefixed) {
            return true;
        }
    }
    false
}

/// Check if a family alias is narrowed by more specific entries in the
/// allowlist.
fn family_has_specific_entries(family: &str, allowlist: &[&str]) -> bool {
    for entry in allowlist {
        if is_model_family_alias(entry) {
            continue;
        }
        let Some(idx) = entry.find(family) else {
            continue;
        };
        let after = idx + family.len();
        if after == entry.len() || entry.as_bytes()[after] == b'-' {
            return true;
        }
    }
    false
}

/// The default built-in allowlist — all known model IDs.
pub fn default_allowlist() -> Vec<String> {
    vec![
        "claude-opus-4-7".into(),
        "claude-opus-4-6".into(),
        "claude-opus-4-5-20251101".into(),
        "claude-opus-4-1-20250805".into(),
        "claude-opus-4-20250514".into(),
        "claude-sonnet-4-6".into(),
        "claude-sonnet-4-5-20250929".into(),
        "claude-sonnet-4-20250514".into(),
        "claude-3-7-sonnet-20250219".into(),
        "claude-3-5-sonnet-20241022".into(),
        "claude-haiku-4-5-20251001".into(),
        "claude-3-5-haiku-20241022".into(),
        // Aliases
        "sonnet".into(),
        "opus".into(),
        "haiku".into(),
        "best".into(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_allowlist_allows_all() {
        assert!(is_model_allowed("claude-opus-4-6", None));
        assert!(is_model_allowed("anything", None));
    }

    #[test]
    fn empty_allowlist_blocks_all() {
        let list: Vec<String> = vec![];
        assert!(!is_model_allowed("claude-opus-4-6", Some(&list)));
    }

    #[test]
    fn exact_match() {
        let list = vec!["claude-opus-4-6".into()];
        assert!(is_model_allowed("claude-opus-4-6", Some(&list)));
        assert!(!is_model_allowed("claude-sonnet-4-6", Some(&list)));
    }

    #[test]
    fn family_alias_wildcard() {
        let list = vec!["opus".into()];
        assert!(is_model_allowed("claude-opus-4-6", Some(&list)));
    }

    #[test]
    fn family_narrowed_by_specific_entry() {
        let list = vec!["opus".into(), "opus-4-5".into()];
        // "opus" alone should NOT match because "opus-4-5" narrows it.
        // But "claude-opus-4-5-20251101" should match via prefix.
        assert!(is_model_allowed("claude-opus-4-5-20251101", Some(&list),));
    }

    #[test]
    fn version_prefix_match() {
        let list = vec!["claude-opus-4-5".into()];
        assert!(is_model_allowed("claude-opus-4-5-20251101", Some(&list),));
    }

    #[test]
    fn default_allowlist_is_not_empty() {
        let list = default_allowlist();
        assert!(!list.is_empty());
    }
}
