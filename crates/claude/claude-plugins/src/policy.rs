//! Plugin policy checks backed by managed settings.
//!
//! Rust equivalent of `pluginPolicy.ts`. Provides policy enforcement for
//! plugin installation and activation, supporting multiple policy sources
//! (user, project, enterprise) with merge semantics.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

/// Errors that can occur during policy operations.
#[derive(Debug, Error)]
pub enum PolicyError {
    /// A policy constraint was violated.
    #[error("policy violation: {0}")]
    Violation(String),
    /// Too many plugins installed.
    #[error("plugin limit exceeded: {maximum} plugins allowed, {current} installed")]
    LimitExceeded {
        /// Configured maximum.
        maximum: usize,
        /// Current count.
        current: usize,
    },
    /// Plugin requires approval but none was granted.
    #[error("plugin '{0}' requires approval before installation")]
    ApprovalRequired(String),
}

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

/// Where a policy originates from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginPolicySource {
    /// User-level policy (personal settings).
    User,
    /// Project-level policy (shared project settings).
    Project,
    /// Enterprise / managed policy (organisation-wide, highest priority).
    Enterprise,
}

impl PluginPolicySource {
    /// Return the merge priority for this source (higher wins).
    pub fn priority(&self) -> u8 {
        match self {
            Self::User => 0,
            Self::Project => 1,
            Self::Enterprise => 2,
        }
    }
}

// ---------------------------------------------------------------------------
// PluginPolicy
// ---------------------------------------------------------------------------

/// A set of policy rules governing which plugins may be installed or enabled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PluginPolicy {
    /// Only plugins from these sources (marketplace names or URLs) are allowed.
    /// An empty set means all sources are allowed.
    #[serde(default)]
    pub allowed_sources: HashSet<String>,

    /// These plugin IDs (in `"name@marketplace"` format) are explicitly blocked.
    #[serde(default)]
    pub blocked_plugins: HashSet<String>,

    /// Maximum number of plugins that may be installed simultaneously.
    /// `None` means no limit.
    #[serde(default)]
    pub max_plugins: Option<usize>,

    /// Whether each plugin installation requires explicit approval.
    #[serde(default)]
    pub require_approval: bool,

    /// The source of this policy (for merge priority).
    #[serde(default)]
    pub source: Option<PluginPolicySource>,
}

impl PluginPolicy {
    /// Create a new permissive (allow-all) policy.
    pub fn allow_all() -> Self {
        Self::default()
    }

