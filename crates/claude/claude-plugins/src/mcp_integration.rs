//! MCP (Model Context Protocol) integration for plugins.
//!
//! Extracts MCP server configurations from plugin manifests and resolves
//! MCP config paths relative to the plugin root.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// An MCP server configuration from a plugin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginMcpServer {
    /// Server name.
    pub name: String,
    /// Command to start the server.
    pub command: String,
    /// Command arguments.
    #[serde(default)]
    pub args: Vec<String>,
    /// Environment variables.
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Source plugin name.
    pub plugin_name: String,
    /// Source file path.
    pub source_path: PathBuf,
}

/// Result of loading MCP servers from a plugin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoadMcpResult {
    /// MCP servers found.
    pub servers: Vec<PluginMcpServer>,
    /// Errors encountered.
    pub errors: Vec<String>,
}

/// Resolved MCP config with absolute paths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedMcpConfig {
    /// Server name.
    pub name: String,
    /// Command to execute.
    pub command: String,
    /// Arguments with resolved paths.
    pub args: Vec<String>,
    /// Environment variables.
    pub env: HashMap<String, String>,
    /// Plugin root directory (for path resolution).
    pub plugin_root: PathBuf,
}

// ---------------------------------------------------------------------------
// Core functions
// ---------------------------------------------------------------------------

/// Load MCP server configurations from a plugin.
///
/// Reads the MCP configuration file referenced by the plugin manifest
/// and parses it into structured server configurations.
pub fn load_plugin_mcp_servers(
    plugin_name: &str,
    mcp_config_path: &Path,
    _plugin_root: &Path,
) -> LoadMcpResult {
    let mut servers = Vec::new();
    let mut errors = Vec::new();

    if !mcp_config_path.exists() {
        errors.push(format!(
            "MCP config {} does not exist",
            mcp_config_path.display()
        ));
        return LoadMcpResult { servers, errors };
    }

    let content = match std::fs::read_to_string(mcp_config_path) {
        Ok(c) => c,
        Err(e) => {
            errors.push(format!(
                "failed to read MCP config {}: {e}",
                mcp_config_path.display()
            ));
            return LoadMcpResult { servers, errors };
        }
    };

    match parse_mcp_config(&content) {
        Ok(parsed_servers) => {
            for (name, config) in parsed_servers {
                servers.push(PluginMcpServer {
                    name,
                    command: config.command,
                    args: config.args,
                    env: config.env,
                    plugin_name: plugin_name.to_owned(),
                    source_path: mcp_config_path.to_path_buf(),
                });
            }
        }
        Err(e) => {
            errors.push(format!(
                "failed to parse MCP config {}: {e}",
                mcp_config_path.display()
            ));
        }
    }

    LoadMcpResult { servers, errors }
}

/// Parse MCP configuration content.
fn parse_mcp_config(content: &str) -> Result<HashMap<String, McpServerEntry>, String> {
    let raw: Value = serde_json::from_str(content).map_err(|e| format!("invalid JSON: {e}"))?;

    // Support both flat format and [mcp_servers] TOML-like format
    let servers_obj = if let Some(obj) = raw.get("mcpServers").or_else(|| raw.get("mcp_servers")) {
        obj.as_object()
            .ok_or_else(|| "mcpServers must be an object".to_owned())?
    } else {
        raw.as_object()
            .ok_or_else(|| "config must be a JSON object".to_owned())?
    };

    let mut servers = HashMap::new();
    for (name, value) in servers_obj {
        let entry = parse_server_entry(value)?;
        servers.insert(name.clone(), entry);
    }

    Ok(servers)
}

/// Parse a single MCP server entry.
fn parse_server_entry(value: &Value) -> Result<McpServerEntry, String> {
    let obj = value
        .as_object()
        .ok_or_else(|| "server entry must be an object".to_owned())?;

    let command = obj
        .get("command")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "server must have a 'command'".to_owned())?
        .to_owned();

    let args = obj
        .get("args")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_owned()))
                .collect()
        })
        .unwrap_or_default();

    let env = obj
        .get("env")
        .and_then(|v| v.as_object())
        .map(|map| {
            map.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_owned())))
                .collect()
        })
        .unwrap_or_default();

    Ok(McpServerEntry { command, args, env })
}

