//! Permission rule definitions and evaluation.
//!
//! Corresponds to `src/utils/permissions/PermissionRule.ts` and
//! `src/types/permissions.ts` (PermissionRuleValue, PermissionRule).

use claude_core::permission_types::{PermissionBehavior, PermissionRuleSource};
use serde::{Deserialize, Serialize};

use crate::classifier::{extract_prompt_description, shell_prompt_rule_matches_command};
use crate::{PermissionClass, classify_tool};

/// The value of a permission rule — specifies which tool and optional content.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct PermissionRuleValue {
    pub tool_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule_content: Option<String>,
}

impl PermissionRuleValue {
    /// Create a new rule value for a tool with optional content pattern.
    #[must_use]
    pub fn new(tool_name: impl Into<String>, rule_content: Option<String>) -> Self {
        Self {
            tool_name: tool_name.into(),
            rule_content,
        }
    }

    /// Create a simple tool-only rule value.
    #[must_use]
    pub fn tool_only(tool_name: impl Into<String>) -> Self {
        Self {
            tool_name: tool_name.into(),
            rule_content: None,
        }
    }
}

/// A permission rule with its source and behavior.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PermissionRuleV2 {
    pub source: PermissionRuleSource,
    pub behavior: PermissionBehavior,
    pub value: PermissionRuleValue,
}

impl PermissionRuleV2 {
    /// Create a new permission rule.
    #[must_use]
    pub fn new(
        source: PermissionRuleSource,
        behavior: PermissionBehavior,
        tool_name: impl Into<String>,
        rule_content: Option<String>,
    ) -> Self {
        Self {
            source,
            behavior,
            value: PermissionRuleValue::new(tool_name, rule_content),
        }
    }

    /// Check if this rule matches a given tool name and optional content.
    pub fn matches(&self, tool_name: &str, content: Option<&str>) -> bool {
        if self.value.tool_name != tool_name {
            return false;
        }
        match (&self.value.rule_content, content) {
            (None, _) => true,
            (Some(pattern), Some(c))
                if classify_tool(tool_name) == PermissionClass::Bash
                    && extract_prompt_description(pattern).is_some() =>
            {
                shell_prompt_rule_matches_command(
                    c,
                    extract_prompt_description(pattern).expect("checked"),
                )
            }
            (Some(pattern), Some(c)) => glob_match(pattern, c),
            (Some(_), None) => false,
        }
    }
}

/// Simple glob-style matching for permission rule content.
/// Supports `*` as a wildcard.
fn glob_match(pattern: &str, text: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if !pattern.contains('*') {
        return pattern == text;
    }
    // Split pattern by wildcard and check sequential matching
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 2 {
        let (prefix, suffix) = (parts[0], parts[1]);
        text.starts_with(prefix)
            && text.ends_with(suffix)
            && text.len() >= prefix.len() + suffix.len()
    } else {
        // For more complex patterns, use a simple recursive approach
        glob_match_recursive(pattern, text)
    }
}

/// Recursive glob matching for complex patterns.
fn glob_match_recursive(pattern: &str, text: &str) -> bool {
    let mut p_chars = pattern.chars().peekable();
    let mut t_chars = text.chars().peekable();

    loop {
        match (p_chars.next(), t_chars.peek().copied()) {
            (None, _) => return t_chars.next().is_none(),
            (Some('*'), _) => {
                let remaining_pattern: String = p_chars.collect();
                let remaining_text: String = t_chars.collect();
                // Try matching zero or more characters
                for i in 0..=remaining_text.len() {
                    if glob_match_recursive(&remaining_pattern, &remaining_text[i..]) {
                        return true;
                    }
                }
                return false;
            }
            (Some(p), Some(t)) if p == t || p == '?' => {
                t_chars.next();
            }
            _ => return false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rule_value_tool_only() {
        let rv = PermissionRuleValue::tool_only("Read");
        assert_eq!(rv.tool_name, "Read");
        assert!(rv.rule_content.is_none());
    }

    #[test]
    fn rule_matches_exact() {
        let rule = PermissionRuleV2::new(
            PermissionRuleSource::UserSettings,
            PermissionBehavior::Allow,
            "Bash",
            Some("git status".to_string()),
        );
        assert!(rule.matches("Bash", Some("git status")));
        assert!(!rule.matches("Bash", Some("git push")));
        assert!(!rule.matches("Read", Some("git status")));
    }

    #[test]
    fn rule_matches_wildcard() {
        let rule = PermissionRuleV2::new(
            PermissionRuleSource::ProjectSettings,
            PermissionBehavior::Allow,
            "Bash",
            Some("git *".to_string()),
        );
        assert!(rule.matches("Bash", Some("git status")));
        assert!(rule.matches("Bash", Some("git push origin main")));
        assert!(!rule.matches("Bash", Some("npm install")));
    }

    #[test]
    fn prompt_rule_matches_semantic_shell_command() {
        let rule = PermissionRuleV2::new(
            PermissionRuleSource::Session,
            PermissionBehavior::Allow,
            "Bash",
            Some("prompt: run tests".to_string()),
        );
        assert!(rule.matches("Bash", Some("cargo test --workspace")));
        assert!(!rule.matches("Bash", Some("cargo build")));
    }

    #[test]
    fn rule_matches_no_content() {
        let rule = PermissionRuleV2::new(
            PermissionRuleSource::CliArg,
            PermissionBehavior::Allow,
            "Read",
            None,
        );
        assert!(rule.matches("Read", Some("/etc/passwd")));
        assert!(rule.matches("Read", None));
        assert!(!rule.matches("Write", None));
    }

    #[test]
    fn glob_match_star_only() {
        assert!(glob_match("*", "anything"));
        assert!(glob_match("*", ""));
    }

    #[test]
    fn glob_match_prefix_suffix() {
        assert!(glob_match("git *", "git status"));
        assert!(glob_match("git *", "git push origin main"));
        assert!(!glob_match("git *", "npm install"));
    }

    #[test]
    fn glob_match_exact() {
        assert!(glob_match("exact", "exact"));
        assert!(!glob_match("exact", "other"));
    }

    #[test]
    fn glob_match_complex() {
        assert!(glob_match("a*b*c", "aXXbYYc"));
        assert!(glob_match("a*b*c", "abc"));
        assert!(!glob_match("a*b*c", "ac"));
    }

    #[test]
    fn rule_serialization_roundtrip() {
        let rule = PermissionRuleV2::new(
            PermissionRuleSource::UserSettings,
            PermissionBehavior::Allow,
            "Bash",
            Some("git *".to_string()),
        );
        let json = serde_json::to_string(&rule).expect("serialize");
        let back: PermissionRuleV2 = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(rule, back);
    }
}
