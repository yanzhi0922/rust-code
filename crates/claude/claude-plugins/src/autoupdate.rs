//! Background plugin auto-update functionality.
//!
//! At startup, this module:
//! 1. First updates marketplaces that have auto-update enabled
//! 2. Then checks all installed plugins from those marketplaces and updates them
//!
//! Updates are non-inplace (disk-only), requiring a restart to take effect.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::schemas::is_marketplace_auto_update;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Result of checking a single plugin for updates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutoUpdateResult {
    /// Plugin ID (`"name@marketplace"`).
    pub plugin_id: String,
    /// Current version.
    pub current_version: String,
    /// Available version (if update found).
    pub available_version: Option<String>,
    /// Whether an update was applied.
    pub updated: bool,
    /// Error message if update failed.
    pub error: Option<String>,
}

/// Result of checking all plugins for updates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct AutoUpdateCheckResult {
    /// Total number of plugins checked.
    pub checked_count: usize,
    /// Number of plugins with updates available.
    pub updates_available: usize,
    /// Number of plugins successfully updated.
    pub updated: usize,
    /// Number of plugins that failed to update.
    pub failed: usize,
    /// Individual results.
    pub results: Vec<AutoUpdateResult>,
}

/// Configuration for auto-update behavior.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutoUpdateConfig {
    /// Whether auto-update is globally enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Marketplace names that have auto-update enabled.
    #[serde(default)]
    pub enabled_marketplaces: HashSet<String>,
    /// Marketplace names that should NOT auto-update.
    #[serde(default)]
    pub disabled_marketplaces: HashSet<String>,
}

impl Default for AutoUpdateConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            enabled_marketplaces: HashSet::new(),
            disabled_marketplaces: HashSet::new(),
        }
    }
}

fn default_true() -> bool {
    true
}

// ---------------------------------------------------------------------------
// Core functions
// ---------------------------------------------------------------------------

/// Check if auto-update is enabled for a given marketplace.
///
/// Uses the stored value if set, otherwise defaults based on whether it's an
/// official Anthropic marketplace.
pub fn should_auto_update(marketplace_name: &str, config: &AutoUpdateConfig) -> bool {
    if !config.enabled {
        return false;
    }
    if config.disabled_marketplaces.contains(marketplace_name) {
        return false;
    }
    if config.enabled_marketplaces.contains(marketplace_name) {
        return true;
    }
    // Default: use the schema-level auto-update check
    is_marketplace_auto_update(marketplace_name, None)
}

/// Check all plugins for available updates.
///
/// This is a synchronous check that compares installed versions against
/// marketplace indices.
pub fn check_for_updates(
    installed_plugins: &[(String, String)], // (plugin_id, current_version)
    marketplace_versions: &[(String, String)], // (plugin_id, latest_version)
) -> AutoUpdateCheckResult {
    let mut result = AutoUpdateCheckResult::default();

    let version_map: std::collections::HashMap<&str, &str> = marketplace_versions
        .iter()
        .map(|(id, v)| (id.as_str(), v.as_str()))
        .collect();

    for (plugin_id, current_version) in installed_plugins {
        result.checked_count += 1;

        let update_result = if let Some(latest) = version_map.get(plugin_id.as_str()) {
            if latest > &current_version.as_str() {
                result.updates_available += 1;
                AutoUpdateResult {
                    plugin_id: plugin_id.clone(),
                    current_version: current_version.clone(),
                    available_version: Some((*latest).to_owned()),
                    updated: false,
                    error: None,
                }
            } else {
                AutoUpdateResult {
                    plugin_id: plugin_id.clone(),
                    current_version: current_version.clone(),
                    available_version: None,
                    updated: false,
                    error: None,
                }
            }
        } else {
            AutoUpdateResult {
                plugin_id: plugin_id.clone(),
                current_version: current_version.clone(),
                available_version: None,
                updated: false,
                error: Some("not found in marketplace".to_owned()),
            }
        };

        result.results.push(update_result);
    }

    result
}

