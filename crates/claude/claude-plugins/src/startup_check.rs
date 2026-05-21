//! Plugin startup checks.
//!
//! Runs health checks on all installed plugins at startup, verifying
//! manifests, dependencies, and configurations.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{PluginBundle, PluginValidationReport, discover_plugins, validate_plugin_bundle};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Result of running all plugin startup checks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginStartupCheckResult {
    /// Total number of plugins checked.
    pub total_plugins: usize,
    /// Number of healthy plugins.
    pub healthy: usize,
    /// Number of plugins with warnings.
    pub warnings: usize,
    /// Number of plugins with errors.
    pub errors: usize,
    /// Individual health check results.
    pub results: Vec<PluginHealthCheck>,
    /// Overall health status.
    pub status: HealthStatus,
}

/// Overall health status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HealthStatus {
    /// All plugins are healthy.
    Healthy,
    /// Some plugins have warnings.
    Warning,
    /// Some plugins have errors.
    Error,
}

/// Result of checking an individual plugin's health.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginHealthCheck {
    /// Plugin name.
    pub name: String,
    /// Plugin root path.
    pub root: PathBuf,
    /// Whether the plugin is disabled.
    pub is_disabled: bool,
    /// Validation report.
    pub validation: PluginValidationReport,
    /// Health status.
    pub status: HealthStatus,
}

// ---------------------------------------------------------------------------
// Core functions
// ---------------------------------------------------------------------------

/// Run all plugin startup checks.
///
/// Discovers all plugins in the given root directory and validates each one.
pub fn perform_plugin_startup_checks(root: &Path) -> PluginStartupCheckResult {
    let mut result = PluginStartupCheckResult {
        total_plugins: 0,
        healthy: 0,
        warnings: 0,
        errors: 0,
        results: Vec::new(),
        status: HealthStatus::Healthy,
    };

    let plugins = match discover_plugins(root) {
        Ok(p) => p,
        Err(_e) => {
            result.status = HealthStatus::Error;
            return result;
        }
    };

    result.total_plugins = plugins.len();

    for plugin in &plugins {
        let health = check_plugin_health(plugin);
        match health.status {
            HealthStatus::Healthy => result.healthy += 1,
            HealthStatus::Warning => {
                result.warnings += 1;
                result.status = HealthStatus::Warning;
            }
            HealthStatus::Error => {
                result.errors += 1;
                result.status = HealthStatus::Error;
            }
        }
        result.results.push(health);
    }

    result
}

/// Check the health of an individual plugin.
pub fn check_plugin_health(plugin: &PluginBundle) -> PluginHealthCheck {
    let validation = validate_plugin_bundle(plugin);

    let status = if !validation.errors.is_empty() {
        HealthStatus::Error
    } else if !validation.warnings.is_empty() {
        HealthStatus::Warning
    } else {
        HealthStatus::Healthy
    };

    PluginHealthCheck {
        name: plugin.manifest.name.clone(),
        root: plugin.root.clone(),
        is_disabled: plugin.is_disabled(),
        validation,
        status,
    }
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
            format!(r#"{{"name":"{name}","version":"0.1.0","description":"test plugin"}}"#),
        )
        .expect("write manifest");
    }

    #[test]
    fn perform_startup_checks_healthy() {
        let temp = ok(tempdir());
        create_test_plugin(&temp.path().join("a"), "plugin-a");

        let result = perform_plugin_startup_checks(temp.path());
        assert_eq!(result.total_plugins, 1);
        // Minimal plugin has no skills/hooks/mcp/runtime surfaces, so it
        // receives a warning from validate_plugin_bundle.
        assert_eq!(result.warnings, 1);
        assert_eq!(result.errors, 0);
        assert_eq!(result.status, HealthStatus::Warning);
    }

    #[test]
    fn perform_startup_checks_empty() {
        let temp = ok(tempdir());
        let result = perform_plugin_startup_checks(temp.path());
        assert_eq!(result.total_plugins, 0);
        assert_eq!(result.status, HealthStatus::Healthy);
    }

    #[test]
    fn check_plugin_health_basic() {
        let temp = ok(tempdir());
        create_test_plugin(temp.path(), "test-plugin");

        let plugins = ok(discover_plugins(temp.path()));
        let health = check_plugin_health(&plugins[0]);
        assert_eq!(health.name, "test-plugin");
        assert!(!health.is_disabled);
    }

    #[test]
    fn check_plugin_health_disabled() {
        let temp = ok(tempdir());
        create_test_plugin(temp.path(), "disabled-plugin");
        fs::write(
            temp.path().join(crate::PLUGIN_DISABLED_MARKER),
            b"disabled\n",
        )
        .expect("write marker");

        let plugins = ok(crate::discover_plugins_including_disabled(temp.path()));
        let health = check_plugin_health(&plugins[0]);
        assert!(health.is_disabled);
    }
}
