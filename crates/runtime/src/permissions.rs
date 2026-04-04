use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    Default,
    Plan,
    AcceptEdits,
    BypassPermissions,
    ReadOnly,
}

impl Default for PermissionMode {
    fn default() -> Self {
        Self::Default
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionResult {
    Allow,
    Deny,
    Ask,
    Passthrough,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionPolicy {
    pub mode: PermissionMode,
    #[serde(default)]
    pub allow_rules: Vec<String>,
    #[serde(default)]
    pub deny_rules: Vec<String>,
}

impl Default for PermissionPolicy {
    fn default() -> Self {
        Self {
            mode: PermissionMode::Default,
            allow_rules: Vec::new(),
            deny_rules: Vec::new(),
        }
    }
}

impl PermissionPolicy {
    pub fn evaluate(&self, tool_name: &str) -> PermissionResult {
        for pattern in &self.deny_rules {
            if matches_pattern(pattern, tool_name) {
                return PermissionResult::Deny;
            }
        }

        for pattern in &self.allow_rules {
            if matches_pattern(pattern, tool_name) {
                return PermissionResult::Allow;
            }
        }

        match self.mode {
            PermissionMode::BypassPermissions => PermissionResult::Allow,
            PermissionMode::ReadOnly => {
                if is_read_only_tool(tool_name) {
                    PermissionResult::Allow
                } else {
                    PermissionResult::Ask
                }
            }
            PermissionMode::AcceptEdits => {
                if is_dangerous_tool(tool_name) {
                    PermissionResult::Ask
                } else {
                    PermissionResult::Allow
                }
            }
            PermissionMode::Plan => PermissionResult::Ask,
            PermissionMode::Default => PermissionResult::Ask,
        }
    }
}

fn matches_pattern(pattern: &str, tool_name: &str) -> bool {
    if pattern == tool_name {
        return true;
    }
    if pattern.ends_with('*') {
        let prefix = &pattern[..pattern.len() - 1];
        tool_name.starts_with(prefix)
    } else {
        false
    }
}

fn is_read_only_tool(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "ReadFile"
            | "GlobTool"
            | "GrepTool"
            | "WebFetchTool"
            | "WebSearchTool"
            | "TaskListTool"
            | "TaskGetTool"
            | "ListMcpResourcesTool"
            | "ReadMcpResourceTool"
            | "ToolSearchTool"
    )
}

fn is_dangerous_tool(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "BashTool" | "WriteFile" | "FileEditTool" | "PowerShellTool"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_permission() {
        let policy = PermissionPolicy::default();
        assert_eq!(policy.evaluate("ReadFile"), PermissionResult::Ask);
        assert_eq!(policy.evaluate("BashTool"), PermissionResult::Ask);
    }

    #[test]
    fn test_readonly_blocks_writes() {
        let policy = PermissionPolicy {
            mode: PermissionMode::ReadOnly,
            allow_rules: vec![],
            deny_rules: vec![],
        };
        assert_eq!(policy.evaluate("ReadFile"), PermissionResult::Allow);
        assert_eq!(policy.evaluate("GlobTool"), PermissionResult::Allow);
        assert_eq!(policy.evaluate("BashTool"), PermissionResult::Ask);
        assert_eq!(policy.evaluate("WriteFile"), PermissionResult::Ask);
    }

    #[test]
    fn test_bypass_allows_everything() {
        let policy = PermissionPolicy {
            mode: PermissionMode::BypassPermissions,
            allow_rules: vec![],
            deny_rules: vec![],
        };
        assert_eq!(policy.evaluate("BashTool"), PermissionResult::Allow);
        assert_eq!(policy.evaluate("WriteFile"), PermissionResult::Allow);
        assert_eq!(policy.evaluate("ReadFile"), PermissionResult::Allow);
    }

    #[test]
    fn test_deny_overrides_allow() {
        let policy = PermissionPolicy {
            mode: PermissionMode::BypassPermissions,
            allow_rules: vec!["BashTool".to_string()],
            deny_rules: vec!["BashTool".to_string()],
        };
        assert_eq!(policy.evaluate("BashTool"), PermissionResult::Deny);
    }

    #[test]
    fn test_pattern_matching() {
        let policy = PermissionPolicy {
            mode: PermissionMode::Default,
            allow_rules: vec!["Read*".to_string()],
            deny_rules: vec![],
        };
        assert_eq!(policy.evaluate("ReadFile"), PermissionResult::Allow);
        assert_eq!(policy.evaluate("ReadMcpResource"), PermissionResult::Allow);
        assert_eq!(policy.evaluate("BashTool"), PermissionResult::Ask);

        let deny_policy = PermissionPolicy {
            mode: PermissionMode::BypassPermissions,
            allow_rules: vec![],
            deny_rules: vec!["Web*".to_string()],
        };
        assert_eq!(deny_policy.evaluate("WebFetchTool"), PermissionResult::Deny);
        assert_eq!(
            deny_policy.evaluate("WebSearchTool"),
            PermissionResult::Deny
        );
        assert_eq!(deny_policy.evaluate("ReadFile"), PermissionResult::Allow);
    }
}
