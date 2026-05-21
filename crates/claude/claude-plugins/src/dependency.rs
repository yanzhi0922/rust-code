//! Plugin dependency resolution — pure functions, no I/O.
//!
//! Semantics are `apt`-style: a dependency is a *presence guarantee*, not a
//! module graph. Plugin A depending on Plugin B means "B's namespaced
//! components (MCP servers, commands, agents) must be available when A runs."
//!
//! Two entry points:
//! - [`resolve_dependency_closure`] — install-time DFS walk, cycle detection
//! - [`verify_and_demote`] — load-time fixed-point check, demotes plugins
//!   with unsatisfied deps

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::identifier::PluginIdentifier;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Plugin ID string in `"name@marketplace"` format.
pub type PluginId = String;

/// Minimal shape the resolver needs from a marketplace lookup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DependencyLookupResult {
    /// Entries may be bare names; [`qualify_dependency`] normalizes them.
    #[serde(default)]
    pub dependencies: Vec<String>,
}

/// Result of dependency resolution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ResolutionResult {
    /// Successful resolution with the full transitive closure.
    Ok {
        /// All plugin IDs to install, in dependency order.
        closure: Vec<PluginId>,
    },
    /// Circular dependency detected.
    Cycle {
        /// The chain forming the cycle.
        chain: Vec<PluginId>,
    },
    /// A required dependency was not found.
    NotFound {
        /// The missing dependency.
        missing: PluginId,
        /// The plugin that required it.
        required_by: PluginId,
    },
    /// Cross-marketplace dependency blocked.
    CrossMarketplace {
        /// The dependency from another marketplace.
        dependency: PluginId,
        /// The plugin that required it.
        required_by: PluginId,
    },
}

/// A plugin with its source identifier for dependency checking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedPluginRef {
    /// Plugin source ID (`"name@marketplace"`).
    pub source: String,
    /// Plugin display name.
    pub name: String,
    /// Whether the plugin is enabled.
    pub enabled: bool,
    /// Declared dependencies.
    pub dependencies: Vec<String>,
}

/// A dependency error from verification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DependencyError {
    /// Type of error.
    pub error_type: String,
    /// Source of the plugin with the error.
    pub source: String,
    /// Plugin name.
    pub plugin: String,
    /// The unsatisfied dependency.
    pub dependency: String,
    /// Reason for the error.
    pub reason: String,
}

/// Result of verify-and-demote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyAndDemoteResult {
    /// Set of plugin IDs to demote.
    pub demoted: HashSet<String>,
    /// Errors collected during verification.
    pub errors: Vec<DependencyError>,
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Synthetic marketplace sentinel for `--plugin-dir` plugins.
const INLINE_MARKETPLACE: &str = "inline";

/// Normalize a dependency reference to fully-qualified `"name@marketplace"` form.
///
/// Bare names (no `@`) inherit the marketplace of the plugin declaring them.
/// If the declaring plugin is `@inline`, bare deps are returned unchanged.
pub fn qualify_dependency(dep: &str, declaring_plugin_id: &str) -> String {
    let parsed = PluginIdentifier::parse(dep);
    if parsed.marketplace.is_some() {
        return dep.to_owned();
    }
    let declaring = PluginIdentifier::parse(declaring_plugin_id);
    if declaring.marketplace.is_none()
        || declaring
            .marketplace
            .as_ref()
            .is_some_and(|m| m == INLINE_MARKETPLACE)
    {
        return dep.to_owned();
    }
    format!(
        "{}@{}",
        parsed.name,
        declaring.marketplace.as_ref().expect("checked above")
    )
}

