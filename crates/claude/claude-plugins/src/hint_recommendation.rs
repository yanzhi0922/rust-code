//! Plugin hint recommendation system.
//!
//! Suggests plugins based on context and generates installation hint text.
//! Companion to LSP recommendations: where LSP recommendations are triggered
//! by file edits, plugin hints are triggered by CLI/SDK usage patterns.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::identifier::PluginIdentifier;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A plugin recommendation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginRecommendation {
    /// Plugin ID (`"name@marketplace"`).
    pub plugin_id: String,
    /// Plugin display name.
    pub plugin_name: String,
    /// Marketplace name.
    pub marketplace_name: String,
    /// Plugin description.
    pub plugin_description: Option<String>,
    /// Source command that triggered the recommendation.
    pub source_command: String,
    /// Relevance score (higher = more relevant).
    #[serde(default)]
    pub relevance: u32,
}

/// Hint recommendation state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HintRecommendationState {
    /// Set of plugin IDs already shown.
    pub shown_plugins: HashSet<String>,
    /// Whether hints are disabled.
    pub disabled: bool,
    /// Maximum number of plugins to show.
    pub max_shown: usize,
}

impl Default for HintRecommendationState {
    fn default() -> Self {
        Self {
            shown_plugins: HashSet::new(),
            disabled: false,
            max_shown: 100,
        }
    }
}

// ---------------------------------------------------------------------------
// Core functions
// ---------------------------------------------------------------------------

/// Get plugin recommendations based on context.
///
/// Filters available plugins against already-shown and installed plugins,
/// returning recommendations sorted by relevance.
pub fn get_plugin_recommendations(
    available_plugins: &[PluginRecommendation],
    state: &HintRecommendationState,
    installed_plugins: &HashSet<String>,
) -> Vec<PluginRecommendation> {
    if state.disabled {
        return Vec::new();
    }

    if state.shown_plugins.len() >= state.max_shown {
        return Vec::new();
    }

    let mut recommendations: Vec<PluginRecommendation> = available_plugins
        .iter()
        .filter(|rec| {
            // Skip already installed
            !installed_plugins.contains(&rec.plugin_id)
                // Skip already shown
                && !state.shown_plugins.contains(&rec.plugin_id)
                // Only recommend from official marketplaces
                && is_official_marketplace(&rec.marketplace_name)
        })
        .cloned()
        .collect();

    // Sort by relevance (descending)
    recommendations.sort_by(|a, b| b.relevance.cmp(&a.relevance));

    recommendations
}

/// Generate installation hint text for a plugin recommendation.
///
/// Returns a human-readable string suggesting the user install the plugin.
pub fn generate_install_hint(rec: &PluginRecommendation) -> String {
    let description = rec
        .plugin_description
        .as_deref()
        .unwrap_or("a useful plugin");

    format!(
        "💡 Plugin available: {} — {}. Install with: /plugin install {}",
        rec.plugin_name, description, rec.plugin_id
    )
}

/// Check if a marketplace name is official.
///
/// For v1, only official Anthropic marketplaces are used for recommendations.
pub fn is_official_marketplace(name: &str) -> bool {
    crate::schemas::ALLOWED_OFFICIAL_MARKETPLACE_NAMES.contains(&name.to_lowercase().as_str())
}

/// Record that a plugin hint has been shown.
pub fn mark_hint_shown(state: &mut HintRecommendationState, plugin_id: &str) {
    state.shown_plugins.insert(plugin_id.to_owned());
}

