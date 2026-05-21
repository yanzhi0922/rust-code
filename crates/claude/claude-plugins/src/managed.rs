//! Managed plugins — plugins locked by org policy.
//!
//! Provides [`ManagedPluginManager`] for tracking which plugins are managed
//! by organizational policy and cannot be modified by users.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Manager for plugins locked by organizational policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedPluginManager {
    /// Set of managed plugin names.
    managed: HashSet<String>,
    /// Source of each managed plugin (e.g., "policy", "admin").
    sources: HashMap<String, String>,
}

// ---------------------------------------------------------------------------
// Core functions
// ---------------------------------------------------------------------------

impl ManagedPluginManager {
    /// Create a new empty managed plugin manager.
    #[must_use]
    pub fn new() -> Self {
        Self {
            managed: HashSet::new(),
            sources: HashMap::new(),
        }
    }

    /// Create a managed plugin manager from a list of plugin IDs.
    ///
    /// Plugin IDs in `"name@marketplace"` format are parsed to extract names.
    /// Only boolean `true` entries are considered managed.
    pub fn from_enabled_plugins(enabled: &HashMap<String, bool>) -> Self {
        let mut managed = HashSet::new();
        let mut sources = HashMap::new();

        for (plugin_id, value) in enabled {
            if !value {
                continue;
            }
            if let Some(at_pos) = plugin_id.find('@') {
                let name = &plugin_id[..at_pos];
                if !name.is_empty() {
                    managed.insert(name.to_owned());
                    sources.insert(name.to_owned(), "policy".to_owned());
                }
            }
        }

        Self { managed, sources }
    }

    /// Add a managed plugin.
    pub fn add_managed_plugin(&mut self, name: &str, source: &str) {
        self.managed.insert(name.to_owned());
        self.sources.insert(name.to_owned(), source.to_owned());
    }

    /// Remove a managed plugin.
    pub fn remove_managed_plugin(&mut self, name: &str) {
        self.managed.remove(name);
        self.sources.remove(name);
    }

    /// Check if a plugin is managed.
    pub fn is_managed_plugin(&self, name: &str) -> bool {
        self.managed.contains(name)
    }

    /// Get the source of a managed plugin.
    pub fn get_source(&self, name: &str) -> Option<&str> {
        self.sources.get(name).map(|s| s.as_str())
    }

    /// Get all managed plugin names.
    pub fn managed_names(&self) -> &HashSet<String> {
        &self.managed
    }

    /// Sync managed plugins with a marketplace.
    ///
    /// Ensures all managed plugins are present in the marketplace.
    /// Returns the list of plugins that need to be installed.
    pub fn sync_managed_plugins(&self, available_in_marketplace: &[String]) -> Vec<String> {
        self.managed
            .iter()
            .filter(|name| !available_in_marketplace.contains(name))
            .cloned()
            .collect()
    }

    /// Number of managed plugins.
    pub fn len(&self) -> usize {
        self.managed.len()
    }

    /// Whether there are no managed plugins.
    pub fn is_empty(&self) -> bool {
        self.managed.is_empty()
    }
}

impl Default for ManagedPluginManager {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_manager_is_empty() {
        let mgr = ManagedPluginManager::new();
        assert!(mgr.is_empty());
        assert_eq!(mgr.len(), 0);
    }

    #[test]
    fn add_and_check_managed() {
        let mut mgr = ManagedPluginManager::new();
        mgr.add_managed_plugin("my-plugin", "policy");
        assert!(mgr.is_managed_plugin("my-plugin"));
        assert!(!mgr.is_managed_plugin("other-plugin"));
        assert_eq!(mgr.len(), 1);
    }

    #[test]
    fn remove_managed() {
        let mut mgr = ManagedPluginManager::new();
        mgr.add_managed_plugin("my-plugin", "policy");
        mgr.remove_managed_plugin("my-plugin");
        assert!(!mgr.is_managed_plugin("my-plugin"));
        assert!(mgr.is_empty());
    }

    #[test]
    fn get_source() {
        let mut mgr = ManagedPluginManager::new();
        mgr.add_managed_plugin("my-plugin", "admin");
        assert_eq!(mgr.get_source("my-plugin"), Some("admin"));
        assert_eq!(mgr.get_source("other"), None);
    }

    #[test]
    fn from_enabled_plugins() {
        let mut enabled = HashMap::new();
        enabled.insert("plugin-a@mkt".to_owned(), true);
        enabled.insert("plugin-b@mkt".to_owned(), true);
        enabled.insert("plugin-c@mkt".to_owned(), false);
        enabled.insert("no-at-sign".to_owned(), true);

        let mgr = ManagedPluginManager::from_enabled_plugins(&enabled);
        assert!(mgr.is_managed_plugin("plugin-a"));
        assert!(mgr.is_managed_plugin("plugin-b"));
        assert!(!mgr.is_managed_plugin("plugin-c"));
        assert!(!mgr.is_managed_plugin("no-at-sign"));
    }

    #[test]
    fn sync_managed_plugins() {
        let mut mgr = ManagedPluginManager::new();
        mgr.add_managed_plugin("plugin-a", "policy");
        mgr.add_managed_plugin("plugin-b", "policy");

        let available = vec!["plugin-a".to_owned()];
        let missing = mgr.sync_managed_plugins(&available);
        assert_eq!(missing, vec!["plugin-b"]);
    }

    #[test]
    fn default_is_empty() {
        let mgr = ManagedPluginManager::default();
        assert!(mgr.is_empty());
    }
}