/// Walk the transitive dependency closure of `root_id` via DFS.
///
/// The returned `closure` ALWAYS contains `root_id`, plus every transitive
/// dependency that is NOT in `already_enabled`.
///
/// # Arguments
/// * `root_id` — Root plugin to resolve from (`"name@marketplace"`).
/// * `lookup` — Synchronous lookup returning `Some(DependencyLookupResult)` or `None`.
/// * `already_enabled` — Plugin IDs to skip (deps only, root is never skipped).
/// * `allowed_cross_marketplaces` — Marketplace names the root trusts.
///
/// # Errors
/// Returns a `ResolutionResult` variant indicating the type of failure.
pub fn resolve_dependency_closure(
    root_id: &str,
    lookup: &dyn Fn(&str) -> Option<DependencyLookupResult>,
    already_enabled: &HashSet<PluginId>,
    allowed_cross_marketplaces: &HashSet<String>,
) -> ResolutionResult {
    let root_parsed = PluginIdentifier::parse(root_id);
    let root_marketplace = root_parsed.marketplace.as_deref();
    let mut closure: Vec<PluginId> = Vec::new();
    let mut visited: HashSet<PluginId> = HashSet::new();
    let mut stack: Vec<PluginId> = Vec::new();

    struct WalkContext<'a> {
        root_id: &'a str,
        root_marketplace: Option<&'a str>,
        lookup: &'a dyn Fn(&str) -> Option<DependencyLookupResult>,
        already_enabled: &'a HashSet<PluginId>,
        allowed_cross_marketplaces: &'a HashSet<String>,
    }

    struct WalkState<'a> {
        closure: &'a mut Vec<PluginId>,
        visited: &'a mut HashSet<PluginId>,
        stack: &'a mut Vec<PluginId>,
    }

    fn walk(
        id: &str,
        required_by: &str,
        ctx: &WalkContext<'_>,
        state: &mut WalkState<'_>,
    ) -> Option<ResolutionResult> {
        // Skip already-enabled DEPENDENCIES (not root)
        if id != ctx.root_id && ctx.already_enabled.contains(id) {
            return None;
        }

        // Security: block auto-install across marketplace boundaries
        let id_parsed = PluginIdentifier::parse(id);
        let id_marketplace = id_parsed.marketplace.as_deref();
        if id_marketplace != ctx.root_marketplace {
            let is_allowed =
                id_marketplace.is_some_and(|m| ctx.allowed_cross_marketplaces.contains(m));
            if !is_allowed {
                return Some(ResolutionResult::CrossMarketplace {
                    dependency: id.to_owned(),
                    required_by: required_by.to_owned(),
                });
            }
        }

        if state.stack.contains(&id.to_owned()) {
            let mut chain = state.stack.clone();
            chain.push(id.to_owned());
            return Some(ResolutionResult::Cycle { chain });
        }
        if state.visited.contains(id) {
            return None;
        }
        state.visited.insert(id.to_owned());

        let entry = match (ctx.lookup)(id) {
            Some(e) => e,
            None => {
                return Some(ResolutionResult::NotFound {
                    missing: id.to_owned(),
                    required_by: required_by.to_owned(),
                });
            }
        };

        state.stack.push(id.to_owned());
        for raw_dep in &entry.dependencies {
            let dep = qualify_dependency(raw_dep, id);
            if let Some(err) = walk(&dep, id, ctx, state) {
                return Some(err);
            }
        }
        state.stack.pop();

        state.closure.push(id.to_owned());
        None
    }

    let ctx = WalkContext {
        root_id,
        root_marketplace,
        lookup,
        already_enabled,
        allowed_cross_marketplaces,
    };
    let mut state = WalkState {
        closure: &mut closure,
        visited: &mut visited,
        stack: &mut stack,
    };

    if let Some(err) = walk(root_id, root_id, &ctx, &mut state) {
        return err;
    }

    ResolutionResult::Ok { closure }
}

