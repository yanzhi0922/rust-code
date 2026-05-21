//! Plugin reconciler — makes installed state consistent with declared intent.
//!
//! Compares declared intent (settings) against materialized state (installed
//! plugins) and produces a [`ReconcilePlan`] of actions needed.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A marketplace source configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "lowercase")]
pub enum MarketplaceSource {
    /// GitHub repository source.
    Github { repo: String },
    /// URL source.
    Url { url: String },
    /// Git repository source.
    Git {
        url: String,
        #[serde(default)]
        ref_: Option<String>,
    },
    /// Local directory source.
    Directory { path: String },
    /// Local file source.
    File { path: String },
}

/// A declared marketplace from settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeclaredMarketplace {
    /// Source of the marketplace.
    pub source: MarketplaceSource,
    /// Whether the source is a fallback.
    #[serde(default)]
    pub source_is_fallback: bool,
}

/// A known marketplace from the materialized state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnownMarketplace {
    /// Source of the marketplace.
    pub source: MarketplaceSource,
}

/// Result of comparing declared vs materialized state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarketplaceDiff {
    /// Declared in settings, absent from known_marketplaces.json.
    pub missing: Vec<String>,
    /// Present in both, but settings source ≠ JSON source.
    pub source_changed: Vec<SourceChangedEntry>,
    /// Present in both, sources match.
    pub up_to_date: Vec<String>,
}

/// Entry for a marketplace whose source has changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceChangedEntry {
    /// Marketplace name.
    pub name: String,
    /// The declared (settings) source.
    pub declared_source: MarketplaceSource,
    /// The materialized (JSON) source.
    pub materialized_source: MarketplaceSource,
}

/// Actions the reconciler can take.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReconcileAction {
    /// Install a new marketplace.
    Install,
    /// Remove an existing marketplace.
    Remove,
    /// Update an existing marketplace.
    Update,
    /// Keep as-is (already up to date).
    Keep,
}

/// A single reconciliation step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReconcileStep {
    /// Marketplace name.
    pub name: String,
    /// Action to take.
    pub action: ReconcileAction,
    /// Source for the action (if applicable).
    pub source: Option<MarketplaceSource>,
}

/// A complete reconciliation plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReconcilePlan {
    /// Steps to execute.
    pub steps: Vec<ReconcileStep>,
    /// Number of marketplaces already up to date.
    pub up_to_date_count: usize,
    /// Number of marketplaces to install.
    pub install_count: usize,
    /// Number of marketplaces to update.
    pub update_count: usize,
    /// Number of marketplaces to remove.
    pub remove_count: usize,
}

/// Progress event during reconciliation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ReconcileProgressEvent {
    /// Installing a marketplace.
    Installing {
        name: String,
        action: ReconcileAction,
        index: usize,
        total: usize,
    },
    /// Completed installation.
    Installed {
        name: String,
        already_materialized: bool,
    },
    /// Failed to install.
    Failed { name: String, error: String },
}

// ---------------------------------------------------------------------------
// Core functions
// ---------------------------------------------------------------------------

/// Compare declared intent against materialized state.
///
/// Produces a diff showing which marketplaces are missing, which have changed
/// sources, and which are up to date.
pub fn diff_marketplaces(
    declared: &HashMap<String, DeclaredMarketplace>,
    materialized: &HashMap<String, KnownMarketplace>,
) -> MarketplaceDiff {
    let mut missing = Vec::new();
    let mut source_changed = Vec::new();
    let mut up_to_date = Vec::new();

    for (name, intent) in declared {
        let state = materialized.get(name);
        if state.is_none() {
            missing.push(name.clone());
        } else if intent.source_is_fallback {
            up_to_date.push(name.clone());
        } else if let Some(known) = state {
            if intent.source != known.source {
                source_changed.push(SourceChangedEntry {
                    name: name.clone(),
                    declared_source: intent.source.clone(),
                    materialized_source: known.source.clone(),
                });
            } else {
                up_to_date.push(name.clone());
            }
        }
    }

    MarketplaceDiff {
        missing,
        source_changed,
        up_to_date,
    }
}

/// Build a reconciliation plan from a diff.
///
/// The plan describes what actions to take to make the materialized state
/// match the declared intent.
pub fn build_reconcile_plan(
    declared: &HashMap<String, DeclaredMarketplace>,
    materialized: &HashSet<String>,
) -> ReconcilePlan {
    let mut steps = Vec::new();
    let mut install_count = 0usize;
    let update_count = 0usize;
    let mut remove_count = 0usize;
    let mut up_to_date_count = 0usize;

    for (name, intent) in declared {
        if materialized.contains(name) {
            // Already present — check if source changed
            // For simplicity, assume up-to-date if present
            steps.push(ReconcileStep {
                name: name.clone(),
                action: ReconcileAction::Keep,
                source: Some(intent.source.clone()),
            });
            up_to_date_count += 1;
        } else {
            steps.push(ReconcileStep {
                name: name.clone(),
                action: ReconcileAction::Install,
                source: Some(intent.source.clone()),
            });
            install_count += 1;
        }
    }

    // Find marketplaces in materialized but not in declared (orphans to remove)
    let declared_names: HashSet<String> = declared.keys().cloned().collect();
    for name in materialized {
        if !declared_names.contains(name) {
            steps.push(ReconcileStep {
                name: name.clone(),
                action: ReconcileAction::Remove,
                source: None,
            });
            remove_count += 1;
        }
    }

    ReconcilePlan {
        steps,
        up_to_date_count,
        install_count,
        update_count,
        remove_count,
    }
}

