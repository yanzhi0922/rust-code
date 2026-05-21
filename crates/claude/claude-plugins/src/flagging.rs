//! Plugin flagging system.
//!
//! Rust equivalent of `pluginFlagging.ts`. Tracks plugins that were
//! auto-removed because they were delisted from their marketplace, or
//! flagged for security, performance, or compatibility issues. Data is
//! persisted to `flagged-plugins.json` in the plugins directory.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

/// Errors during flagging operations.
#[derive(Debug, Error)]
pub enum FlaggingError {
    /// I/O error reading or writing the flag file.
    #[error("flagging I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// Failed to parse the flag JSON.
    #[error("flagging parse error: {0}")]
    Parse(#[from] serde_json::Error),
}

// ---------------------------------------------------------------------------
// Flag types
// ---------------------------------------------------------------------------

/// Categories of plugin flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginFlag {
    /// Known security vulnerability.
    Security,
    /// Performance degradation.
    Performance,
    /// Compatibility issues with current runtime.
    Compatibility,
    /// Suspected malicious behaviour.
    Malicious,
    /// Plugin was delisted from marketplace.
    Delisted,
    /// Deprecated by author.
    Deprecated,
}

/// Severity of a flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FlagSeverity {
    /// Low impact, informational.
    Low,
    /// Moderate impact.
    Medium,
    /// High impact, should be addressed.
    High,
    /// Critical, plugin should not be used.
    Critical,
}

impl FlagSeverity {
    /// Numeric severity for comparison (higher = more severe).
    pub fn level(&self) -> u8 {
        match self {
            Self::Low => 0,
            Self::Medium => 1,
            Self::High => 2,
            Self::Critical => 3,
        }
    }
}

impl PartialOrd for FlagSeverity {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for FlagSeverity {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.level().cmp(&other.level())
    }
}

// ---------------------------------------------------------------------------
// Flag entry
// ---------------------------------------------------------------------------

/// A flag record for a single plugin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlagEntry {
    /// Plugin ID in `"name@marketplace"` format.
    pub plugin_id: String,
    /// When the flag was created.
    pub flagged_at: DateTime<Utc>,
    /// When the user last saw this flag (for dismissal tracking).
    #[serde(default)]
    pub seen_at: Option<DateTime<Utc>>,
    /// The flag category.
    pub flag: PluginFlag,
    /// Severity level.
    pub severity: FlagSeverity,
    /// Optional human-readable message.
    #[serde(default)]
    pub message: Option<String>,
}

// ---------------------------------------------------------------------------
// Flag store
// ---------------------------------------------------------------------------

/// In-memory store of plugin flags.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct FlagStore {
    /// Map from plugin ID to its flag entry.
    pub flags: HashMap<String, FlagEntry>,
}

impl FlagStore {
    /// Create a new empty flag store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Check if a plugin has any flags.
    pub fn is_flagged(&self, plugin_id: &str) -> bool {
        self.flags.contains_key(plugin_id)
    }

    /// Get the flag entry for a plugin.
    pub fn get_flag(&self, plugin_id: &str) -> Option<&FlagEntry> {
        self.flags.get(plugin_id)
    }

    /// Flag a plugin.
    pub fn flag_plugin(
        &mut self,
        plugin_id: impl Into<String>,
        flag: PluginFlag,
        severity: FlagSeverity,
        message: Option<String>,
    ) {
        let id = plugin_id.into();
        self.flags.insert(
            id.clone(),
            FlagEntry {
                plugin_id: id,
                flagged_at: Utc::now(),
                seen_at: None,
                flag,
                severity,
                message,
            },
        );
    }

    /// Mark a flagged plugin as seen.
    ///
    /// Returns `true` if the plugin was flagged and updated.
    pub fn mark_seen(&mut self, plugin_id: &str) -> bool {
        if let Some(entry) = self.flags.get_mut(plugin_id) {
            entry.seen_at = Some(Utc::now());
            true
        } else {
            false
        }
    }

    /// Remove a flag (dismiss).
    ///
    /// Returns `true` if the flag existed.
    pub fn dismiss_flag(&mut self, plugin_id: &str) -> bool {
        self.flags.remove(plugin_id).is_some()
    }

    /// Remove flags that have been seen longer than `max_age` ago.
    pub fn evict_seen(&mut self, max_age: chrono::Duration) {
        let now = Utc::now();
        self.flags.retain(|_, entry| {
            match entry.seen_at {
                Some(seen) => now - seen < max_age,
                None => true, // Not seen yet → keep
            }
        });
    }

    /// Return the number of flagged plugins.
    pub fn len(&self) -> usize {
        self.flags.len()
    }

    /// Return whether the store is empty.
    pub fn is_empty(&self) -> bool {
        self.flags.is_empty()
    }

    /// Get all flagged plugin IDs.
    pub fn flagged_ids(&self) -> Vec<&str> {
        self.flags.keys().map(|s| s.as_str()).collect()
    }