/// Detect circular dependencies in a set of plugins.
///
/// Returns a list of cycles found. Each cycle is a list of plugin IDs.
pub fn detect_circular_dependencies(plugins: &[LoadedPluginRef]) -> Vec<Vec<PluginId>> {
    let mut cycles: Vec<Vec<PluginId>> = Vec::new();
    let mut visited: HashSet<PluginId> = HashSet::new();
    let mut rec_stack: HashSet<PluginId> = HashSet::new();
    let mut path: Vec<PluginId> = Vec::new();

    let source_set: HashSet<PluginId> = plugins.iter().map(|p| p.source.clone()).collect();

    fn dfs(
        plugin: &LoadedPluginRef,
        plugins_map: &HashMap<PluginId, &LoadedPluginRef>,
        visited: &mut HashSet<PluginId>,
        rec_stack: &mut HashSet<PluginId>,
        path: &mut Vec<PluginId>,
        cycles: &mut Vec<Vec<PluginId>>,
        source_set: &HashSet<PluginId>,
    ) {
        if visited.contains(&plugin.source) {
            if rec_stack.contains(&plugin.source) {
                // Found a cycle
                if let Some(start) = path.iter().position(|p| p == &plugin.source) {
                    cycles.push(path[start..].to_vec());
                }
            }
            return;
        }

        visited.insert(plugin.source.clone());
        rec_stack.insert(plugin.source.clone());
        path.push(plugin.source.clone());

        for raw_dep in &plugin.dependencies {
            let dep = qualify_dependency(raw_dep, &plugin.source);
            if !source_set.contains(&dep) {
                continue;
            }
            if let Some(dep_plugin) = plugins_map.get(&dep) {
                dfs(
                    dep_plugin,
                    plugins_map,
                    visited,
                    rec_stack,
                    path,
                    cycles,
                    source_set,
                );
            }
        }

        path.pop();
        rec_stack.remove(&plugin.source);
    }

    let plugins_map: HashMap<PluginId, &LoadedPluginRef> =
        plugins.iter().map(|p| (p.source.clone(), p)).collect();

    for plugin in plugins {
        if !visited.contains(&plugin.source) {
            dfs(
                plugin,
                &plugins_map,
                &mut visited,
                &mut rec_stack,
                &mut path,
                &mut cycles,
                &source_set,
            );
        }
    }

    cycles
}

/// Load-time safety net: for each enabled plugin, verify all manifest
/// dependencies are also in the enabled set. Demote any that fail.
///
/// Fixed-point loop: demoting plugin A may break plugin B that depends on A,
/// so we iterate until nothing changes.
///
/// Does NOT mutate input. Returns the set of plugin IDs to demote.
pub fn verify_and_demote(plugins: &[LoadedPluginRef]) -> VerifyAndDemoteResult {
    let known: HashSet<PluginId> = plugins.iter().map(|p| p.source.clone()).collect();
    let mut enabled: HashSet<PluginId> = plugins
        .iter()
        .filter(|p| p.enabled)
        .map(|p| p.source.clone())
        .collect();

    // Name-only indexes for bare deps from @inline plugins
    let known_by_name: HashSet<String> = plugins
        .iter()
        .map(|p| PluginIdentifier::parse(&p.source).name)
        .collect();
    let mut enabled_by_name: HashMap<String, usize> = HashMap::new();
    for id in &enabled {
        let name = PluginIdentifier::parse(id).name;
        *enabled_by_name.entry(name).or_insert(0) += 1;
    }

    let mut errors: Vec<DependencyError> = Vec::new();

    let mut changed = true;
    while changed {
        changed = false;
        for p in plugins {
            if !enabled.contains(&p.source) {
                continue;
            }
            for raw_dep in &p.dependencies {
                let dep = qualify_dependency(raw_dep, &p.source);
                let is_bare = PluginIdentifier::parse(&dep).marketplace.is_none();
                let satisfied = if is_bare {
                    enabled_by_name.get(&dep).copied().unwrap_or(0) > 0
                } else {
                    enabled.contains(&dep)
                };
                if !satisfied {
                    enabled.remove(&p.source);
                    let count = enabled_by_name.get(&p.name).copied().unwrap_or(0);
                    if count <= 1 {
                        enabled_by_name.remove(&p.name);
                    } else {
                        enabled_by_name.insert(p.name.clone(), count - 1);
                    }
                    let reason = if is_bare {
                        if known_by_name.contains(&dep) {
                            "not-enabled"
                        } else {
                            "not-found"
                        }
                    } else if known.contains(&dep) {
                        "not-enabled"
                    } else {
                        "not-found"
                    };
                    errors.push(DependencyError {
                        error_type: "dependency-unsatisfied".to_owned(),
                        source: p.source.clone(),
                        plugin: p.name.clone(),
                        dependency: dep,
                        reason: reason.to_owned(),
                    });
                    changed = true;
                    break;
                }
            }
        }
    }

    let demoted: HashSet<String> = plugins
        .iter()
        .filter(|p| p.enabled && !enabled.contains(&p.source))
        .map(|p| p.source.clone())
        .collect();

    VerifyAndDemoteResult { demoted, errors }
}

