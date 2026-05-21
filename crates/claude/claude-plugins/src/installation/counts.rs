//! Plugin install counts — track and query plugin popularity.
//!
//! Provides install count tracking for plugins, useful for sorting
//! marketplace results by popularity.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Plugin install counts tracker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginInstallCounts {
    /// Map of plugin ID to install count.
    counts: HashMap<String, u64>,
}

/// A plugin with its install count, for sorting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginCountEntry {
    /// Plugin ID.
    pub plugin_id: String,
    /// Install count.
    pub count: u64,
}

// ---------------------------------------------------------------------------
// Core functions
// ---------------------------------------------------------------------------

impl PluginInstallCounts {
    /// Create a new empty install counts tracker.
    #[must_use]
    pub fn new() -> Self {
        Self {
            counts: HashMap::new(),
        }
    }

    /// Create from a list of (plugin_id, count) pairs.
    pub fn from_pairs(pairs: &[(String, u64)]) -> Self {
        let mut counts = HashMap::new();
        for (id, count) in pairs {
            counts.insert(id.clone(), *count);
        }
        Self { counts }
    }

    /// Increment the install count for a plugin.
    pub fn increment_install_count(&mut self, plugin_id: &str) -> u64 {
        let count = self.counts.entry(plugin_id.to_owned()).or_insert(0);
        *count += 1;
        *count
    }

    /// Get the install count for a plugin.
    pub fn get_install_count(&self, plugin_id: &str) -> u64 {
        self.counts.get(plugin_id).copied().unwrap_or(0)
    }

    /// Set the install count for a plugin.
    pub fn set_install_count(&mut self, plugin_id: &str, count: u64) {
        self.counts.insert(plugin_id.to_owned(), count);
    }

    /// Get all plugins sorted by install count (descending).
    pub fn track_popular_plugins(&self) -> Vec<PluginCountEntry> {
        let mut entries: Vec<PluginCountEntry> = self
            .counts
            .iter()
            .map(|(id, count)| PluginCountEntry {
                plugin_id: id.clone(),
                count: *count,
            })
            .collect();
        entries.sort_by(|a, b| b.count.cmp(&a.count));
        entries
    }

    /// Get the top N plugins by install count.
    pub fn top_n(&self, n: usize) -> Vec<PluginCountEntry> {
        self.track_popular_plugins().into_iter().take(n).collect()
    }

    /// Number of plugins tracked.
    pub fn len(&self) -> usize {
        self.counts.len()
    }

    /// Whether there are no tracked plugins.
    pub fn is_empty(&self) -> bool {
        self.counts.is_empty()
    }

    /// Total install count across all plugins.
    pub fn total_installs(&self) -> u64 {
        self.counts.values().sum()
    }
}

impl Default for PluginInstallCounts {
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
    fn new_is_empty() {
        let counts = PluginInstallCounts::new();
        assert!(counts.is_empty());
    }

    #[test]
    fn increment_and_get() {
        let mut counts = PluginInstallCounts::new();
        assert_eq!(counts.get_install_count("test@mkt"), 0);

        let count = counts.increment_install_count("test@mkt");
        assert_eq!(count, 1);
        assert_eq!(counts.get_install_count("test@mkt"), 1);

        counts.increment_install_count("test@mkt");
        assert_eq!(counts.get_install_count("test@mkt"), 2);
    }

    #[test]
    fn set_install_count() {
        let mut counts = PluginInstallCounts::new();
        counts.set_install_count("test@mkt", 100);
        assert_eq!(counts.get_install_count("test@mkt"), 100);
    }

    #[test]
    fn from_pairs() {
        let counts =
            PluginInstallCounts::from_pairs(&[("a@mkt".to_owned(), 10), ("b@mkt".to_owned(), 20)]);
        assert_eq!(counts.len(), 2);
        assert_eq!(counts.get_install_count("a@mkt"), 10);
        assert_eq!(counts.get_install_count("b@mkt"), 20);
    }

    #[test]
    fn track_popular_sorted() {
        let mut counts = PluginInstallCounts::new();
        counts.set_install_count("a@mkt", 5);
        counts.set_install_count("b@mkt", 50);
        counts.set_install_count("c@mkt", 10);

        let popular = counts.track_popular_plugins();
        assert_eq!(popular[0].plugin_id, "b@mkt");
        assert_eq!(popular[1].plugin_id, "c@mkt");
        assert_eq!(popular[2].plugin_id, "a@mkt");
    }

    #[test]
    fn top_n_works() {
        let mut counts = PluginInstallCounts::new();
        counts.set_install_count("a@mkt", 5);
        counts.set_install_count("b@mkt", 50);
        counts.set_install_count("c@mkt", 10);

        let top = counts.top_n(2);
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].plugin_id, "b@mkt");
    }

    #[test]
    fn total_installs() {
        let mut counts = PluginInstallCounts::new();
        counts.set_install_count("a@mkt", 5);
        counts.set_install_count("b@mkt", 10);
        assert_eq!(counts.total_installs(), 15);
    }

    #[test]
    fn default_is_empty() {
        let counts = PluginInstallCounts::default();
        assert!(counts.is_empty());
    }
}