/// Internal server entry during parsing.
struct McpServerEntry {
    command: String,
    args: Vec<String>,
    env: HashMap<String, String>,
}

/// Resolve MCP config paths relative to the plugin root.
///
/// Converts relative paths in args to absolute paths based on the plugin root.
pub fn resolve_plugin_mcp_config(
    server: &PluginMcpServer,
    plugin_root: &Path,
) -> ResolvedMcpConfig {
    let resolved_args = server
        .args
        .iter()
        .map(|arg| {
            let path = Path::new(arg);
            if !path.is_absolute() && path.exists() {
                plugin_root.join(arg).to_string_lossy().to_string()
            } else {
                arg.clone()
            }
        })
        .collect();

    ResolvedMcpConfig {
        name: server.name.clone(),
        command: server.command.clone(),
        args: resolved_args,
        env: server.env.clone(),
        plugin_root: plugin_root.to_path_buf(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn ok<T, E: std::fmt::Display>(result: Result<T, E>) -> T {
        match result {
            Ok(value) => value,
            Err(error) => panic!("unexpected error: {error}"),
        }
    }

    #[test]
    fn load_plugin_mcp_servers_basic() {
        let temp = ok(tempdir());
        let mcp_path = temp.path().join("mcp.json");
        fs::write(
            &mcp_path,
            r#"{
                "mcp_servers": {
                    "demo": {
                        "command": "uvx",
                        "args": ["--help"]
                    }
                }
            }"#,
        )
        .expect("write");

        let result = load_plugin_mcp_servers("test-plugin", &mcp_path, temp.path());
        assert_eq!(result.servers.len(), 1);
        assert!(result.errors.is_empty());
        assert_eq!(result.servers[0].name, "demo");
        assert_eq!(result.servers[0].command, "uvx");
    }

    #[test]
    fn load_plugin_mcp_servers_nonexistent() {
        let result = load_plugin_mcp_servers(
            "test-plugin",
            Path::new("/nonexistent/mcp.json"),
            Path::new("/tmp"),
        );
        assert!(result.servers.is_empty());
        assert_eq!(result.errors.len(), 1);
    }

    #[test]
    fn load_plugin_mcp_servers_invalid_json() {
        let temp = ok(tempdir());
        let mcp_path = temp.path().join("mcp.json");
        fs::write(&mcp_path, "not json").expect("write");

        let result = load_plugin_mcp_servers("test-plugin", &mcp_path, temp.path());
        assert!(result.servers.is_empty());
        assert_eq!(result.errors.len(), 1);
    }

    #[test]
    fn load_plugin_mcp_servers_with_env() {
        let temp = ok(tempdir());
        let mcp_path = temp.path().join("mcp.json");
        fs::write(
            &mcp_path,
            r#"{
                "demo": {
                    "command": "node",
                    "args": ["server.js"],
                    "env": {"PORT": "3000"}
                }
            }"#,
        )
        .expect("write");

        let result = load_plugin_mcp_servers("test-plugin", &mcp_path, temp.path());
        assert_eq!(result.servers.len(), 1);
        assert_eq!(result.servers[0].env.get("PORT"), Some(&"3000".to_owned()));
    }

    #[test]
    fn resolve_plugin_mcp_config_basic() {
        let server = PluginMcpServer {
            name: "demo".to_owned(),
            command: "node".to_owned(),
            args: vec!["server.js".to_owned()],
            env: HashMap::new(),
            plugin_name: "test".to_owned(),
            source_path: PathBuf::from("mcp.json"),
        };
        let resolved = resolve_plugin_mcp_config(&server, Path::new("/plugin"));
        assert_eq!(resolved.name, "demo");
        assert_eq!(resolved.command, "node");
    }
}
