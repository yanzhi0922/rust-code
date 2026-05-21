//! Plugin directory management.
//!
//! Centralized plugin directory configuration. Provides the single source of
//! truth for the plugins directory path, supporting both standard and cowork
//! modes, custom cache directories via environment variables, and seed
//! directory layering.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Default plugins directory name.
const PLUGINS_DIR: &str = "plugins";
/// Cowork plugins directory name (used with --cowork flag).
const COWORK_PLUGINS_DIR: &str = "cowork_plugins";
/// Data subdirectory name for per-plugin persistent data.
const DATA_DIR: &str = "data";
/// Marketplaces subdirectory name.
const MARKETPLACES_DIR: &str = "marketplaces";
/// Cache subdirectory name.
const CACHE_DIR: &str = "cache";
/// Known marketplaces file name.
pub const KNOWN_MARKETPLACES_FILE: &str = "known_marketplaces.json";
/// Installed plugins file name.
pub const INSTALLED_PLUGINS_FILE: &str = "installed_plugins.json";

/// Environment variable for overriding the plugin cache directory.
const ENV_PLUGIN_CACHE_DIR: &str = "CLAUDE_CODE_PLUGIN_CACHE_DIR";
/// Environment variable for enabling cowork plugins mode.
const ENV_USE_COWORK_PLUGINS: &str = "CLAUDE_CODE_USE_COWORK_PLUGINS";
/// Environment variable for plugin seed directories.
const ENV_PLUGIN_SEED_DIR: &str = "CLAUDE_CODE_PLUGIN_SEED_DIR";

// ---------------------------------------------------------------------------
// PluginDirectoryManager
// ---------------------------------------------------------------------------

/// Manages plugin directory paths and directory structure creation.
///
/// Provides the single source of truth for all plugin-related directory
/// paths, supporting environment variable overrides and cowork mode.
#[derive(Debug, Clone)]
pub struct PluginDirectoryManager {
    /// Base plugins directory path.
    base_dir: PathBuf,
}

impl PluginDirectoryManager {
    /// Create a new directory manager with the default plugins directory.
    ///
    /// Priority:
    /// 1. `CLAUDE_CODE_PLUGIN_CACHE_DIR` env var (explicit override)
    /// 2. Default: `~/.claude/plugins` or `~/.claude/cowork_plugins`
    pub fn new() -> Self {
        Self {
            base_dir: Self::resolve_plugins_directory(),
        }
    }

    /// Create a directory manager with a specific base directory (for
    /// testing).
    pub fn with_base_dir(base_dir: PathBuf) -> Self {
        Self { base_dir }
    }

    /// Get the base plugins directory path.
    pub fn base_dir(&self) -> &Path {
        &self.base_dir
    }

    /// Get the plugins directory (same as base_dir).
    pub fn get_plugins_dir(&self) -> &Path {
        &self.base_dir
    }

    /// Get the marketplaces directory path.
    pub fn get_marketplace_dir(&self) -> PathBuf {
        self.base_dir.join(MARKETPLACES_DIR)
    }

    /// Get the cache directory path.
    pub fn get_cache_dir(&self) -> PathBuf {
        self.base_dir.join(CACHE_DIR)
    }

    /// Get the data directory path (for per-plugin persistent data).
    pub fn get_data_dir(&self) -> PathBuf {
        self.base_dir.join(DATA_DIR)
    }

    /// Get the path to the known_marketplaces.json file.
    pub fn get_known_marketplaces_path(&self) -> PathBuf {
        self.base_dir.join(KNOWN_MARKETPLACES_FILE)
    }

    /// Get the path to the installed_plugins.json file.
    pub fn get_installed_plugins_path(&self) -> PathBuf {
        self.base_dir.join(INSTALLED_PLUGINS_FILE)
    }

    /// Get the per-plugin data directory path (no mkdir).
    pub fn plugin_data_dir_path(&self, plugin_id: &str) -> PathBuf {
        self.get_data_dir().join(sanitize_plugin_id(plugin_id))
    }

    /// Get or create the per-plugin data directory.
    ///
    /// Creates the directory if it doesn't exist. Returns the path.
    pub fn get_plugin_data_dir(&self, plugin_id: &str) -> Result<PathBuf> {
        let dir = self.plugin_data_dir_path(plugin_id);
        fs::create_dir_all(&dir)
            .with_context(|| format!("Failed to create plugin data dir: {}", dir.display()))?;
        Ok(dir)
    }

    /// Get the marketplace directory for a specific marketplace name.
    pub fn get_marketplace_named_dir(&self, marketplace_name: &str) -> PathBuf {
        self.get_marketplace_dir().join(marketplace_name)
    }

    /// Get the plugin install path within a marketplace.
    pub fn get_plugin_install_path(&self, marketplace_name: &str, plugin_name: &str) -> PathBuf {
        self.get_marketplace_named_dir(marketplace_name)
            .join(plugin_name)
    }

