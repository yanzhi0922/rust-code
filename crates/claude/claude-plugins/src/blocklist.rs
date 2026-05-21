//! Plugin blocklist management.
//!
//! Rust equivalent of `pluginBlocklist.ts`. Tracks plugins that have been
//! removed from marketplaces (delisted) and auto-uninstalled. Data is
//! persisted to a JSON file in the plugins directory.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

/// Errors that can occur during blocklist operations.
#[derive(Debug, Error)]
pub enum BlocklistError {
    /// I/O error reading or writing the blocklist file.
    #[error("blocklist I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// Failed to parse the blocklist JSON.
    #[error("blocklist parse error: {0}")]
    Parse(#[from] serde_json::Error),
}

// ---------------------------------------------------------------------------
// BlockReason
// ---------------------------------------------------------------------------

/// Why a plugin was added to the blocklist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockReason {
    /// Plugin has a known security vulnerability.
    Security,
    /// Plugin has been deprecated by its author or marketplace.
    Deprecated,
    /// Manually blocked by the user.
    UserBlocked,
    /// Plugin was delisted from its marketplace.
    Delisted,
    /// Plugin failed compatibility checks.
    Incompatible,
}

// ---------------------------------------------------------------------------
// BlocklistEntry
// ---------------------------------------------------------------------------

/// A single blocklisted plugin record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlocklistEntry {
    /// Plugin ID in `"name@marketplace"` format.
    pub plugin_id: String,
    /// Why the plugin was blocked.
    pub reason: BlockReason,
    /// When the plugin was added to the blocklist.
    pub blocked_at: DateTime<Utc>,
    /// Optional human-readable explanation.
    #[serde(default)]
    pub details: Option<String>,
}

// ---------------------------------------------------------------------------
// PluginBlocklist
// ---------------------------------------------------------------------------

/// In-memory representation of the plugin blocklist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PluginBlocklist {
    /// Map from plugin ID to blocklist entry.
    pub entries: HashMap<String, BlocklistEntry>,
}

impl PluginBlocklist {
    /// Create a new empty blocklist.
    pub fn new() -> Self {
        Self::default()
    }

    /// Check if a plugin is blocklisted.
    pub fn is_blocked(&self, plugin_id: &str) -> bool {
        self.entries.contains_key(plugin_id)
    }

    /// Get the blocklist entry for a plugin, if any.
    pub fn get_entry(&self, plugin_id: &str) -> Option<&BlocklistEntry> {
        self.entries.get(plugin_id)
    }

    /// Block a plugin.
    pub fn block_plugin(
        &mut self,
        plugin_id: impl Into<String>,
        reason: BlockReason,
        details: Option<String>,
    ) {
        let id = plugin_id.into();
        self.entries.insert(
            id.clone(),
            BlocklistEntry {
                plugin_id: id,
                reason,
                blocked_at: Utc::now(),
                details,
            },
        );
    }

    /// Unblock a plugin (remove from blocklist).
    ///
    /// Returns `true` if the plugin was previously blocked.
    pub fn unblock_plugin(&mut self, plugin_id: &str) -> bool {
        self.entries.remove(plugin_id).is_some()
    }

    /// Return the number of blocklisted plugins.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Return whether the blocklist is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Persistence
// ---------------------------------------------------------------------------

/// File name for the blocklist JSON file.
pub const BLOCKLIST_FILENAME: &str = "blocked-plugins.json";

/// Load the blocklist from a JSON file.
pub fn load_blocklist(path: &Path) -> Result<PluginBlocklist, BlocklistError> {
    if !path.exists() {
        return Ok(PluginBlocklist::new());
    }
    let content = std::fs::read_to_string(path)?;
    if content.trim().is_empty() {
        return Ok(PluginBlocklist::new());
    }
    let blocklist: PluginBlocklist = serde_json::from_str(&content)?;
    Ok(blocklist)
}

/// Save the blocklist to a JSON file.
///
/// Writes atomically via a temp file to avoid corruption.
pub fn save_blocklist(blocklist: &PluginBlocklist, path: &Path) -> Result<(), BlocklistError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content = serde_json::to_string_pretty(blocklist)?;
    // Atomic write: write to temp file then rename
    let tmp_path = path.with_extension("json.tmp");
    std::fs::write(&tmp_path, &content)?;
    std::fs::rename(&tmp_path, path)?;
    Ok(())
}

