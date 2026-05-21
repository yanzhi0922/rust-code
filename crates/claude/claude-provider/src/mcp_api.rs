//! MCP Tools in API requests.
//!
//! Provides types and utilities for configuring MCP (Model Context Protocol)
//! tools that are passed directly in the API request body, enabling
//! server-side tool routing.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

// ---------------------------------------------------------------------------
// MCP Server Tool Definition
// ---------------------------------------------------------------------------

/// Definition of a single MCP server tool for the API.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct McpServerToolDef {
    /// The MCP server name.
    pub server_name: String,
    /// The tool name within the server.
    pub tool_name: String,
    /// Optional description of what the tool does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The JSON schema for the tool's input parameters.
    #[serde(default)]
    pub input_schema: Value,
}

impl McpServerToolDef {
    /// Create a new MCP server tool definition.
    #[must_use]
    pub fn new(server_name: String, tool_name: String) -> Self {
        Self {
            server_name,
            tool_name,
            description: None,
            input_schema: json!({}),
        }
    }

    /// Set the tool description.
    #[must_use]
    pub fn with_description(mut self, description: String) -> Self {
        self.description = Some(description);
        self
    }

    /// Set the input schema.
    #[must_use]
    pub fn with_input_schema(mut self, schema: Value) -> Self {
        self.input_schema = schema;
        self
    }

    /// Generate the qualified tool name (server__tool format).
    #[must_use]
    pub fn qualified_name(&self) -> String {
        format!("mcp__{}__{}", self.server_name, self.tool_name)
    }
}

// ---------------------------------------------------------------------------
// MCP Tool Configuration
// ---------------------------------------------------------------------------

/// Configuration for MCP tools in the API request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct McpToolConfig {
    /// Whether MCP tools are enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// List of MCP server tool definitions.
    #[serde(default)]
    pub tools: Vec<McpServerToolDef>,
    /// Whether to allow all MCP tools from configured servers.
    #[serde(default)]
    pub allow_all: bool,
    /// List of allowed server names (empty = all allowed).
    #[serde(default)]
    pub allowed_servers: Vec<String>,
}

fn default_true() -> bool {
    true
}

impl Default for McpToolConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            tools: vec![],
            allow_all: false,
            allowed_servers: vec![],
        }
    }
}

impl McpToolConfig {
    /// Create a new MCP tool config with the given tools.
    #[must_use]
    pub fn new(tools: Vec<McpServerToolDef>) -> Self {
        Self {
            enabled: true,
            tools,
            allow_all: false,
            allowed_servers: vec![],
        }
    }

    /// Create a config that allows all MCP tools.
    #[must_use]
    pub fn allow_all() -> Self {
        Self {
            enabled: true,
            tools: vec![],
            allow_all: true,
            allowed_servers: vec![],
        }
    }

    /// Create a disabled MCP tool config.
    #[must_use]
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            tools: vec![],
            allow_all: false,
            allowed_servers: vec![],
        }
    }

    /// Check if a specific server is allowed.
    #[must_use]
    pub fn is_server_allowed(&self, server_name: &str) -> bool {
        if !self.enabled {
            return false;
        }
        if self.allow_all || self.allowed_servers.is_empty() {
            return true;
        }
        self.allowed_servers.iter().any(|s| s == server_name)
    }

    /// Get all qualified tool names.
    #[must_use]
    pub fn qualified_tool_names(&self) -> Vec<String> {
        self.tools.iter().map(|t| t.qualified_name()).collect()
    }
}

// ---------------------------------------------------------------------------
// API parameter generation
// ---------------------------------------------------------------------------

/// Build the `mcpTools` API parameter from the configuration.
///
/// # Arguments
///
/// * `config` — The MCP tool configuration.
///
/// # Returns
///
/// A JSON value suitable for inclusion in the API request body, or `None`
/// if MCP tools are disabled.
#[must_use]
pub fn build_mcp_tools_param(config: &McpToolConfig) -> Option<Value> {
    if !config.enabled {
        return None;
    }

    if config.allow_all && config.tools.is_empty() {
        return Some(json!({
            "mcpTools": { "allowAll": true }
        }));
    }

    if config.tools.is_empty() {
        return None;
    }

    let tools_json: Vec<Value> = config
        .tools
        .iter()
        .map(|tool| {
            let mut obj = json!({
                "server_name": tool.server_name,
                "tool_name": tool.tool_name,
                "input_schema": tool.input_schema,
            });
            if let Some(ref desc) = tool.description {
                obj["description"] = json!(desc);
            }
            obj
        })
        .collect();

    Some(json!({
        "mcpTools": {
            "tools": tools_json,
            "allowAll": config.allow_all,
        }
    }))
}

