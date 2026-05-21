//! Shadowed rule detection.
//!
//! Corresponds to `src/utils/permissions/shadowedRuleDetection.ts`.
//! Detects when a permission rule is shadowed by another rule with higher priority,
//! meaning it will never be evaluated.

use claude_core::permission_types::{PermissionBehavior, PermissionRuleSource};

use crate::rule::PermissionRuleV2;

/// Priority order for permission rule sources (higher = evaluated first).
const SOURCE_PRIORITY: &[PermissionRuleSource] = &[
    PermissionRuleSource::CliArg,
    PermissionRuleSource::Command,
    PermissionRuleSource::Session,
    PermissionRuleSource::FlagSettings,
    PermissionRuleSource::PolicySettings,
    PermissionRuleSource::LocalSettings,
    PermissionRuleSource::ProjectSettings,
    PermissionRuleSource::UserSettings,
];

/// Get the priority of a rule source (lower number = higher priority).
fn source_priority(source: PermissionRuleSource) -> usize {
    SOURCE_PRIORITY
        .iter()
        .position(|&s| s == source)
        .unwrap_or(usize::MAX)
}

/// A shadowed rule and the rule that shadows it.
#[derive(Debug, Clone)]
pub struct ShadowedRule {
    /// The rule that is shadowed.
    pub shadowed: PermissionRuleV2,
    /// The rule that shadows it.
    pub shadowed_by: PermissionRuleV2,
    /// Reason for the shadowing.
    pub reason: ShadowReason,
}

/// Reason why a rule is shadowed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShadowReason {
    /// A higher-priority source has the same rule.
    HigherPrioritySource,
    /// An earlier rule in the same source is broader.
    BroaderRule,
    /// A deny rule from a higher-priority source overrides an allow.
    DenyOverridesAllow,
}

/// Detect shadowed rules in a list of permission rules.
///
/// Rules are evaluated in order of source priority. If a rule will never
/// be reached because another rule already matches, it is shadowed.
pub fn detect_shadowed_rules(rules: &[PermissionRuleV2]) -> Vec<ShadowedRule> {
    let mut shadowed = Vec::new();

    // Sort rules by source priority
    let mut sorted: Vec<(usize, &PermissionRuleV2)> = rules.iter().enumerate().collect();
    sorted.sort_by_key(|(_, r)| source_priority(r.source));

    // Check each rule against all higher-priority rules
    for (i, (_, rule)) in sorted.iter().enumerate() {
        for (_, other) in sorted.iter().take(i) {
            // Check if `other` shadows `rule`
            if rules_overlap(other, rule) {
                let reason = if other.source != rule.source {
                    ShadowReason::HigherPrioritySource
                } else if other.behavior == PermissionBehavior::Deny
                    && rule.behavior == PermissionBehavior::Allow
                {
                    ShadowReason::DenyOverridesAllow
                } else {
                    ShadowReason::BroaderRule
                };

                shadowed.push(ShadowedRule {
                    shadowed: (*rule).clone(),
                    shadowed_by: (*other).clone(),
                    reason,
                });
                break; // Only report the first shadowing rule
            }
        }
    }

    shadowed
}

/// Check if two rules overlap (one could shadow the other).
fn rules_overlap(a: &PermissionRuleV2, b: &PermissionRuleV2) -> bool {
    // Same tool name
    if a.value.tool_name != b.value.tool_name {
        return false;
    }

    match (&a.value.rule_content, &b.value.rule_content) {
        // Both have no content — exact same rule
        (None, None) => true,
        // One has no content (broader), other has content (narrower)
        (None, Some(_)) => true,
        (Some(_), None) => false, // Narrower rule can't shadow broader
        // Both have content — check if one contains the other
        (Some(a_content), Some(b_content)) => {
            a_content == b_content
                || a_content == "*"
                || (a_content.ends_with('*')
                    && b_content.starts_with(a_content.trim_end_matches('*').trim()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_rule(
        source: PermissionRuleSource,
        behavior: PermissionBehavior,
        tool: &str,
        content: Option<&str>,
    ) -> PermissionRuleV2 {
        PermissionRuleV2::new(source, behavior, tool, content.map(String::from))
    }

    #[test]
    fn detect_higher_priority_shadowing() {
        let rules = vec![
            make_rule(
                PermissionRuleSource::CliArg,
                PermissionBehavior::Allow,
                "Bash",
                Some("git *"),
            ),
            make_rule(
                PermissionRuleSource::UserSettings,
                PermissionBehavior::Allow,
                "Bash",
                Some("git *"),
            ),
        ];

        let shadowed = detect_shadowed_rules(&rules);
        assert_eq!(shadowed.len(), 1);
        assert_eq!(shadowed[0].reason, ShadowReason::HigherPrioritySource);
    }

    #[test]
    fn detect_deny_overrides_allow() {
        // Same source, deny before allow
        let rules = vec![
            make_rule(
                PermissionRuleSource::UserSettings,
                PermissionBehavior::Deny,
                "Bash",
                Some("rm *"),
            ),
            make_rule(
                PermissionRuleSource::UserSettings,
                PermissionBehavior::Allow,
                "Bash",
                Some("rm *"),
            ),
        ];

        let shadowed = detect_shadowed_rules(&rules);
        assert_eq!(shadowed.len(), 1);
        assert_eq!(shadowed[0].reason, ShadowReason::DenyOverridesAllow);
    }

    #[test]
    fn no_shadowing_different_tools() {
        let rules = vec![
            make_rule(
                PermissionRuleSource::CliArg,
                PermissionBehavior::Allow,
                "Read",
                None,
            ),
            make_rule(
                PermissionRuleSource::UserSettings,
                PermissionBehavior::Allow,
                "Write",
                None,
            ),
        ];

        let shadowed = detect_shadowed_rules(&rules);
        assert!(shadowed.is_empty());
    }

    #[test]
    fn broader_rule_shadows_narrower() {
        let rules = vec![
            make_rule(
                PermissionRuleSource::UserSettings,
                PermissionBehavior::Allow,
                "Bash",
                None,
            ),
            make_rule(
                PermissionRuleSource::UserSettings,
                PermissionBehavior::Allow,
                "Bash",
                Some("git *"),
            ),
        ];

        let shadowed = detect_shadowed_rules(&rules);
        assert_eq!(shadowed.len(), 1);
        assert_eq!(shadowed[0].reason, ShadowReason::BroaderRule);
    }
}