/// Convenience: get the default blocklist file path for a given plugins
/// directory.
pub fn blocklist_path(plugins_dir: &Path) -> PathBuf {
    plugins_dir.join(BLOCKLIST_FILENAME)
}

// ---------------------------------------------------------------------------
// Delist detection
// ---------------------------------------------------------------------------

/// Detect plugins installed from a marketplace that are no longer listed.
///
/// Given the set of installed plugin IDs and the set of marketplace plugin
/// names, returns the IDs of plugins that have been delisted.
pub fn detect_delisted_plugins(
    installed_ids: &[String],
    marketplace_plugin_names: &[String],
    marketplace_name: &str,
) -> Vec<String> {
    let available: std::collections::HashSet<&str> = marketplace_plugin_names
        .iter()
        .map(|s| s.as_str())
        .collect();
    let suffix = format!("@{marketplace_name}");

    installed_ids
        .iter()
        .filter(|id| id.ends_with(&suffix) && !available.contains(&id[..id.len() - suffix.len()]))
        .cloned()
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn new_blocklist_is_empty() {
        let bl = PluginBlocklist::new();
        assert!(bl.is_empty());
        assert_eq!(bl.len(), 0);
    }

    #[test]
    fn block_and_check() {
        let mut bl = PluginBlocklist::new();
        assert!(!bl.is_blocked("bad@m"));
        bl.block_plugin("bad@m", BlockReason::Security, Some("CVE-2024".into()));
        assert!(bl.is_blocked("bad@m"));
        assert_eq!(bl.len(), 1);
    }

    #[test]
    fn block_sets_reason() {
        let mut bl = PluginBlocklist::new();
        bl.block_plugin("p@m", BlockReason::Deprecated, None);
        let entry = bl.get_entry("p@m").expect("entry exists");
        assert_eq!(entry.reason, BlockReason::Deprecated);
        assert!(entry.blocked_at <= Utc::now());
    }

    #[test]
    fn unblock_removes() {
        let mut bl = PluginBlocklist::new();
        bl.block_plugin("p@m", BlockReason::UserBlocked, None);
        assert!(bl.unblock_plugin("p@m"));
        assert!(!bl.is_blocked("p@m"));
    }

    #[test]
    fn unblock_nonexistent_returns_false() {
        let mut bl = PluginBlocklist::new();
        assert!(!bl.unblock_plugin("nope@m"));
    }

    #[test]
    fn save_and_load_roundtrip() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("blocked-plugins.json");

        let mut bl = PluginBlocklist::new();
        bl.block_plugin("a@m", BlockReason::Security, Some("vuln".into()));
        bl.block_plugin("b@m", BlockReason::Deprecated, None);

        save_blocklist(&bl, &path).expect("save");
        let loaded = load_blocklist(&path).expect("load");

        assert_eq!(loaded.len(), 2);
        assert!(loaded.is_blocked("a@m"));
        assert!(loaded.is_blocked("b@m"));
        assert_eq!(
            loaded.get_entry("a@m").expect("entry").reason,
            BlockReason::Security
        );
    }

    #[test]
    fn load_missing_file_returns_empty() {
        let path = PathBuf::from("/nonexistent/blocked-plugins.json");
        let bl = load_blocklist(&path).expect("should succeed with empty");
        assert!(bl.is_empty());
    }

    #[test]
    fn load_empty_file_returns_empty() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("blocked-plugins.json");
        std::fs::write(&path, "").expect("write empty");
        let bl = load_blocklist(&path).expect("should succeed");
        assert!(bl.is_empty());
    }

    #[test]
    fn detect_delisted() {
        let installed = vec![
            "plugin-a@official".to_string(),
            "plugin-b@official".to_string(),
            "plugin-c@community".to_string(),
        ];
        let marketplace_names = vec!["plugin-a".to_string()];
        let delisted = detect_delisted_plugins(&installed, &marketplace_names, "official");
        assert_eq!(delisted, vec!["plugin-b@official"]);
    }

    #[test]
    fn detect_delisted_empty() {
        let installed: Vec<String> = vec!["plugin-a@m".to_string()];
        let marketplace_names = vec!["plugin-a".to_string()];
        let delisted = detect_delisted_plugins(&installed, &marketplace_names, "m");
        assert!(delisted.is_empty());
    }

    #[test]
    fn blocklist_path_helper() {
        let dir = PathBuf::from("/home/user/.claude/plugins");
        let p = blocklist_path(&dir);
        assert_eq!(
            p,
            PathBuf::from("/home/user/.claude/plugins/blocked-plugins.json")
        );
    }
}