    /// Create a policy that blocks everything.
    pub fn deny_all() -> Self {
        Self {
            allowed_sources: HashSet::new(),
            blocked_plugins: HashSet::new(),
            max_plugins: Some(0),
            require_approval: true,
            source: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Policy checking
// ---------------------------------------------------------------------------

/// Outcome of a policy check for a single plugin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyCheckResult {
    /// Whether the plugin is allowed.
    pub allowed: bool,
    /// Human-readable reason if blocked.
    pub reason: Option<String>,
}

/// Check whether a plugin is permitted under the given policy.
///
/// The `plugin_id` should be in `"name@marketplace"` format.
/// The `source` is the marketplace name or URL the plugin comes from.
pub fn check_plugin_policy(
    policy: &PluginPolicy,
    plugin_id: &str,
    source: Option<&str>,
    current_plugin_count: usize,
) -> PolicyCheckResult {
    // 1. Blocked list takes highest priority
    if policy.blocked_plugins.contains(plugin_id) {
        return PolicyCheckResult {
            allowed: false,
            reason: Some(format!("Plugin '{plugin_id}' is blocked by policy")),
        };
    }

    // 2. Source allowlist
    if !policy.allowed_sources.is_empty() {
        if let Some(src) = source {
            if !policy.allowed_sources.contains(src) {
                return PolicyCheckResult {
                    allowed: false,
                    reason: Some(format!("Source '{src}' is not in the allowed sources list")),
                };
            }
        } else {
            // No source provided but allowlist is non-empty → block
            return PolicyCheckResult {
                allowed: false,
                reason: Some("No source provided and allowed-sources list is non-empty".into()),
            };
        }
    }

    // 3. Max plugins
    if let Some(max) = policy.max_plugins
        && current_plugin_count >= max
    {
        return PolicyCheckResult {
            allowed: false,
            reason: Some(format!(
                "Plugin limit of {max} reached ({current_plugin_count} installed)"
            )),
        };
    }

    // 4. Approval required
    if policy.require_approval {
        return PolicyCheckResult {
            allowed: false,
            reason: Some(format!(
                "Plugin '{plugin_id}' requires approval before installation"
            )),
        };
    }

    PolicyCheckResult {
        allowed: true,
        reason: None,
    }
}

// ---------------------------------------------------------------------------
// Policy merging
// ---------------------------------------------------------------------------

/// Merge multiple policies. The most restrictive combination wins:
///
/// - `allowed_sources`: intersection of all non-empty sets (empty = allow all)
/// - `blocked_plugins`: union
/// - `max_plugins`: minimum of all set values
/// - `require_approval`: logical OR
pub fn merge_policies(policies: &[PluginPolicy]) -> PluginPolicy {
    if policies.is_empty() {
        return PluginPolicy::default();
    }
    if policies.len() == 1 {
        return policies[0].clone();
    }

    let mut merged = PluginPolicy::default();

    // Sort by priority (highest first) to set the source
    let mut sorted: Vec<&PluginPolicy> = policies.iter().collect();
    sorted.sort_by(|a, b| {
        let pa = a.source.as_ref().map(|s| s.priority()).unwrap_or(0);
        let pb = b.source.as_ref().map(|s| s.priority()).unwrap_or(0);
        pb.cmp(&pa)
    });
    merged.source = sorted[0].source;

    // allowed_sources: intersection of non-empty sets
    let non_empty: Vec<&HashSet<String>> = policies
        .iter()
        .filter(|p| !p.allowed_sources.is_empty())
        .map(|p| &p.allowed_sources)
        .collect();
    if non_empty.is_empty() {
        // All empty → allow all
        merged.allowed_sources = HashSet::new();
    } else if non_empty.len() == 1 {
        merged.allowed_sources = non_empty[0].clone();
    } else {
        merged.allowed_sources = non_empty[0]
            .iter()
            .filter(|s| non_empty[1..].iter().all(|set| set.contains(*s)))
            .cloned()
            .collect();
    }

    // blocked_plugins: union
    for p in policies {
        merged
            .blocked_plugins
            .extend(p.blocked_plugins.iter().cloned());
    }

    // max_plugins: minimum
    let maxes: Vec<usize> = policies.iter().filter_map(|p| p.max_plugins).collect();
    merged.max_plugins = maxes.into_iter().min();

    // require_approval: OR
    merged.require_approval = policies.iter().any(|p| p.require_approval);

    merged
}

/// Build a policy from a simple enabled/disabled map (as used in managed
/// settings). Keys are plugin IDs, values are `true` (allowed) or `false`
/// (blocked).
pub fn policy_from_enabled_map(
    enabled_map: &HashMap<String, bool>,
    source: PluginPolicySource,
) -> PluginPolicy {
    let blocked: HashSet<String> = enabled_map
        .iter()
        .filter(|(_, enabled)| !*enabled)
        .map(|(id, _)| id.clone())
        .collect();

    PluginPolicy {
        allowed_sources: HashSet::new(),
        blocked_plugins: blocked,
        max_plugins: None,
        require_approval: false,
        source: Some(source),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_policy(
        allowed: &[&str],
        blocked: &[&str],
        max: Option<usize>,
        approval: bool,
    ) -> PluginPolicy {
        PluginPolicy {
            allowed_sources: allowed.iter().map(|s| (*s).to_string()).collect(),
            blocked_plugins: blocked.iter().map(|s| (*s).to_string()).collect(),
            max_plugins: max,
            require_approval: approval,
            source: None,
        }
    }

    #[test]
    fn default_policy_allows_everything() {
        let policy = PluginPolicy::default();
        let result = check_plugin_policy(&policy, "test@marketplace", Some("marketplace"), 0);
        assert!(result.allowed);
    }

    #[test]
    fn blocked_plugin_is_rejected() {
        let policy = make_policy(&[], &["bad@marketplace"], None, false);
        let result = check_plugin_policy(&policy, "bad@marketplace", Some("marketplace"), 0);
        assert!(!result.allowed);
        assert!(result.reason.is_some());
    }

    #[test]
    fn non_blocked_plugin_passes() {
        let policy = make_policy(&[], &["bad@marketplace"], None, false);
        let result = check_plugin_policy(&policy, "good@marketplace", Some("marketplace"), 0);
        assert!(result.allowed);
    }

    #[test]
    fn allowed_sources_restricts() {
        let policy = make_policy(&["official"], &[], None, false);
        // From allowed source
        let result = check_plugin_policy(&policy, "p@official", Some("official"), 0);
        assert!(result.allowed);
        // From disallowed source
        let result = check_plugin_policy(&policy, "p@unofficial", Some("unofficial"), 0);
        assert!(!result.allowed);
    }

    #[test]
    fn max_plugins_enforced() {
        let policy = make_policy(&[], &[], Some(3), false);
        assert!(check_plugin_policy(&policy, "p@m", Some("m"), 2).allowed);
        assert!(!check_plugin_policy(&policy, "p@m", Some("m"), 3).allowed);
    }

    #[test]
    fn require_approval_blocks() {
        let policy = make_policy(&[], &[], None, true);
        let result = check_plugin_policy(&policy, "p@m", Some("m"), 0);
        assert!(!result.allowed);
        assert!(
            result
                .reason
                .expect("blocked policy should include a reason")
                .contains("approval")
        );
    }

    #[test]
    fn merge_takes_union_of_blocked() {
        let p1 = make_policy(&[], &["a@m"], None, false);
        let p2 = make_policy(&[], &["b@m"], None, false);
        let merged = merge_policies(&[p1, p2]);
        assert!(merged.blocked_plugins.contains("a@m"));
        assert!(merged.blocked_plugins.contains("b@m"));
    }

    #[test]
    fn merge_takes_intersection_of_sources() {
        let p1 = make_policy(&["s1", "s2"], &[], None, false);
        let p2 = make_policy(&["s2", "s3"], &[], None, false);
        let merged = merge_policies(&[p1, p2]);
        assert!(merged.allowed_sources.contains("s2"));
        assert!(!merged.allowed_sources.contains("s1"));
        assert!(!merged.allowed_sources.contains("s3"));
    }

    #[test]
    fn merge_takes_min_max() {
        let p1 = make_policy(&[], &[], Some(5), false);
        let p2 = make_policy(&[], &[], Some(3), false);
        let merged = merge_policies(&[p1, p2]);
        assert_eq!(merged.max_plugins, Some(3));
    }

    #[test]
    fn merge_or_approval() {
        let p1 = make_policy(&[], &[], None, false);
        let p2 = make_policy(&[], &[], None, true);
        let merged = merge_policies(&[p1, p2]);
        assert!(merged.require_approval);
    }

    #[test]
    fn merge_empty_returns_default() {
        let merged = merge_policies(&[]);
        assert_eq!(merged, PluginPolicy::default());
    }

    #[test]
    fn merge_single_returns_clone() {
        let p = make_policy(&["s"], &["b"], Some(1), true);
        let merged = merge_policies(std::slice::from_ref(&p));
        assert_eq!(merged, p);
    }

    #[test]
    fn test_policy_from_enabled_map() {
        let mut map = HashMap::new();
        map.insert("good@m".to_string(), true);
        map.insert("bad@m".to_string(), false);
        let policy = policy_from_enabled_map(&map, PluginPolicySource::Enterprise);
        assert!(!policy.blocked_plugins.contains("good@m"));
        assert!(policy.blocked_plugins.contains("bad@m"));
        assert_eq!(policy.source, Some(PluginPolicySource::Enterprise));
    }

    #[test]
    fn policy_source_priority() {
        assert!(PluginPolicySource::Enterprise.priority() > PluginPolicySource::Project.priority());
        assert!(PluginPolicySource::Project.priority() > PluginPolicySource::User.priority());
    }

    #[test]
    fn deny_all_policy() {
        let policy = PluginPolicy::deny_all();
        assert_eq!(policy.max_plugins, Some(0));
        assert!(policy.require_approval);
    }
}
