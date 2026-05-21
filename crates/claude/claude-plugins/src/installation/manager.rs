//! Plugin installation manager.
//!
//! Manages the lifecycle of plugin installations: install, uninstall, update,
//! and listing installed plugins.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Progress of an installation operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InstallationProgress {
    /// Downloading plugin files.
    Downloading,
    /// Extracting plugin archive.
    Extracting,
    /// Verifying plugin integrity.
    Verifying,
    /// Resolving dependencies.
    ResolvingDependencies,
    /// Registering plugin.
    Registering,
    /// Installation complete.
    Complete,
    /// Installation failed.
    Failed,
}

/// An installed plugin record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledPlugin {
    /// Plugin ID (`"name@marketplace"`).
    pub plugin_id: String,
    /// Installation path.
    pub install_path: PathBuf,
    /// Installed version.
    pub version: String,
    /// Installation timestamp (ISO 8601).
    pub installed_at: String,
    /// Whether the plugin is enabled.
    pub enabled: bool,
}

/// Result of an installation operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallResult {
    /// Plugin ID.
    pub plugin_id: String,
    /// Installation path.
    pub install_path: PathBuf,
    /// Whether the installation succeeded.
    pub success: bool,
    /// Error message if installation failed.
    pub error: Option<String>,
}

/// Plugin installation manager.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginInstallationManager {
    /// Base directory for plugin installations.
    install_base: PathBuf,
    /// Map of installed plugins by ID.
    installed: HashMap<String, InstalledPlugin>,
}

// ---------------------------------------------------------------------------
// Core functions
// ---------------------------------------------------------------------------

impl PluginInstallationManager {
    /// Create a new installation manager with the given base directory.
    pub fn new(install_base: PathBuf) -> Self {
        Self {
            install_base,
            installed: HashMap::new(),
        }
    }

    /// Install a plugin.
    ///
    /// Creates the installation directory and registers the plugin.
    pub fn install_plugin(
        &mut self,
        plugin_id: &str,
        version: &str,
        _source_path: &Path,
    ) -> InstallResult {
        let install_path = compute_install_dir(&self.install_base, plugin_id);

        // Create install directory
        if let Err(e) = std::fs::create_dir_all(&install_path) {
            return InstallResult {
                plugin_id: plugin_id.to_owned(),
                install_path: install_path.clone(),
                success: false,
                error: Some(format!("failed to create install dir: {e}")),
            };
        }

        // Register the plugin
        self.installed.insert(
            plugin_id.to_owned(),
            InstalledPlugin {
                plugin_id: plugin_id.to_owned(),
                install_path: install_path.clone(),
                version: version.to_owned(),
                installed_at: chrono::Utc::now().to_rfc3339(),
                enabled: true,
            },
        );

        InstallResult {
            plugin_id: plugin_id.to_owned(),
            install_path,
            success: true,
            error: None,
        }
    }

    /// Uninstall a plugin.
    ///
    /// Removes the plugin directory and unregisters it.
    pub fn uninstall_plugin(&mut self, plugin_id: &str) -> InstallResult {
        let install_path = match self.installed.remove(plugin_id) {
            Some(plugin) => plugin.install_path,
            None => {
                return InstallResult {
                    plugin_id: plugin_id.to_owned(),
                    install_path: PathBuf::new(),
                    success: false,
                    error: Some("plugin not found".to_owned()),
                };
            }
        };

        // Remove install directory (best effort)
        let _ = std::fs::remove_dir_all(&install_path);

        InstallResult {
            plugin_id: plugin_id.to_owned(),
            install_path,
            success: true,
            error: None,
        }
    }

    /// Update a plugin to a new version.
    pub fn update_plugin(&mut self, plugin_id: &str, new_version: &str) -> InstallResult {
        let plugin = match self.installed.get_mut(plugin_id) {
            Some(p) => p,
            None => {
                return InstallResult {
                    plugin_id: plugin_id.to_owned(),
                    install_path: PathBuf::new(),
                    success: false,
                    error: Some("plugin not found".to_owned()),
                };
            }
        };

        let install_path = plugin.install_path.clone();
        plugin.version = new_version.to_owned();

        InstallResult {
            plugin_id: plugin_id.to_owned(),
            install_path,
            success: true,
            error: None,
        }
    }

