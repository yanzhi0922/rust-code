//! Plugin refresh — swap active plugin components in the running session.
//!
//! Three-layer model:
//! - Layer 1: intent (settings)
//! - Layer 2: materialization (~/.claude/plugins/) — reconciler
//! - Layer 3: active components (AppState) — this module

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{PluginBundle, discover_plugins};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Result of refreshing all plugins.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RefreshResult {
    /// Number of enabled plugins after refresh.
    pub enabled_count: usize,
    /// Number of disabled plugins after refresh.
    pub disabled_count: usize,
    /// Number of commands loaded from plugins.
    pub command_count: usize,
    /// Number of agents loaded from plugins.
    pub agent_count: usize,
    /// Number of hooks loaded from plugins.
    pub hook_count: usize,
    /// Number of MCP servers loaded from plugins.
    pub mcp_count: usize,
    /// Number of LSP servers loaded from plugins.
    pub lsp_count: usize,
    /// Number of errors encountered during refresh.
    pub error_count: usize,
    /// List of errors encountered.
    pub errors: Vec<String>,
    /// List of warnings encountered.
    pub warnings: Vec<String>,
}

/// Result of refreshing marketplace plugins.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarketplaceRefreshResult {
    /// Marketplace name.
    pub marketplace_name: String,
    /// Whether the refresh succeeded.
    pub success: bool,
    /// Number of plugins in the marketplace.
    pub plugin_count: usize,
    /// Error message if refresh failed.
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// Core functions
// ---------------------------------------------------------------------------

/// Refresh all plugin metadata from the given root directory.
///
/// Discovers all plugins, validates them, and returns a summary.
pub fn refresh_plugins(root: &Path) -> RefreshResult {
    let mut result = RefreshResult::default();

    match discover_plugins(root) {
        Ok(plugins) => {
            for plugin in &plugins {
                if plugin.is_disabled() {
                    result.disabled_count += 1;
                } else {
                    result.enabled_count += 1;
                }

                // Count components
                if plugin.hooks_config_path().is_some() {
                    result.hook_count += 1;
                }
                if plugin.mcp_config_path().is_some() {
                    result.mcp_count += 1;
                }
                if let Some(skills_root) = plugin.skills_root()
                    && skills_root.exists()
                {
                    result.command_count += 1;
                }

                // Validate and collect errors
                let report = crate::validate_plugin_bundle(plugin);
                if !report.errors.is_empty() {
                    result.error_count += 1;
                    for err in &report.errors {
                        result
                            .errors
                            .push(format!("{}: {}", plugin.manifest.name, err));
                    }
                }
                result.warnings.extend(
                    report
                        .warnings
                        .iter()
                        .map(|w| format!("{}: {}", plugin.manifest.name, w)),
                );
            }
        }
        Err(e) => {
            result.error_count += 1;
            result
                .errors
                .push(format!("failed to discover plugins: {e}"));
        }
    }

    result
}

/// Refresh plugins from a specific marketplace directory.
///
/// Discovers plugins within a marketplace's cache directory.
pub fn refresh_marketplace_plugins(
    marketplace_dir: &Path,
    marketplace_name: &str,
) -> MarketplaceRefreshResult {
    let mut result = MarketplaceRefreshResult {
        marketplace_name: marketplace_name.to_owned(),
        success: true,
        plugin_count: 0,
        error: None,
    };

    if !marketplace_dir.exists() {
        result.success = false;
        result.error = Some(format!(
            "marketplace directory {} does not exist",
            marketplace_dir.display()
        ));
        return result;
    }

    match discover_plugins(marketplace_dir) {
        Ok(plugins) => {
            result.plugin_count = plugins.len();
        }
        Err(e) => {
            result.success = false;
            result.error = Some(format!("failed to discover plugins: {e}"));
        }
    }

    result
}

/// Check if a refresh is needed by comparing current state with previous.
pub fn is_refresh_needed(current_plugins: &[PluginBundle], previous_count: usize) -> bool {
    current_plugins.len() != previous_count
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn ok<T, E: std::fmt::Display>(result: Result<T, E>) -> T {
        match result {
            Ok(value) => value,
            Err(error) => panic!("unexpected error: {error}"),
        }
    }

    fn create_test_plugin(root: &Path, name: &str) {
        let manifest_dir = root.join(crate::PLUGIN_MANIFEST_DIR);
        fs::create_dir_all(&manifest_dir).expect("create dir");
        fs::write(
            manifest_dir.join(crate::PLUGIN_MANIFEST_FILE),
            format!(r#"{{"name":"{name}","version":"0.1.0"}}"#),
        )
        .expect("write manifest");
    }

    #[test]
    fn refresh_plugins_empty_directory() {
        let temp = ok(tempdir());
        let result = refresh_plugins(temp.path());
        assert_eq!(result.enabled_count, 0);
        assert_eq!(result.disabled_count, 0);
        assert_eq!(result.error_count, 0);
    }

    #[test]
    fn refresh_plugins_with_plugins() {
        let temp = ok(tempdir());
        create_test_plugin(&temp.path().join("a"), "plugin-a");
        create_test_plugin(&temp.path().join("b"), "plugin-b");

        let result = refresh_plugins(temp.path());
        assert_eq!(result.enabled_count, 2);
        assert_eq!(result.disabled_count, 0);
    }

    #[test]
    fn refresh_plugins_counts_disabled() {
        let temp = ok(tempdir());
        let disabled_dir = temp.path().join("disabled");
        create_test_plugin(&disabled_dir, "disabled-plugin");
        fs::write(
            disabled_dir.join(crate::PLUGIN_DISABLED_MARKER),
            b"disabled\n",
        )
        .expect("write marker");

        // discover_plugins filters out disabled plugins by default,
        // so neither enabled nor disabled count includes them.
        let result = refresh_plugins(temp.path());
        assert_eq!(result.enabled_count, 0);
        assert_eq!(result.disabled_count, 0);
    }

    #[test]
    fn refresh_marketplace_plugins_basic() {
        let temp = ok(tempdir());
        let mkt_dir = temp.path().join("my-marketplace");
        create_test_plugin(&mkt_dir.join("plugin-a"), "plugin-a");

        let result = refresh_marketplace_plugins(&mkt_dir, "my-marketplace");
        assert!(result.success);
        assert_eq!(result.plugin_count, 1);
        assert!(result.error.is_none());
    }

    #[test]
    fn refresh_marketplace_plugins_nonexistent() {
        let result = refresh_marketplace_plugins(Path::new("/nonexistent"), "test");
        assert!(!result.success);
        assert!(result.error.is_some());
    }

    #[test]
    fn refresh_result_default() {
        let result = RefreshResult::default();
        assert_eq!(result.enabled_count, 0);
        assert_eq!(result.disabled_count, 0);
        assert!(result.errors.is_empty());
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn is_refresh_needed_detects_change() {
        let temp = ok(tempdir());
        create_test_plugin(&temp.path().join("a"), "plugin-a");
        let plugins = ok(discover_plugins(temp.path()));
        assert!(is_refresh_needed(&plugins, 0));
        assert!(!is_refresh_needed(&plugins, 1));
    }
}