    /// Ensure all plugin directories exist, creating them if necessary.
    pub fn ensure_plugin_dirs(&self) -> Result<()> {
        let dirs = [
            self.base_dir.clone(),
            self.get_marketplace_dir(),
            self.get_cache_dir(),
            self.get_data_dir(),
        ];

        for dir in &dirs {
            fs::create_dir_all(dir)
                .with_context(|| format!("Failed to create plugin directory: {}", dir.display()))?;
        }

        Ok(())
    }

    /// Delete the per-plugin data directory (best-effort).
    ///
    /// Failure is logged but does not propagate — the uninstall itself
    /// already succeeded; we don't want a cleanup side-effect surfacing
    /// as "uninstall failed".
    pub fn delete_plugin_data_dir(&self, plugin_id: &str) {
        let dir = self.plugin_data_dir_path(plugin_id);
        if let Err(e) = fs::remove_dir_all(&dir) {
            // Only log if the directory actually existed
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!("Failed to delete plugin data dir {}: {e}", dir.display());
            }
        }
    }

    // -----------------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------------

    /// Resolve the plugins directory from environment or default.
    fn resolve_plugins_directory() -> PathBuf {
        // Check for explicit override
        if let Ok(env_override) = env::var(ENV_PLUGIN_CACHE_DIR) {
            let expanded = expand_tilde(&env_override);
            return expanded;
        }

        // Default: config home + plugins dir name
        let config_home = get_config_home();
        config_home.join(Self::get_plugins_dir_name())
    }

    /// Get the plugins directory name based on current mode.
    fn get_plugins_dir_name() -> &'static str {
        // Check env var for cowork mode
        if is_env_truthy(ENV_USE_COWORK_PLUGINS) {
            return COWORK_PLUGINS_DIR;
        }
        PLUGINS_DIR
    }
}

impl Default for PluginDirectoryManager {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Seed directories
// ---------------------------------------------------------------------------

/// Get the read-only plugin seed directories, if configured.
///
/// Customers can pre-bake a populated plugins directory into their container
/// image and point `CLAUDE_CODE_PLUGIN_SEED_DIR` at it. Multiple seed
/// directories can be layered using the platform path delimiter.
pub fn get_plugin_seed_dirs() -> Vec<PathBuf> {
    let raw = match env::var(ENV_PLUGIN_SEED_DIR) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };

    let delimiter = if cfg!(windows) { ';' } else { ':' };

    raw.split(delimiter)
        .filter(|s| !s.is_empty())
        .map(expand_tilde)
        .collect()
}

// ---------------------------------------------------------------------------
// Utility functions
// ---------------------------------------------------------------------------

/// Sanitize a plugin ID for use as a directory name.
fn sanitize_plugin_id(plugin_id: &str) -> String {
    plugin_id
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// Expand `~` at the start of a path to the user's home directory.
fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix('~') {
        // Handle ~/ or just ~
        let rest = rest
            .strip_prefix(std::path::MAIN_SEPARATOR_STR)
            .unwrap_or(rest);
        if let Some(base_dirs) = directories::BaseDirs::new() {
            return base_dirs.home_dir().join(rest);
        }
    }
    PathBuf::from(path)
}

/// Get the config home directory (`~/.claude` or equivalent).
fn get_config_home() -> PathBuf {
    // Try CLAUDE_CONFIG_HOME first
    if let Ok(config_home) = env::var("CLAUDE_CONFIG_HOME") {
        return expand_tilde(&config_home);
    }

    // Fall back to ~/.claude
    if let Some(base_dirs) = directories::BaseDirs::new() {
        return base_dirs.home_dir().join(".claude");
    }

    // Last resort: current dir
    PathBuf::from(".claude")
}