/// Check if a plugin hint should be shown.
pub fn should_show_hint(
    state: &HintRecommendationState,
    plugin_id: &str,
    installed_plugins: &HashSet<String>,
) -> bool {
    if state.disabled {
        return false;
    }
    if state.shown_plugins.len() >= state.max_shown {
        return false;
    }
    if state.shown_plugins.contains(plugin_id) {
        return false;
    }
    if installed_plugins.contains(plugin_id) {
        return false;
    }

    // Validate plugin ID format
    let parsed = PluginIdentifier::parse(plugin_id);
    if parsed.name.is_empty() {
        return false;
    }
    if parsed.marketplace.is_none() {
        return false;
    }

    // Only official marketplaces
    if let Some(ref mkt) = parsed.marketplace
        && !is_official_marketplace(mkt)
    {
        return false;
    }

    true
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_rec(id: &str, name: &str, mkt: &str) -> PluginRecommendation {
        PluginRecommendation {
            plugin_id: id.to_owned(),
            plugin_name: name.to_owned(),
            marketplace_name: mkt.to_owned(),
            plugin_description: Some("A test plugin".to_owned()),
            source_command: "test".to_owned(),
            relevance: 1,
        }
    }

    #[test]
    fn get_recommendations_filters_installed() {
        let recs = vec![make_rec("a@mkt", "a", "mkt")];
        let state = HintRecommendationState::default();
        let mut installed = HashSet::new();
        installed.insert("a@mkt".to_owned());

        let result = get_plugin_recommendations(&recs, &state, &installed);
        assert!(result.is_empty());
    }

    #[test]
    fn get_recommendations_filters_shown() {
        let recs = vec![make_rec("a@mkt", "a", "mkt")];
        let mut state = HintRecommendationState::default();
        state.shown_plugins.insert("a@mkt".to_owned());

        let result = get_plugin_recommendations(&recs, &state, &HashSet::new());
        assert!(result.is_empty());
    }

    #[test]
    fn get_recommendations_disabled() {
        let recs = vec![make_rec("a@mkt", "a", "mkt")];
        let state = HintRecommendationState {
            disabled: true,
            ..HintRecommendationState::default()
        };

        let result = get_plugin_recommendations(&recs, &state, &HashSet::new());
        assert!(result.is_empty());
    }

    #[test]
    fn generate_install_hint_basic() {
        let rec = make_rec(
            "test-plugin@claude-code-marketplace",
            "Test",
            "claude-code-marketplace",
        );
        let hint = generate_install_hint(&rec);
        assert!(hint.contains("Test"));
        assert!(hint.contains("test-plugin@claude-code-marketplace"));
    }

    #[test]
    fn is_official_marketplace_known() {
        assert!(is_official_marketplace("claude-code-marketplace"));
        assert!(is_official_marketplace("anthropic-marketplace"));
    }

    #[test]
    fn is_official_marketplace_unknown() {
        assert!(!is_official_marketplace("my-custom-marketplace"));
    }

    #[test]
    fn mark_hint_shown_adds_to_set() {
        let mut state = HintRecommendationState::default();
        mark_hint_shown(&mut state, "test@mkt");
        assert!(state.shown_plugins.contains("test@mkt"));
    }

    #[test]
    fn should_show_hint_basic() {
        let state = HintRecommendationState::default();
        assert!(should_show_hint(
            &state,
            "test@claude-code-marketplace",
            &HashSet::new(),
        ));
    }

    #[test]
    fn should_show_hint_already_shown() {
        let mut state = HintRecommendationState::default();
        state
            .shown_plugins
            .insert("test@claude-code-marketplace".to_owned());
        assert!(!should_show_hint(
            &state,
            "test@claude-code-marketplace",
            &HashSet::new(),
        ));
    }

    #[test]
    fn should_show_hint_installed() {
        let state = HintRecommendationState::default();
        let mut installed = HashSet::new();
        installed.insert("test@claude-code-marketplace".to_owned());
        assert!(!should_show_hint(
            &state,
            "test@claude-code-marketplace",
            &installed,
        ));
    }

    #[test]
    fn should_show_hint_non_official() {
        let state = HintRecommendationState::default();
        assert!(!should_show_hint(
            &state,
            "test@random-marketplace",
            &HashSet::new(),
        ));
    }

    #[test]
    fn hint_recommendation_state_default() {
        let state = HintRecommendationState::default();
        assert!(state.shown_plugins.is_empty());
        assert!(!state.disabled);
        assert_eq!(state.max_shown, 100);
    }
}