/// Merge MCP tools configuration into an API request body.
///
/// # Arguments
///
/// * `body` — The mutable API request body.
/// * `config` — The MCP tool configuration.
pub fn merge_mcp_tools_into_body(body: &mut Value, config: &McpToolConfig) {
    if let Some(params) = build_mcp_tools_param(config)
        && let Value::Object(map) = params
    {
        for (key, value) in map {
            body[key] = value;
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- McpServerToolDef ---

    #[test]
    fn mcp_server_tool_def_new() {
        let def = McpServerToolDef::new("my_server".to_string(), "my_tool".to_string());
        assert_eq!(def.server_name, "my_server");
        assert_eq!(def.tool_name, "my_tool");
        assert!(def.description.is_none());
        assert_eq!(def.input_schema, json!({}));
    }

    #[test]
    fn mcp_server_tool_def_builder() {
        let def = McpServerToolDef::new("srv".to_string(), "tool".to_string())
            .with_description("A test tool".to_string())
            .with_input_schema(json!({"type": "object", "properties": {}}));
        assert_eq!(
            def.description.as_ref().expect("description"),
            "A test tool"
        );
        assert_eq!(def.input_schema["type"], "object");
    }

    #[test]
    fn mcp_server_tool_def_qualified_name() {
        let def = McpServerToolDef::new("server1".to_string(), "read_file".to_string());
        assert_eq!(def.qualified_name(), "mcp__server1__read_file");
    }

    #[test]
    fn mcp_server_tool_def_serialization_roundtrip() {
        let def = McpServerToolDef::new("srv".to_string(), "tool".to_string())
            .with_description("desc".to_string());
        let json = serde_json::to_string(&def).expect("serialize");
        let deserialized: McpServerToolDef = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(def, deserialized);
    }

    // --- McpToolConfig ---

    #[test]
    fn mcp_tool_config_default() {
        let config = McpToolConfig::default();
        assert!(config.enabled);
        assert!(config.tools.is_empty());
        assert!(!config.allow_all);
    }

    #[test]
    fn mcp_tool_config_new() {
        let tools = vec![McpServerToolDef::new("s1".to_string(), "t1".to_string())];
        let config = McpToolConfig::new(tools);
        assert_eq!(config.tools.len(), 1);
    }

    #[test]
    fn mcp_tool_config_allow_all() {
        let config = McpToolConfig::allow_all();
        assert!(config.allow_all);
    }

    #[test]
    fn mcp_tool_config_disabled() {
        let config = McpToolConfig::disabled();
        assert!(!config.enabled);
    }

    #[test]
    fn mcp_tool_config_is_server_allowed_default() {
        let config = McpToolConfig::default();
        assert!(config.is_server_allowed("any_server"));
    }

    #[test]
    fn mcp_tool_config_is_server_allowed_specific() {
        let config = McpToolConfig {
            allowed_servers: vec!["srv1".to_string(), "srv2".to_string()],
            ..McpToolConfig::default()
        };
        assert!(config.is_server_allowed("srv1"));
        assert!(!config.is_server_allowed("srv3"));
    }

    #[test]
    fn mcp_tool_config_is_server_allowed_disabled() {
        let config = McpToolConfig::disabled();
        assert!(!config.is_server_allowed("any"));
    }

    #[test]
    fn mcp_tool_config_qualified_tool_names() {
        let config = McpToolConfig::new(vec![
            McpServerToolDef::new("s1".to_string(), "t1".to_string()),
            McpServerToolDef::new("s2".to_string(), "t2".to_string()),
        ]);
        let names = config.qualified_tool_names();
        assert_eq!(names, vec!["mcp__s1__t1", "mcp__s2__t2"]);
    }

    #[test]
    fn mcp_tool_config_serialization_roundtrip() {
        let config = McpToolConfig::new(vec![McpServerToolDef::new(
            "srv".to_string(),
            "tool".to_string(),
        )]);
        let json = serde_json::to_string(&config).expect("serialize");
        let deserialized: McpToolConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(config, deserialized);
    }

    // --- build_mcp_tools_param ---

    #[test]
    fn build_mcp_tools_param_disabled() {
        let config = McpToolConfig::disabled();
        assert!(build_mcp_tools_param(&config).is_none());
    }

    #[test]
    fn build_mcp_tools_param_empty_tools() {
        let config = McpToolConfig::default();
        assert!(build_mcp_tools_param(&config).is_none());
    }

    #[test]
    fn build_mcp_tools_param_allow_all() {
        let config = McpToolConfig::allow_all();
        let param = build_mcp_tools_param(&config).expect("should return");
        assert_eq!(param["mcpTools"]["allowAll"], true);
    }

    #[test]
    fn build_mcp_tools_param_with_tools() {
        let config = McpToolConfig::new(vec![
            McpServerToolDef::new("srv".to_string(), "tool".to_string())
                .with_description("desc".to_string()),
        ]);
        let param = build_mcp_tools_param(&config).expect("should return");
        let tools = param["mcpTools"]["tools"].as_array().expect("array");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["server_name"], "srv");
        assert_eq!(tools[0]["tool_name"], "tool");
        assert_eq!(tools[0]["description"], "desc");
    }

    // --- merge_mcp_tools_into_body ---

    #[test]
    fn merge_mcp_tools_into_body_disabled() {
        let mut body = json!({"model": "test"});
        merge_mcp_tools_into_body(&mut body, &McpToolConfig::disabled());
        assert!(body.get("mcpTools").is_none());
    }

    #[test]
    fn merge_mcp_tools_into_body_with_tools() {
        let mut body = json!({"model": "test"});
        let config = McpToolConfig::new(vec![McpServerToolDef::new(
            "s".to_string(),
            "t".to_string(),
        )]);
        merge_mcp_tools_into_body(&mut body, &config);
        assert!(body.get("mcpTools").is_some());
        assert_eq!(body["model"], "test");
    }
}