/// Reconcile installed vs desired plugin state.
///
/// Takes the current set of installed plugin names and the desired set,
/// returning actions to take.
pub fn reconcile_plugins(installed: &HashSet<String>, desired: &HashSet<String>) -> ReconcilePlan {
    let mut steps = Vec::new();
    let mut install_count = 0usize;
    let mut remove_count = 0usize;
    let mut up_to_date_count = 0usize;

    // Plugins to install
    for name in desired.difference(installed) {
        steps.push(ReconcileStep {
            name: name.clone(),
            action: ReconcileAction::Install,
            source: None,
        });
        install_count += 1;
    }

    // Plugins to keep
    for name in installed.intersection(desired) {
        steps.push(ReconcileStep {
            name: name.clone(),
            action: ReconcileAction::Keep,
            source: None,
        });
        up_to_date_count += 1;
    }

    // Plugins to remove
    for name in installed.difference(desired) {
        steps.push(ReconcileStep {
            name: name.clone(),
            action: ReconcileAction::Remove,
            source: None,
        });
        remove_count += 1;
    }

    ReconcilePlan {
        steps,
        up_to_date_count,
        install_count,
        update_count: 0,
        remove_count,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_declared(entries: &[(&str, &str)]) -> HashMap<String, DeclaredMarketplace> {
        entries
            .iter()
            .map(|(name, repo)| {
                (
                    (*name).to_owned(),
                    DeclaredMarketplace {
                        source: MarketplaceSource::Github {
                            repo: (*repo).to_owned(),
                        },
                        source_is_fallback: false,
                    },
                )
            })
            .collect()
    }

    fn make_materialized(entries: &[(&str, &str)]) -> HashMap<String, KnownMarketplace> {
        entries
            .iter()
            .map(|(name, repo)| {
                (
                    (*name).to_owned(),
                    KnownMarketplace {
                        source: MarketplaceSource::Github {
                            repo: (*repo).to_owned(),
                        },
                    },
                )
            })
            .collect()
    }

    #[test]
    fn diff_marketplaces_all_up_to_date() {
        let declared = make_declared(&[("mkt-a", "org/repo-a")]);
        let materialized = make_materialized(&[("mkt-a", "org/repo-a")]);
        let diff = diff_marketplaces(&declared, &materialized);
        assert!(diff.missing.is_empty());
        assert!(diff.source_changed.is_empty());
        assert_eq!(diff.up_to_date, vec!["mkt-a"]);
    }

    #[test]
    fn diff_marketplaces_missing() {
        let declared = make_declared(&[("mkt-a", "org/repo-a")]);
        let materialized = HashMap::new();
        let diff = diff_marketplaces(&declared, &materialized);
        assert_eq!(diff.missing, vec!["mkt-a"]);
        assert!(diff.up_to_date.is_empty());
    }

    #[test]
    fn diff_marketplaces_source_changed() {
        let declared = make_declared(&[("mkt-a", "org/repo-a")]);
        let materialized = make_materialized(&[("mkt-a", "org/repo-b")]);
        let diff = diff_marketplaces(&declared, &materialized);
        assert!(diff.missing.is_empty());
        assert_eq!(diff.source_changed.len(), 1);
        assert_eq!(diff.source_changed[0].name, "mkt-a");
    }

    #[test]
    fn diff_marketplaces_fallback_is_up_to_date() {
        let mut declared = make_declared(&[("mkt-a", "org/repo-a")]);
        declared.get_mut("mkt-a").expect("entry").source_is_fallback = true;
        let materialized = make_materialized(&[("mkt-a", "org/different")]);
        let diff = diff_marketplaces(&declared, &materialized);
        assert!(diff.source_changed.is_empty());
        assert_eq!(diff.up_to_date, vec!["mkt-a"]);
    }

    #[test]
    fn build_reconcile_plan_installs_missing() {
        let declared = make_declared(&[("mkt-a", "org/repo-a"), ("mkt-b", "org/repo-b")]);
        let materialized = HashSet::from(["mkt-a".to_owned()]);
        let plan = build_reconcile_plan(&declared, &materialized);
        assert_eq!(plan.install_count, 1);
        assert_eq!(plan.up_to_date_count, 1);
    }

    #[test]
    fn build_reconcile_plan_removes_orphans() {
        let declared = make_declared(&[("mkt-a", "org/repo-a")]);
        let materialized = HashSet::from(["mkt-a".to_owned(), "orphan-mkt".to_owned()]);
        let plan = build_reconcile_plan(&declared, &materialized);
        assert_eq!(plan.remove_count, 1);
    }

    #[test]
    fn reconcile_plugins_basic() {
        let installed: HashSet<String> = ["a".to_owned(), "b".to_owned()].into_iter().collect();
        let desired: HashSet<String> = ["b".to_owned(), "c".to_owned()].into_iter().collect();
        let plan = reconcile_plugins(&installed, &desired);
        assert_eq!(plan.install_count, 1);
        assert_eq!(plan.remove_count, 1);
        assert_eq!(plan.up_to_date_count, 1);

        let install_names: Vec<&str> = plan
            .steps
            .iter()
            .filter(|s| s.action == ReconcileAction::Install)
            .map(|s| s.name.as_str())
            .collect();
        assert!(install_names.contains(&"c"));

        let remove_names: Vec<&str> = plan
            .steps
            .iter()
            .filter(|s| s.action == ReconcileAction::Remove)
            .map(|s| s.name.as_str())
            .collect();
        assert!(remove_names.contains(&"a"));
    }

    #[test]
    fn reconcile_plugins_all_match() {
        let installed: HashSet<String> = ["a".to_owned()].into_iter().collect();
        let desired: HashSet<String> = ["a".to_owned()].into_iter().collect();
        let plan = reconcile_plugins(&installed, &desired);
        assert_eq!(plan.install_count, 0);
        assert_eq!(plan.remove_count, 0);
        assert_eq!(plan.up_to_date_count, 1);
    }
}
