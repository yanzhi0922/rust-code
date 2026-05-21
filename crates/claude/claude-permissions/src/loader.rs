//! Permission rule loader.
//!
//! Corresponds to `src/utils/permissions/permissionsLoader.ts`.
//! Loads permission rules from settings files and CLI arguments.

use std::path::Path;

use claude_core::permission_types::{PermissionBehavior, PermissionRuleSource};

use crate::rule::PermissionRuleV2;

/// Load permission rules from a settings JSON file.
///
/// The file should contain a JSON object with `allow`, `deny`, and `ask` arrays
/// of rule strings like `"Bash(git *)"`, `"Read"`, etc.
pub fn load_rules_from_file(
    path: &Path,
    source: PermissionRuleSource,
) -> anyhow::Result<Vec<PermissionRuleV2>> {
    let content = std::fs::read_to_string(path)?;
    let json: serde_json::Value = serde_json::from_str(&content)?;
    let mut rules = Vec::new();

    if let Some(allow_arr) = json.get("allow").and_then(|v| v.as_array()) {
        for item in allow_arr {
            if let Some(rule_str) = item.as_str()
                && let Some(rule) = parse_rule_string(rule_str, source, PermissionBehavior::Allow)
            {
                rules.push(rule);
            }
        }
    }

    if let Some(deny_arr) = json.get("deny").and_then(|v| v.as_array()) {
        for item in deny_arr {
            if let Some(rule_str) = item.as_str()
                && let Some(rule) = parse_rule_string(rule_str, source, PermissionBehavior::Deny)
            {
                rules.push(rule);
            }
        }
    }

    if let Some(ask_arr) = json.get("ask").and_then(|v| v.as_array()) {
        for item in ask_arr {
            if let Some(rule_str) = item.as_str()
                && let Some(rule) = parse_rule_string(rule_str, source, PermissionBehavior::Ask)
            {
                rules.push(rule);
            }
        }
    }

    Ok(rules)
}

/// Parse a rule string like `"Bash(git *)"` or `"Read"` into a PermissionRuleV2.
pub fn parse_rule_string(
    s: &str,
    source: PermissionRuleSource,
    behavior: PermissionBehavior,
) -> Option<PermissionRuleV2> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return None;
    }

    // Check for tool-prefixed pattern: "Bash(git status)"
    if let Some(open) = trimmed.find('(')
        && let Some(close) = trimmed.rfind(')')
        && open < close
    {
        let tool_name = &trimmed[..open];
        let content = &trimmed[open + 1..close];
        return Some(PermissionRuleV2::new(
            source,
            behavior,
            tool_name,
            Some(content.to_string()),
        ));
    }

    // Simple tool name
    Some(PermissionRuleV2::new(source, behavior, trimmed, None))
}

/// Merge rules from multiple sources, respecting priority order.
/// Higher-priority sources come first in the result.
pub fn merge_rules(
    rules_from_sources: Vec<(PermissionRuleSource, Vec<PermissionRuleV2>)>,
) -> Vec<PermissionRuleV2> {
    let mut all_rules: Vec<PermissionRuleV2> = rules_from_sources
        .into_iter()
        .flat_map(|(_, rules)| rules)
        .collect();

    // Sort by source priority (higher priority first)
    all_rules.sort_by(|a, b| {
        let pa = source_priority(a.source);
        let pb = source_priority(b.source);
        pa.cmp(&pb)
    });

    all_rules
}

/// Source priority (lower = higher priority).
fn source_priority(source: PermissionRuleSource) -> u8 {
    match source {
        PermissionRuleSource::CliArg => 0,
        PermissionRuleSource::Command => 1,
        PermissionRuleSource::Session => 2,
        PermissionRuleSource::FlagSettings => 3,
        PermissionRuleSource::PolicySettings => 4,
        PermissionRuleSource::LocalSettings => 5,
        PermissionRuleSource::ProjectSettings => 6,
        PermissionRuleSource::UserSettings => 7,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;

    #[test]
    fn parse_simple_tool() -> anyhow::Result<()> {
        let rule = parse_rule_string(
            "Read",
            PermissionRuleSource::UserSettings,
            PermissionBehavior::Allow,
        )
        .ok_or_else(|| anyhow!("expected Read rule to parse"))?;
        assert_eq!(rule.value.tool_name, "Read");
        assert!(rule.value.rule_content.is_none());
        Ok(())
    }

    #[test]
    fn parse_tool_with_content() -> anyhow::Result<()> {
        let rule = parse_rule_string(
            "Bash(git *)",
            PermissionRuleSource::UserSettings,
            PermissionBehavior::Allow,
        )
        .ok_or_else(|| anyhow!("expected Bash rule with content to parse"))?;
        assert_eq!(rule.value.tool_name, "Bash");
        assert_eq!(rule.value.rule_content, Some("git *".to_string()));
        Ok(())
    }

    #[test]
    fn parse_empty_returns_none() {
        assert!(
            parse_rule_string(
                "",
                PermissionRuleSource::UserSettings,
                PermissionBehavior::Allow,
            )
            .is_none()
        );
    }

    #[test]
    fn merge_rules_by_priority() {
        let user_rules = vec![PermissionRuleV2::new(
            PermissionRuleSource::UserSettings,
            PermissionBehavior::Allow,
            "Bash",
            Some("git *".to_string()),
        )];
        let cli_rules = vec![PermissionRuleV2::new(
            PermissionRuleSource::CliArg,
            PermissionBehavior::Deny,
            "Bash",
            Some("rm *".to_string()),
        )];

        let merged = merge_rules(vec![
            (PermissionRuleSource::UserSettings, user_rules),
            (PermissionRuleSource::CliArg, cli_rules),
        ]);

        assert_eq!(merged.len(), 2);
        // CLI arg should come first (higher priority)
        assert_eq!(merged[0].source, PermissionRuleSource::CliArg);
    }

    #[test]
    fn load_from_json_file() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("permissions.json");
        std::fs::write(
            &path,
            r#"{"allow": ["Read", "Bash(git *)"], "deny": ["Bash(rm *)"]}"#,
        )?;

        let rules = load_rules_from_file(&path, PermissionRuleSource::UserSettings)?;
        assert_eq!(rules.len(), 3);
        assert!(
            rules
                .iter()
                .any(|r| r.value.tool_name == "Read" && r.behavior == PermissionBehavior::Allow)
        );
        assert!(
            rules
                .iter()
                .any(|r| r.value.tool_name == "Bash" && r.behavior == PermissionBehavior::Deny)
        );
        Ok(())
    }
}