    /// Get flags above a severity threshold.
    pub fn flags_above_severity(&self, min_severity: FlagSeverity) -> Vec<&FlagEntry> {
        self.flags
            .values()
            .filter(|e| e.severity >= min_severity)
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Persistence
// ---------------------------------------------------------------------------

/// File name for the flags JSON file.
pub const FLAGS_FILENAME: &str = "flagged-plugins.json";

/// Default expiry for seen flags: 48 hours.
pub const SEEN_EXPIRY_HOURS: i64 = 48;

/// Load the flag store from a JSON file.
pub fn load_flags(path: &Path) -> Result<FlagStore, FlaggingError> {
    if !path.exists() {
        return Ok(FlagStore::new());
    }
    let content = std::fs::read_to_string(path)?;
    if content.trim().is_empty() {
        return Ok(FlagStore::new());
    }
    let store: FlagStore = serde_json::from_str(&content)?;

    // Evict expired seen entries
    let mut store = store;
    store.evict_seen(chrono::Duration::hours(SEEN_EXPIRY_HOURS));

    Ok(store)
}

/// Save the flag store to a JSON file (atomic write).
pub fn save_flags(store: &FlagStore, path: &Path) -> Result<(), FlaggingError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content = serde_json::to_string_pretty(store)?;
    let tmp_path = path.with_extension("json.tmp");
    std::fs::write(&tmp_path, &content)?;
    std::fs::rename(&tmp_path, path)?;
    Ok(())
}

/// Convenience: get the default flags file path.
pub fn flags_path(plugins_dir: &Path) -> PathBuf {
    plugins_dir.join(FLAGS_FILENAME)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn new_store_is_empty() {
        let store = FlagStore::new();
        assert!(store.is_empty());
    }

    #[test]
    fn flag_and_check() {
        let mut store = FlagStore::new();
        assert!(!store.is_flagged("p@m"));
        store.flag_plugin(
            "p@m",
            PluginFlag::Security,
            FlagSeverity::High,
            Some("CVE".into()),
        );
        assert!(store.is_flagged("p@m"));
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn flag_sets_fields() {
        let mut store = FlagStore::new();
        store.flag_plugin("p@m", PluginFlag::Malicious, FlagSeverity::Critical, None);
        let entry = store.get_flag("p@m").expect("entry");
        assert_eq!(entry.flag, PluginFlag::Malicious);
        assert_eq!(entry.severity, FlagSeverity::Critical);
        assert!(entry.seen_at.is_none());
    }

    #[test]
    fn mark_seen_updates() {
        let mut store = FlagStore::new();
        store.flag_plugin("p@m", PluginFlag::Delisted, FlagSeverity::Low, None);
        assert!(store.mark_seen("p@m"));
        let entry = store.get_flag("p@m").expect("entry");
        assert!(entry.seen_at.is_some());
    }

    #[test]
    fn mark_seen_nonexistent() {
        let mut store = FlagStore::new();
        assert!(!store.mark_seen("nope@m"));
    }

    #[test]
    fn dismiss_removes() {
        let mut store = FlagStore::new();
        store.flag_plugin("p@m", PluginFlag::Deprecated, FlagSeverity::Medium, None);
        assert!(store.dismiss_flag("p@m"));
        assert!(!store.is_flagged("p@m"));
    }

    #[test]
    fn dismiss_nonexistent() {
        let mut store = FlagStore::new();
        assert!(!store.dismiss_flag("nope@m"));
    }

    #[test]
    fn evict_seen_removes_old() {
        let mut store = FlagStore::new();
        store.flag_plugin("old@m", PluginFlag::Delisted, FlagSeverity::Low, None);
        // Manually set seen_at to far past
        let entry = store.flags.get_mut("old@m").expect("entry");
        entry.seen_at = Some(Utc::now() - chrono::Duration::hours(100));

        store.flag_plugin("new@m", PluginFlag::Security, FlagSeverity::High, None);

        store.evict_seen(chrono::Duration::hours(48));
        assert!(!store.is_flagged("old@m"));
        assert!(store.is_flagged("new@m"));
    }

    #[test]
    fn flagged_ids_returns_all() {
        let mut store = FlagStore::new();
        store.flag_plugin("a@m", PluginFlag::Security, FlagSeverity::High, None);
        store.flag_plugin("b@m", PluginFlag::Delisted, FlagSeverity::Low, None);
        let mut ids: Vec<&str> = store.flagged_ids();
        ids.sort();
        assert_eq!(ids, vec!["a@m", "b@m"]);
    }

    #[test]
    fn flags_above_severity() {
        let mut store = FlagStore::new();
        store.flag_plugin("low@m", PluginFlag::Delisted, FlagSeverity::Low, None);
        store.flag_plugin("high@m", PluginFlag::Security, FlagSeverity::High, None);
        store.flag_plugin(
            "crit@m",
            PluginFlag::Malicious,
            FlagSeverity::Critical,
            None,
        );

        let high_or_above = store.flags_above_severity(FlagSeverity::High);
        assert_eq!(high_or_above.len(), 2);
    }

    #[test]
    fn severity_ordering() {
        assert!(FlagSeverity::Critical > FlagSeverity::High);
        assert!(FlagSeverity::High > FlagSeverity::Medium);
        assert!(FlagSeverity::Medium > FlagSeverity::Low);
    }

    #[test]
    fn save_and_load_roundtrip() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("flagged-plugins.json");

        let mut store = FlagStore::new();
        store.flag_plugin(
            "a@m",
            PluginFlag::Security,
            FlagSeverity::High,
            Some("vuln".into()),
        );
        store.flag_plugin("b@m", PluginFlag::Delisted, FlagSeverity::Low, None);

        save_flags(&store, &path).expect("save");
        let loaded = load_flags(&path).expect("load");

        assert_eq!(loaded.len(), 2);
        assert!(loaded.is_flagged("a@m"));
        assert!(loaded.is_flagged("b@m"));
    }

    #[test]
    fn load_missing_returns_empty() {
        let path = PathBuf::from("/nonexistent/flagged-plugins.json");
        let store = load_flags(&path).expect("should succeed");
        assert!(store.is_empty());
    }

    #[test]
    fn flags_path_helper() {
        let dir = PathBuf::from("/home/user/.claude/plugins");
        let p = flags_path(&dir);
        assert_eq!(
            p,
            PathBuf::from("/home/user/.claude/plugins/flagged-plugins.json")
        );
    }
}
