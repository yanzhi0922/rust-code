//! Per-tool policy filtering for MCP servers.
//!
//! Allows configuring allowlists and denylists for individual MCP tools so
//! that the model only sees (and can call) tools that pass the policy filter.

use serde::{Deserialize, Serialize};

use crate::types::McpToolDescriptor;

/// Per-server tool policy governing which MCP tools are visible to the model.
///
/// If neither `allowlist` nor `denylist` is set, all tools from the server
/// are exposed (the default).
///
/// If `allowlist` is set, **only** tools whose names appear in the list are
/// exposed; all others are hidden.
///
/// If `denylist` is set (and `allowlist` is not), tools whose names appear
/// in the denylist are hidden; all others are exposed.
///
/// If both are set, `allowlist` takes precedence — only allowlisted tools
/// are shown, and the denylist is ignored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct McpToolPolicy {
    /// If set, only these tool names are exposed. Takes precedence over
    /// `denylist` when both are specified.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowlist: Option<Vec<String>>,
    /// If set (and `allowlist` is `None`), these tool names are hidden.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub denylist: Option<Vec<String>>,
}

impl McpToolPolicy {
    /// Create a new empty policy (all tools pass).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a policy that only allows the specified tools.
    #[must_use]
    pub fn allow_only(names: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            allowlist: Some(names.into_iter().map(Into::into).collect()),
            denylist: None,
        }
    }

    /// Create a policy that denies the specified tools.
    #[must_use]
    pub fn deny_only(names: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            allowlist: None,
            denylist: Some(names.into_iter().map(Into::into).collect()),
        }
    }

    /// Returns `true` if the policy has no filtering rules (all tools pass).
    #[must_use]
    pub fn is_pass_all(&self) -> bool {
        self.allowlist.is_none() && self.denylist.is_none()
    }

    /// Check whether a tool name is allowed under this policy.
    ///
    /// Returns `true` when:
    /// - The policy is pass-all (both lists are `None`).
    /// - An allowlist is set and the name is in it.
    /// - No allowlist is set, a denylist is set, and the name is not in it.
    #[must_use]
    pub fn is_tool_allowed(&self, tool_name: &str) -> bool {
        if let Some(ref allowlist) = self.allowlist {
            // Allowlist mode: only listed names pass.
            allowlist.iter().any(|n| n == tool_name)
        } else if let Some(ref denylist) = self.denylist {
            // Denylist mode: everything except listed names passes.
            !denylist.iter().any(|n| n == tool_name)
        } else {
            // No filter: everything passes.
            true
        }
    }

    /// Filter a list of tool descriptors, retaining only those that are
    /// allowed under this policy.
    pub fn filter_tools(&self, tools: &[McpToolDescriptor]) -> Vec<McpToolDescriptor> {
        if self.is_pass_all() {
            return tools.to_vec();
        }
        tools
            .iter()
            .filter(|t| self.is_tool_allowed(&t.name))
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_passes_all() {
        let policy = McpToolPolicy::default();
        assert!(policy.is_pass_all());
        assert!(policy.is_tool_allowed("anything"));
        assert!(policy.is_tool_allowed("other"));
    }

    #[test]
    fn allowlist_only_passes_listed() {
        let policy = McpToolPolicy::allow_only(["search", "read"]);
        assert!(!policy.is_pass_all());
        assert!(policy.is_tool_allowed("search"));
        assert!(policy.is_tool_allowed("read"));
        assert!(!policy.is_tool_allowed("delete"));
    }

    #[test]
    fn denylist_blocks_listed() {
        let policy = McpToolPolicy::deny_only(["delete", "drop"]);
        assert!(!policy.is_pass_all());
        assert!(!policy.is_tool_allowed("delete"));
        assert!(!policy.is_tool_allowed("drop"));
        assert!(policy.is_tool_allowed("search"));
        assert!(policy.is_tool_allowed("read"));
    }

    #[test]
    fn allowlist_takes_precedence_over_denylist() {
        let policy = McpToolPolicy {
            allowlist: Some(vec!["search".to_owned()]),
            denylist: Some(vec!["search".to_owned()]),
        };
        // search is in the allowlist, so it passes even though it is also
        // in the denylist (allowlist takes precedence).
        assert!(policy.is_tool_allowed("search"));
        assert!(!policy.is_tool_allowed("other"));
    }

    #[test]
    fn empty_allowlist_blocks_everything() {
        let policy = McpToolPolicy {
            allowlist: Some(vec![]),
            denylist: None,
        };
        assert!(!policy.is_tool_allowed("anything"));
    }

    #[test]
    fn empty_denylist_passes_everything() {
        let policy = McpToolPolicy {
            allowlist: None,
            denylist: Some(vec![]),
        };
        assert!(policy.is_tool_allowed("anything"));
    }

    #[test]
    fn filter_tools_returns_allowed_subset() {
        let tools = vec![make_tool("search"), make_tool("read"), make_tool("delete")];
        let policy = McpToolPolicy::allow_only(["search", "read"]);
        let filtered = policy.filter_tools(&tools);
        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].name, "search");
        assert_eq!(filtered[1].name, "read");
    }

    #[test]
    fn filter_tools_pass_all_returns_all() {
        let tools = vec![make_tool("a"), make_tool("b")];
        let policy = McpToolPolicy::default();
        let filtered = policy.filter_tools(&tools);
        assert_eq!(filtered.len(), 2);
    }

    #[test]
    fn filter_tools_denylist_removes_matching() {
        let tools = vec![make_tool("search"), make_tool("delete"), make_tool("drop")];
        let policy = McpToolPolicy::deny_only(["delete", "drop"]);
        let filtered = policy.filter_tools(&tools);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name, "search");
    }

    #[test]
    fn serde_roundtrip_allowlist() {
        let policy = McpToolPolicy::allow_only(["a", "b"]);
        let json = serde_json::to_string(&policy).unwrap();
        let back: McpToolPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(policy, back);
    }

    #[test]
    fn serde_roundtrip_denylist() {
        let policy = McpToolPolicy::deny_only(["x"]);
        let json = serde_json::to_string(&policy).unwrap();
        let back: McpToolPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(policy, back);
    }

    #[test]
    fn serde_roundtrip_default_skips_none_fields() {
        let policy = McpToolPolicy::default();
        let json = serde_json::to_string(&policy).unwrap();
        assert_eq!(json, "{}");
    }

    fn make_tool(name: &str) -> McpToolDescriptor {
        McpToolDescriptor {
            name: name.to_owned(),
            title: None,
            description: None,
            input_schema: serde_json::json!({}),
            annotations: serde_json::json!({}),
        }
    }
}