/// Check if an environment variable is set to a truthy value.
fn is_env_truthy(key: &str) -> bool {
    match env::var(key) {
        Ok(val) => {
            let lower = val.to_lowercase();
            matches!(lower.as_str(), "1" | "true" | "yes" | "on")
        }
        Err(_) => false,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_plugin_id_normal() {
        assert_eq!(sanitize_plugin_id("my-plugin"), "my-plugin");
    }

    #[test]
    fn test_sanitize_plugin_id_with_special_chars() {
        assert_eq!(sanitize_plugin_id("my@marketplace"), "my-marketplace");
    }

    #[test]
    fn test_sanitize_plugin_id_with_slashes() {
        assert_eq!(sanitize_plugin_id("my/plugin/name"), "my-plugin-name");
    }

    #[test]
    fn test_sanitize_plugin_id_preserves_alphanumeric() {
        assert_eq!(sanitize_plugin_id("plugin_name-123"), "plugin_name-123");
    }

    #[test]
    fn test_directory_manager_with_base_dir() {
        let tmp = tempfile::tempdir().expect("create temp dir");
        let base = tmp.path().to_path_buf();
        let mgr = PluginDirectoryManager::with_base_dir(base.clone());

        assert_eq!(mgr.base_dir(), &base);
        assert_eq!(mgr.get_plugins_dir(), &base);
        assert_eq!(mgr.get_marketplace_dir(), base.join(MARKETPLACES_DIR));
        assert_eq!(mgr.get_cache_dir(), base.join(CACHE_DIR));
        assert_eq!(mgr.get_data_dir(), base.join(DATA_DIR));
    }

    #[test]
    fn test_ensure_plugin_dirs_creates_directories() {
        let tmp = tempfile::tempdir().expect("create temp dir");
        let base = tmp.path().join("plugins");
        let mgr = PluginDirectoryManager::with_base_dir(base.clone());

        mgr.ensure_plugin_dirs().expect("ensure dirs");

        assert!(base.exists());
        assert!(base.join(MARKETPLACES_DIR).exists());
        assert!(base.join(CACHE_DIR).exists());
        assert!(base.join(DATA_DIR).exists());
    }

    #[test]
    fn test_ensure_plugin_dirs_idempotent() {
        let tmp = tempfile::tempdir().expect("create temp dir");
        let base = tmp.path().join("plugins");
        let mgr = PluginDirectoryManager::with_base_dir(base.clone());

        mgr.ensure_plugin_dirs().expect("first ensure");
        mgr.ensure_plugin_dirs().expect("second ensure");

        assert!(base.exists());
    }

    #[test]
    fn test_plugin_data_dir_path() {
        let tmp = tempfile::tempdir().expect("create temp dir");
        let base = tmp.path().to_path_buf();
        let mgr = PluginDirectoryManager::with_base_dir(base.clone());

        let path = mgr.plugin_data_dir_path("my-plugin@market");
        assert_eq!(path, base.join(DATA_DIR).join("my-plugin-market"));
    }

    #[test]
    fn test_get_plugin_data_dir_creates_dir() {
        let tmp = tempfile::tempdir().expect("create temp dir");
        let base = tmp.path().join("plugins");
        let mgr = PluginDirectoryManager::with_base_dir(base.clone());

        let dir = mgr
            .get_plugin_data_dir("test-plugin")
            .expect("get data dir");
        assert!(dir.exists());
        assert!(dir.is_dir());
    }

    #[test]
    fn test_get_marketplace_named_dir() {
        let tmp = tempfile::tempdir().expect("create temp dir");
        let base = tmp.path().to_path_buf();
        let mgr = PluginDirectoryManager::with_base_dir(base.clone());

        let path = mgr.get_marketplace_named_dir("anthropic-tools");
        assert_eq!(path, base.join(MARKETPLACES_DIR).join("anthropic-tools"));
    }

    #[test]
    fn test_get_plugin_install_path() {
        let tmp = tempfile::tempdir().expect("create temp dir");
        let base = tmp.path().to_path_buf();
        let mgr = PluginDirectoryManager::with_base_dir(base.clone());

        let path = mgr.get_plugin_install_path("anthropic-tools", "code-formatter");
        assert_eq!(
            path,
            base.join(MARKETPLACES_DIR)
                .join("anthropic-tools")
                .join("code-formatter")
        );
    }

    #[test]
    fn test_get_known_marketplaces_path() {
        let tmp = tempfile::tempdir().expect("create temp dir");
        let base = tmp.path().to_path_buf();
        let mgr = PluginDirectoryManager::with_base_dir(base.clone());

        assert_eq!(
            mgr.get_known_marketplaces_path(),
            base.join(KNOWN_MARKETPLACES_FILE)
        );
    }

    #[test]
    fn test_get_installed_plugins_path() {
        let tmp = tempfile::tempdir().expect("create temp dir");
        let base = tmp.path().to_path_buf();
        let mgr = PluginDirectoryManager::with_base_dir(base.clone());

        assert_eq!(
            mgr.get_installed_plugins_path(),
            base.join(INSTALLED_PLUGINS_FILE)
        );
    }

    #[test]
    fn test_delete_plugin_data_dir() {
        let tmp = tempfile::tempdir().expect("create temp dir");
        let base = tmp.path().join("plugins");
        let mgr = PluginDirectoryManager::with_base_dir(base.clone());

        // Create the data dir first
        let dir = mgr.get_plugin_data_dir("test-plugin").expect("create");
        assert!(dir.exists());

        // Delete it
        mgr.delete_plugin_data_dir("test-plugin");
        assert!(!dir.exists());
    }

    #[test]
    fn test_delete_plugin_data_dir_nonexistent() {
        let tmp = tempfile::tempdir().expect("create temp dir");
        let base = tmp.path().join("plugins");
        let mgr = PluginDirectoryManager::with_base_dir(base.clone());

        // Should not panic on nonexistent dir
        mgr.delete_plugin_data_dir("nonexistent-plugin");
    }

    #[test]
    fn test_expand_tilde() {
        if directories::BaseDirs::new().is_some() {
            let expanded = expand_tilde("~/test");
            assert!(!expanded.starts_with("~"));
            assert!(expanded.ends_with("test"));
        }
    }

    #[test]
    fn test_expand_tilde_no_tilde() {
        let expanded = expand_tilde("/absolute/path");
        assert_eq!(expanded, PathBuf::from("/absolute/path"));
    }

    #[test]
    fn test_default_impl() {
        let mgr = PluginDirectoryManager::default();
        assert!(
            mgr.base_dir().to_string_lossy().contains("plugins")
                || mgr.base_dir().to_string_lossy().contains("cowork_plugins")
        );
    }
}