/// Find all enabled plugins that declare `plugin_id` as a dependency.
///
/// Used to warn on uninstall/disable ("required by: X, Y").
pub fn find_reverse_dependents(plugin_id: &str, plugins: &[LoadedPluginRef]) -> Vec<String> {
    let target_name = PluginIdentifier::parse(plugin_id).name;
    plugins
        .iter()
        .filter(|p| {
            p.enabled
                && p.source != plugin_id
                && p.dependencies.iter().any(|d| {
                    let qualified = qualify_dependency(d, &p.source);
                    let parsed = PluginIdentifier::parse(&qualified);
                    if parsed.marketplace.is_some() {
                        qualified == plugin_id
                    } else {
                        parsed.name == target_name
                    }
                })
        })
        .map(|p| p.name.clone())
        .collect()
}

/// Format the "(+ N dependencies)" suffix for install success messages.
pub fn format_dependency_count_suffix(installed_deps: &[String]) -> String {
    if installed_deps.is_empty() {
        return String::new();
    }
    let n = installed_deps.len();
    format!(
        " (+ {n} {})",
        if n == 1 { "dependency" } else { "dependencies" }
    )
}

/// Format the "warning: required by X, Y" suffix for uninstall/disable results.
pub fn format_reverse_dependents_suffix(rdeps: &[String]) -> String {
    if rdeps.is_empty() {
        return String::new();
    }
    format!(" — warning: required by {}", rdeps.join(", "))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_lookup(entries: &[(&str, &[&str])]) -> impl Fn(&str) -> Option<DependencyLookupResult> {
        let map: HashMap<String, DependencyLookupResult> = entries
            .iter()
            .map(|(id, deps)| {
                (
                    (*id).to_owned(),
                    DependencyLookupResult {
                        dependencies: deps.iter().map(|d| (*d).to_owned()).collect(),
                    },
                )
            })
            .collect();
        move |id: &str| map.get(id).cloned()
    }

    #[test]
    fn qualify_dependency_bare_inherits_marketplace() {
        assert_eq!(qualify_dependency("dep-a", "root@mkt"), "dep-a@mkt");
    }

    #[test]
    fn qualify_dependency_already_qualified() {
        assert_eq!(qualify_dependency("dep-a@other", "root@mkt"), "dep-a@other");
    }

    #[test]
    fn qualify_dependency_inline_stays_bare() {
        assert_eq!(qualify_dependency("dep-a", "root@inline"), "dep-a");
    }

    #[test]
    fn qualify_dependency_no_marketplace_stays_bare() {
        assert_eq!(qualify_dependency("dep-a", "root"), "dep-a");
    }

    #[test]
    fn resolve_closure_simple() {
        let lookup = make_lookup(&[("root@mkt", &["dep-a@mkt"]), ("dep-a@mkt", &[])]);
        let result =
            resolve_dependency_closure("root@mkt", &lookup, &HashSet::new(), &HashSet::new());
        match result {
            ResolutionResult::Ok { closure } => {
                assert!(closure.contains(&"dep-a@mkt".to_owned()));
                assert!(closure.contains(&"root@mkt".to_owned()));
                assert_eq!(closure.len(), 2);
            }
            _ => panic!("expected Ok, got {result:?}"),
        }
    }

    #[test]
    fn resolve_closure_skips_already_enabled() {
        let lookup = make_lookup(&[("root@mkt", &["dep-a@mkt"]), ("dep-a@mkt", &[])]);
        let mut already = HashSet::new();
        already.insert("dep-a@mkt".to_owned());
        let result = resolve_dependency_closure("root@mkt", &lookup, &already, &HashSet::new());
        match result {
            ResolutionResult::Ok { closure } => {
                assert!(closure.contains(&"root@mkt".to_owned()));
                assert!(
                    !closure.contains(&"dep-a@mkt".to_owned()),
                    "already-enabled dep should be skipped"
                );
            }
            _ => panic!("expected Ok, got {result:?}"),
        }
    }

    #[test]
    fn resolve_closure_never_skips_root() {
        let lookup = make_lookup(&[("root@mkt", &[])]);
        let mut already = HashSet::new();
        already.insert("root@mkt".to_owned());
        let result = resolve_dependency_closure("root@mkt", &lookup, &already, &HashSet::new());
        match result {
            ResolutionResult::Ok { closure } => {
                assert!(
                    closure.contains(&"root@mkt".to_owned()),
                    "root should never be skipped"
                );
            }
            _ => panic!("expected Ok, got {result:?}"),
        }
    }

    #[test]
    fn resolve_closure_detects_cycle() {
        let lookup = make_lookup(&[("a@mkt", &["b@mkt"]), ("b@mkt", &["a@mkt"])]);
        let result = resolve_dependency_closure("a@mkt", &lookup, &HashSet::new(), &HashSet::new());
        assert!(
            matches!(result, ResolutionResult::Cycle { .. }),
            "expected Cycle, got {result:?}"
        );
    }

    #[test]
    fn resolve_closure_detects_not_found() {
        let lookup = make_lookup(&[("root@mkt", &["missing@mkt"])]);
        let result =
            resolve_dependency_closure("root@mkt", &lookup, &HashSet::new(), &HashSet::new());
        assert!(
            matches!(result, ResolutionResult::NotFound { .. }),
            "expected NotFound, got {result:?}"
        );
    }

    #[test]
    fn resolve_closure_blocks_cross_marketplace() {
        let lookup = make_lookup(&[("root@mkt", &["dep@other"]), ("dep@other", &[])]);
        let result =
            resolve_dependency_closure("root@mkt", &lookup, &HashSet::new(), &HashSet::new());
        assert!(
            matches!(result, ResolutionResult::CrossMarketplace { .. }),
            "expected CrossMarketplace, got {result:?}"
        );
    }

    #[test]
    fn resolve_closure_allows_cross_marketplace_in_allowlist() {
        let lookup = make_lookup(&[("root@mkt", &["dep@other"]), ("dep@other", &[])]);
        let mut allowed = HashSet::new();
        allowed.insert("other".to_owned());
        let result = resolve_dependency_closure("root@mkt", &lookup, &HashSet::new(), &allowed);
        assert!(
            matches!(result, ResolutionResult::Ok { .. }),
            "expected Ok, got {result:?}"
        );
    }

    #[test]
    fn verify_and_demote_removes_plugins_with_missing_deps() {
        let plugins = vec![
            LoadedPluginRef {
                source: "a@mkt".to_owned(),
                name: "a".to_owned(),
                enabled: true,
                dependencies: vec!["b@mkt".to_owned()],
            },
            LoadedPluginRef {
                source: "b@mkt".to_owned(),
                name: "b".to_owned(),
                enabled: false,
                dependencies: vec![],
            },
        ];
        let result = verify_and_demote(&plugins);
        assert!(result.demoted.contains("a@mkt"));
        assert_eq!(result.errors.len(), 1);
        assert_eq!(result.errors[0].reason, "not-enabled");
    }

    #[test]
    fn verify_and_demote_keeps_satisfied_plugins() {
        let plugins = vec![
            LoadedPluginRef {
                source: "a@mkt".to_owned(),
                name: "a".to_owned(),
                enabled: true,
                dependencies: vec!["b@mkt".to_owned()],
            },
            LoadedPluginRef {
                source: "b@mkt".to_owned(),
                name: "b".to_owned(),
                enabled: true,
                dependencies: vec![],
            },
        ];
        let result = verify_and_demote(&plugins);
        assert!(result.demoted.is_empty());
        assert!(result.errors.is_empty());
    }

    #[test]
    fn verify_and_demote_fixed_point() {
        // a depends on b, b depends on c, c is disabled
        // demoting c should cascade to b then a
        let plugins = vec![
            LoadedPluginRef {
                source: "a@mkt".to_owned(),
                name: "a".to_owned(),
                enabled: true,
                dependencies: vec!["b@mkt".to_owned()],
            },
            LoadedPluginRef {
                source: "b@mkt".to_owned(),
                name: "b".to_owned(),
                enabled: true,
                dependencies: vec!["c@mkt".to_owned()],
            },
            LoadedPluginRef {
                source: "c@mkt".to_owned(),
                name: "c".to_owned(),
                enabled: false,
                dependencies: vec![],
            },
        ];
        let result = verify_and_demote(&plugins);
        assert_eq!(result.demoted.len(), 2);
        assert!(result.demoted.contains("a@mkt"));
        assert!(result.demoted.contains("b@mkt"));
    }

    #[test]
    fn find_reverse_dependents_works() {
        let plugins = vec![
            LoadedPluginRef {
                source: "a@mkt".to_owned(),
                name: "a".to_owned(),
                enabled: true,
                dependencies: vec!["b@mkt".to_owned()],
            },
            LoadedPluginRef {
                source: "b@mkt".to_owned(),
                name: "b".to_owned(),
                enabled: true,
                dependencies: vec![],
            },
            LoadedPluginRef {
                source: "c@mkt".to_owned(),
                name: "c".to_owned(),
                enabled: true,
                dependencies: vec!["b@mkt".to_owned()],
            },
        ];
        let rdeps = find_reverse_dependents("b@mkt", &plugins);
        assert!(rdeps.contains(&"a".to_owned()));
        assert!(rdeps.contains(&"c".to_owned()));
    }

    #[test]
    fn detect_circular_dependencies_finds_cycle() {
        let plugins = vec![
            LoadedPluginRef {
                source: "a@mkt".to_owned(),
                name: "a".to_owned(),
                enabled: true,
                dependencies: vec!["b@mkt".to_owned()],
            },
            LoadedPluginRef {
                source: "b@mkt".to_owned(),
                name: "b".to_owned(),
                enabled: true,
                dependencies: vec!["a@mkt".to_owned()],
            },
        ];
        let cycles = detect_circular_dependencies(&plugins);
        assert!(!cycles.is_empty(), "should detect at least one cycle");
    }

    #[test]
    fn detect_circular_dependencies_no_cycle() {
        let plugins = vec![
            LoadedPluginRef {
                source: "a@mkt".to_owned(),
                name: "a".to_owned(),
                enabled: true,
                dependencies: vec!["b@mkt".to_owned()],
            },
            LoadedPluginRef {
                source: "b@mkt".to_owned(),
                name: "b".to_owned(),
                enabled: true,
                dependencies: vec![],
            },
        ];
        let cycles = detect_circular_dependencies(&plugins);
        assert!(cycles.is_empty(), "should find no cycles");
    }

    #[test]
    fn format_dependency_count_suffix_works() {
        assert_eq!(format_dependency_count_suffix(&[]), "");
        assert_eq!(
            format_dependency_count_suffix(&["a".to_owned()]),
            " (+ 1 dependency)"
        );
        assert_eq!(
            format_dependency_count_suffix(&["a".to_owned(), "b".to_owned()]),
            " (+ 2 dependencies)"
        );
    }

    #[test]
    fn format_reverse_dependents_suffix_works() {
        assert_eq!(format_reverse_dependents_suffix(&[]), "");
        assert_eq!(
            format_reverse_dependents_suffix(&["a".to_owned()]),
            " — warning: required by a"
        );
        assert_eq!(
            format_reverse_dependents_suffix(&["a".to_owned(), "b".to_owned()]),
            " — warning: required by a, b"
        );
    }
}
