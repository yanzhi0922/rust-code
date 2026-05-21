//! Orphaned plugin filter.
//!
//! Finds and cleans up orphaned plugin versions — plugins without a
//! marketplace source or plugins marked with `.orphaned_at` markers.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Marker file name for orphaned plugin versions.
pub const ORPHANED_AT_FILENAME: &str = ".orphaned_at";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// An orphaned plugin entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrphanedPlugin {
    /// Plugin directory path.
    pub path: PathBuf,
    /// Plugin name (if determinable).
    pub name: Option<String>,
    /// Whether the plugin has an `.orphaned_at` marker.
    pub has_marker: bool,
    /// Marketplace source (if known).
    pub marketplace: Option<String>,
}

/// Result of finding orphaned plugins.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrphanFilterResult {
    /// Orphaned plugins found.
    pub orphans: Vec<OrphanedPlugin>,
    /// Directories that were checked.
    pub checked_dirs: usize,
    /// Errors encountered during scanning.
    pub errors: Vec<String>,
}

// ---------------------------------------------------------------------------
// Core functions
// ---------------------------------------------------------------------------

/// Find orphaned plugins — plugins without a marketplace source.
///
/// Scans the plugin directory for plugins that are not associated with any
/// known marketplace.
pub fn find_orphaned_plugins(
    plugin_dir: &Path,
    known_marketplaces: &HashSet<String>,
) -> OrphanFilterResult {
    let mut orphans = Vec::new();
    let mut checked_dirs = 0;
    let mut errors = Vec::new();

    if !plugin_dir.exists() {
        return OrphanFilterResult {
            orphans,
            checked_dirs,
            errors,
        };
    }

    let entries = match std::fs::read_dir(plugin_dir) {
        Ok(entries) => entries,
        Err(e) => {
            errors.push(format!(
                "failed to read plugin dir {}: {e}",
                plugin_dir.display()
            ));
            return OrphanFilterResult {
                orphans,
                checked_dirs,
                errors,
            };
        }
    };

    for entry in entries.flatten() {
        if !entry
            .file_type()
            .is_ok_and(|ft: std::fs::FileType| ft.is_dir())
        {
            continue;
        }
        checked_dirs += 1;

        let dir_name = entry.file_name().to_string_lossy().to_string();

        // Check if this directory is associated with a known marketplace
        let is_known_marketplace = known_marketplaces.contains(&dir_name);

        if !is_known_marketplace {
            // Check for orphaned marker
            let marker_path = entry.path().join(ORPHANED_AT_FILENAME);
            let has_marker = marker_path.exists();

            // Try to determine plugin name
            let name = determine_plugin_name(&entry.path());

            orphans.push(OrphanedPlugin {
                path: entry.path(),
                name,
                has_marker,
                marketplace: None,
            });
        }
    }

    OrphanFilterResult {
        orphans,
        checked_dirs,
        errors,
    }
}

/// Clean orphaned plugins by removing them.
///
/// Removes plugin directories that are orphaned. Only removes plugins
/// that have the `.orphaned_at` marker (safety check).
pub fn clean_orphaned_plugins(
    orphans: &[OrphanedPlugin],
    dry_run: bool,
) -> Vec<Result<PathBuf, String>> {
    orphans
        .iter()
        .filter(|orphan| orphan.has_marker)
        .map(|orphan| {
            if dry_run {
                Ok(orphan.path.clone())
            } else {
                std::fs::remove_dir_all(&orphan.path)
                    .map(|_| orphan.path.clone())
                    .map_err(|e| format!("failed to remove {}: {e}", orphan.path.display()))
            }
        })
        .collect()
}

/// Determine the plugin name from a plugin directory.
fn determine_plugin_name(dir: &Path) -> Option<String> {
    let manifest_path = dir
        .join(crate::PLUGIN_MANIFEST_DIR)
        .join(crate::PLUGIN_MANIFEST_FILE);

    if manifest_path.exists()
        && let Ok(content) = std::fs::read_to_string(&manifest_path)
        && let Ok(value) = serde_json::from_str::<serde_json::Value>(&content)
    {
        return value
            .get("name")
            .and_then(|v| v.as_str())
            .map(|s| s.to_owned());
    }
    None
}

/// Check if a specific directory is orphaned.
pub fn is_orphaned(plugin_path: &Path) -> bool {
    plugin_path.join(ORPHANED_AT_FILENAME).exists()
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

    #[test]
    fn find_orphaned_plugins_empty_dir() {
        let temp = ok(tempdir());
        let known = HashSet::new();
        let result = find_orphaned_plugins(temp.path(), &known);
        assert!(result.orphans.is_empty());
        assert_eq!(result.checked_dirs, 0);
    }

    #[test]
    fn find_orphaned_plugins_finds_unknown_dirs() {
        let temp = ok(tempdir());
        fs::create_dir_all(temp.path().join("unknown-plugin")).expect("create dir");
        fs::create_dir_all(temp.path().join("known-mkt")).expect("create dir");

        let mut known = HashSet::new();
        known.insert("known-mkt".to_owned());

        let result = find_orphaned_plugins(temp.path(), &known);
        assert_eq!(result.orphans.len(), 1);
        assert_eq!(result.orphans[0].path, temp.path().join("unknown-plugin"));
    }

    #[test]
    fn find_orphaned_plugins_detects_marker() {
        let temp = ok(tempdir());
        let orphan_dir = temp.path().join("orphaned");
        fs::create_dir_all(&orphan_dir).expect("create dir");
        fs::write(orphan_dir.join(ORPHANED_AT_FILENAME), "2024-01-01").expect("write marker");

        let result = find_orphaned_plugins(temp.path(), &HashSet::new());
        assert_eq!(result.orphans.len(), 1);
        assert!(result.orphans[0].has_marker);
    }

    #[test]
    fn clean_orphaned_plugins_dry_run() {
        let temp = ok(tempdir());
        let orphan_dir = temp.path().join("orphaned");
        fs::create_dir_all(&orphan_dir).expect("create dir");
        fs::write(orphan_dir.join(ORPHANED_AT_FILENAME), "2024-01-01").expect("write marker");

        let orphans = vec![OrphanedPlugin {
            path: orphan_dir.clone(),
            name: Some("test".to_owned()),
            has_marker: true,
            marketplace: None,
        }];

        let results = clean_orphaned_plugins(&orphans, true);
        assert!(results.len() == 1);
        assert!(results[0].is_ok());
        assert!(orphan_dir.exists()); // dry run should not delete
    }

    #[test]
    fn clean_orphaned_plugins_skips_no_marker() {
        let temp = ok(tempdir());
        let no_marker_dir = temp.path().join("no-marker");
        fs::create_dir_all(&no_marker_dir).expect("create dir");

        let orphans = vec![OrphanedPlugin {
            path: no_marker_dir.clone(),
            name: None,
            has_marker: false,
            marketplace: None,
        }];

        let results = clean_orphaned_plugins(&orphans, false);
        assert!(results.is_empty()); // should skip non-marker orphans
    }

    #[test]
    fn is_orphaned_checks_marker() {
        let temp = ok(tempdir());
        assert!(!is_orphaned(temp.path()));

        fs::write(temp.path().join(ORPHANED_AT_FILENAME), "2024-01-01").expect("write marker");
        assert!(is_orphaned(temp.path()));
    }
}