    /// List all installed plugins.
    pub fn list_installed_plugins(&self) -> Vec<&InstalledPlugin> {
        self.installed.values().collect()
    }

    /// Check if a plugin is installed.
    pub fn is_installed(&self, plugin_id: &str) -> bool {
        self.installed.contains_key(plugin_id)
    }

    /// Get an installed plugin by ID.
    pub fn get_installed(&self, plugin_id: &str) -> Option<&InstalledPlugin> {
        self.installed.get(plugin_id)
    }

    /// Number of installed plugins.
    pub fn len(&self) -> usize {
        self.installed.len()
    }

    /// Whether there are no installed plugins.
    pub fn is_empty(&self) -> bool {
        self.installed.is_empty()
    }
}

/// Compute the installation directory for a plugin.
fn compute_install_dir(base: &Path, plugin_id: &str) -> PathBuf {
    // Replace @ with directory separator for filesystem safety
    let safe_id = plugin_id.replace('@', std::path::MAIN_SEPARATOR.encode_utf8(&mut [0u8; 4]));
    base.join(safe_id)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn ok<T, E: std::fmt::Display>(result: Result<T, E>) -> T {
        match result {
            Ok(value) => value,
            Err(error) => panic!("unexpected error: {error}"),
        }
    }

    #[test]
    fn new_manager_is_empty() {
        let mgr = PluginInstallationManager::new(PathBuf::from("/tmp/plugins"));
        assert!(mgr.is_empty());
    }

    #[test]
    fn install_and_list() {
        let temp = ok(tempdir());
        let mut mgr = PluginInstallationManager::new(temp.path().to_path_buf());
        let result = mgr.install_plugin("test@mkt", "1.0.0", Path::new("/source"));
        assert!(result.success);
        assert_eq!(mgr.len(), 1);

        let list = mgr.list_installed_plugins();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].plugin_id, "test@mkt");
    }

    #[test]
    fn install_and_is_installed() {
        let temp = ok(tempdir());
        let mut mgr = PluginInstallationManager::new(temp.path().to_path_buf());
        mgr.install_plugin("test@mkt", "1.0.0", Path::new("/source"));
        assert!(mgr.is_installed("test@mkt"));
        assert!(!mgr.is_installed("other@mkt"));
    }

    #[test]
    fn uninstall_removes() {
        let temp = ok(tempdir());
        let mut mgr = PluginInstallationManager::new(temp.path().to_path_buf());
        mgr.install_plugin("test@mkt", "1.0.0", Path::new("/source"));
        let result = mgr.uninstall_plugin("test@mkt");
        assert!(result.success);
        assert!(mgr.is_empty());
    }

    #[test]
    fn uninstall_nonexistent() {
        let temp = ok(tempdir());
        let mut mgr = PluginInstallationManager::new(temp.path().to_path_buf());
        let result = mgr.uninstall_plugin("nonexistent@mkt");
        assert!(!result.success);
    }

    #[test]
    fn update_changes_version() {
        let temp = ok(tempdir());
        let mut mgr = PluginInstallationManager::new(temp.path().to_path_buf());
        mgr.install_plugin("test@mkt", "1.0.0", Path::new("/source"));
        let result = mgr.update_plugin("test@mkt", "2.0.0");
        assert!(result.success);

        let plugin = mgr.get_installed("test@mkt").expect("plugin");
        assert_eq!(plugin.version, "2.0.0");
    }

    #[test]
    fn update_nonexistent() {
        let temp = ok(tempdir());
        let mut mgr = PluginInstallationManager::new(temp.path().to_path_buf());
        let result = mgr.update_plugin("nonexistent@mkt", "2.0.0");
        assert!(!result.success);
    }
}