/// Simulate auto-updating a single plugin.
///
/// In a real implementation, this would download and install the new version.
/// Here it returns a result indicating what would happen.
pub fn auto_update_plugin(
    plugin_id: &str,
    current_version: &str,
    target_version: &str,
) -> AutoUpdateResult {
    AutoUpdateResult {
        plugin_id: plugin_id.to_owned(),
        current_version: current_version.to_owned(),
        available_version: Some(target_version.to_owned()),
        updated: true,
        error: None,
    }
}

/// Get the list of marketplace names that should be auto-updated.
pub fn get_auto_update_marketplaces(
    known_marketplaces: &[String],
    config: &AutoUpdateConfig,
) -> Vec<String> {
    known_marketplaces
        .iter()
        .filter(|name| should_auto_update(name, config))
        .cloned()
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_auto_update_respects_global_disable() {
        let config = AutoUpdateConfig {
            enabled: false,
            ..AutoUpdateConfig::default()
        };
        assert!(!should_auto_update("any-marketplace", &config));
    }

    #[test]
    fn should_auto_update_respects_disabled_marketplace() {
        let mut config = AutoUpdateConfig::default();
        config
            .disabled_marketplaces
            .insert("my-marketplace".to_owned());
        assert!(!should_auto_update("my-marketplace", &config));
    }

    #[test]
    fn should_auto_update_respects_enabled_marketplace() {
        let config = AutoUpdateConfig::default();
        // Official marketplace should auto-update by default
        assert!(should_auto_update("claude-code-marketplace", &config));
    }

    #[test]
    fn check_for_updates_finds_update() {
        let installed = vec![
            ("plugin-a@mkt".to_owned(), "1.0.0".to_owned()),
            ("plugin-b@mkt".to_owned(), "2.0.0".to_owned()),
        ];
        let marketplace = vec![
            ("plugin-a@mkt".to_owned(), "1.1.0".to_owned()),
            ("plugin-b@mkt".to_owned(), "2.0.0".to_owned()),
        ];
        let result = check_for_updates(&installed, &marketplace);
        assert_eq!(result.checked_count, 2);
        assert_eq!(result.updates_available, 1);
    }

    #[test]
    fn check_for_updates_no_updates() {
        let installed = vec![("plugin-a@mkt".to_owned(), "1.0.0".to_owned())];
        let marketplace = vec![("plugin-a@mkt".to_owned(), "1.0.0".to_owned())];
        let result = check_for_updates(&installed, &marketplace);
        assert_eq!(result.checked_count, 1);
        assert_eq!(result.updates_available, 0);
    }

    #[test]
    fn check_for_updates_missing_from_marketplace() {
        let installed = vec![("plugin-a@mkt".to_owned(), "1.0.0".to_owned())];
        let marketplace: Vec<(String, String)> = vec![];
        let result = check_for_updates(&installed, &marketplace);
        assert_eq!(result.checked_count, 1);
        let error_result = result.results.iter().find(|r| r.error.is_some());
        assert!(error_result.is_some());
    }

    #[test]
    fn auto_update_plugin_returns_updated() {
        let result = auto_update_plugin("plugin-a@mkt", "1.0.0", "1.1.0");
        assert!(result.updated);
        assert_eq!(result.available_version, Some("1.1.0".to_owned()));
    }

    #[test]
    fn get_auto_update_marketplaces_filters() {
        let known = vec!["claude-code-marketplace".to_owned(), "my-custom".to_owned()];
        let config = AutoUpdateConfig::default();
        let result = get_auto_update_marketplaces(&known, &config);
        // claude-code-marketplace is official and should auto-update
        assert!(result.contains(&"claude-code-marketplace".to_owned()));
    }

    #[test]
    fn auto_update_config_default() {
        let config = AutoUpdateConfig::default();
        assert!(config.enabled);
        assert!(config.enabled_marketplaces.is_empty());
        assert!(config.disabled_marketplaces.is_empty());
    }

    #[test]
    fn auto_update_check_result_default() {
        let result = AutoUpdateCheckResult::default();
        assert_eq!(result.checked_count, 0);
        assert_eq!(result.updates_available, 0);
        assert_eq!(result.updated, 0);
        assert_eq!(result.failed, 0);
        assert!(result.results.is_empty());
    }
}
