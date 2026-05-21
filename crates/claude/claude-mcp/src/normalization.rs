//! MCP name normalization utilities.
//!
//! Provides functions for normalizing server/tool names to comply with the
//! MCP API pattern `^[a-zA-Z0-9_-]{1,64}$`, building fully-qualified tool
//! names, and parsing them back into their components.

use serde::{Deserialize, Serialize};

/// Parsed MCP name information (server name + tool name).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpNameInfo {
    /// The MCP server name.
    pub server_name: String,
    /// The tool name within the server.
    pub tool_name: String,
}

/// Normalize a name to be compatible with the MCP API pattern `^[a-zA-Z0-9_-]{1,64}$`.
///
/// Replaces any character that is not alphanumeric, underscore, or hyphen
/// with an underscore, then truncates to 64 characters.
///
/// # Examples
/// ```
/// use claude_mcp::normalization::normalize_name_for_mcp;
/// assert_eq!(normalize_name_for_mcp("hello world"), "hello_world");
/// assert_eq!(normalize_name_for_mcp("foo.bar/baz"), "foo_bar_baz");
/// ```
pub fn normalize_name_for_mcp(name: &str) -> String {
    let normalized: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let mut result = normalized;
    // Truncate to 64 characters, but don't cut in the middle of a multi-byte char
    result.truncate(64);
    // Ensure the result is not empty
    if result.is_empty() {
        result = "_".to_owned();
    }
    result
}

/// Parse a fully-qualified MCP tool string into server name and tool name.
///
/// The expected format is `mcp__<server_name>__<tool_name>`.
/// Returns `None` if the string doesn't match the expected pattern.
pub fn mcp_info_from_string(tool_string: &str) -> Option<McpNameInfo> {
    let stripped = tool_string.strip_prefix("mcp__")?;
    let separator_pos = stripped.find("__")?;
    let server_name = stripped[..separator_pos].to_owned();
    let tool_name = stripped[separator_pos + 2..].to_owned();
    if server_name.is_empty() || tool_name.is_empty() {
        return None;
    }
    Some(McpNameInfo {
        server_name,
        tool_name,
    })
}

/// Build a fully-qualified MCP tool name from server name and tool name.
///
/// Format: `mcp__<server_name>__<tool_name>`
pub fn build_mcp_tool_name(server_name: &str, tool_name: &str) -> String {
    format!("mcp__{server_name}__{tool_name}")
}

/// Build a fully-qualified MCP prompt/slash-command name from raw server and
/// prompt identifiers.
///
/// Claude Code normalizes only the server segment for MCP prompt commands and
/// keeps the prompt programmatic name unchanged.
pub fn build_mcp_prompt_command_name(server_name: &str, prompt_name: &str) -> String {
    build_mcp_tool_name(&normalize_name_for_mcp(server_name), prompt_name)
}

/// Get the MCP tool name prefix for a given server.
///
/// Format: `mcp__<server_name>__`
pub fn get_mcp_prefix(server_name: &str) -> String {
    format!("mcp__{server_name}__")
}

/// Get the display name of a tool by stripping the server prefix.
///
/// If `full_name` starts with `mcp__<server_name>__`, returns the remainder.
/// Otherwise returns `full_name` unchanged.
pub fn get_mcp_display_name(full_name: &str, server_name: &str) -> String {
    let prefix = get_mcp_prefix(server_name);
    full_name
        .strip_prefix(&prefix)
        .map_or_else(|| full_name.to_owned(), str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_replaces_spaces() {
        assert_eq!(normalize_name_for_mcp("hello world"), "hello_world");
    }

    #[test]
    fn normalize_replaces_special_chars() {
        assert_eq!(normalize_name_for_mcp("foo.bar/baz@qux"), "foo_bar_baz_qux");
    }

    #[test]
    fn normalize_preserves_valid_chars() {
        assert_eq!(normalize_name_for_mcp("my-server_v2"), "my-server_v2");
    }

    #[test]
    fn normalize_truncates_long_names() {
        let long = "a".repeat(100);
        let result = normalize_name_for_mcp(&long);
        assert_eq!(result.len(), 64);
    }

    #[test]
    fn normalize_empty_input() {
        assert_eq!(normalize_name_for_mcp(""), "_");
    }

    #[test]
    fn build_mcp_prompt_command_normalizes_server_only() {
        assert_eq!(
            build_mcp_prompt_command_name("my server", "daily.plan"),
            "mcp__my_server__daily.plan"
        );
    }

    #[test]
    fn mcp_info_from_string_valid() {
        let info = mcp_info_from_string("mcp__myserver__search").expect("should parse");
        assert_eq!(info.server_name, "myserver");
        assert_eq!(info.tool_name, "search");
    }

    #[test]
    fn mcp_info_from_string_no_prefix() {
        assert!(mcp_info_from_string("search").is_none());
    }

    #[test]
    fn mcp_info_from_string_no_separator() {
        assert!(mcp_info_from_string("mcp__myserver").is_none());
    }

    #[test]
    fn mcp_info_from_string_empty_parts() {
        assert!(mcp_info_from_string("mcp____search").is_none());
    }

    #[test]
    fn build_tool_name() {
        assert_eq!(build_mcp_tool_name("srv", "tool"), "mcp__srv__tool");
    }

    #[test]
    fn get_prefix() {
        assert_eq!(get_mcp_prefix("srv"), "mcp__srv__");
    }

    #[test]
    fn display_name_strips_prefix() {
        assert_eq!(get_mcp_display_name("mcp__srv__search", "srv"), "search");
    }

    #[test]
    fn display_name_no_prefix_returns_original() {
        assert_eq!(get_mcp_display_name("search", "srv"), "search");
    }

    #[test]
    fn roundtrip_build_and_parse() {
        let server = "my-server";
        let tool = "my_tool";
        let full = build_mcp_tool_name(server, tool);
        let info = mcp_info_from_string(&full).expect("should parse");
        assert_eq!(info.server_name, server);
        assert_eq!(info.tool_name, tool);
    }
}
