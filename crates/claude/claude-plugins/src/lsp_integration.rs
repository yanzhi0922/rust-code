//! LSP (Language Server Protocol) integration for plugins.
//!
//! Extracts LSP server configurations from plugin directories and manifests.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// An LSP server configuration from a plugin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginLspConfig {
    /// Server name.
    pub name: String,
    /// Command to start the server.
    pub command: String,
    /// Command arguments.
    #[serde(default)]
    pub args: Vec<String>,
    /// Language file extensions this server handles.
    #[serde(default)]
    pub languages: Vec<String>,
    /// Source plugin name.
    pub plugin_name: String,
    /// Source file path.
    pub source_path: PathBuf,
}

/// Result of loading LSP servers from a plugin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoadLspResult {
    /// LSP servers found.
    pub servers: Vec<PluginLspConfig>,
    /// Errors encountered.
    pub errors: Vec<String>,
}

// ---------------------------------------------------------------------------
// Core functions
// ---------------------------------------------------------------------------

/// Load LSP server configurations from a plugin.
///
/// Checks for:
/// 1. `.lsp.json` file in plugin directory
/// 2. Manifest `lspServers` field
pub fn load_plugin_lsp_servers(plugin_name: &str, plugin_root: &Path) -> LoadLspResult {
    let mut servers = Vec::new();
    let mut errors = Vec::new();

    // 1. Check for .lsp.json file
    let lsp_json_path = plugin_root.join(".lsp.json");
    if lsp_json_path.exists() {
        match load_lsp_from_file(plugin_name, &lsp_json_path) {
            Ok(mut s) => servers.append(&mut s),
            Err(e) => {
                errors.push(format!("failed to load .lsp.json: {e}"));
            }
        }
    }

    // 2. Check for manifest lspServers field
    let manifest_path = plugin_root
        .join(crate::PLUGIN_MANIFEST_DIR)
        .join(crate::PLUGIN_MANIFEST_FILE);
    if manifest_path.exists()
        && let Ok(content) = std::fs::read_to_string(&manifest_path)
        && let Ok(raw) = serde_json::from_str::<Value>(&content)
        && let Some(lsp_servers) = raw.get("lspServers")
    {
        match parse_lsp_servers(plugin_name, lsp_servers, &manifest_path) {
            Ok(mut s) => servers.append(&mut s),
            Err(e) => {
                errors.push(format!("failed to parse manifest lspServers: {e}"));
            }
        }
    }

    LoadLspResult { servers, errors }
}

/// Load LSP servers from a `.lsp.json` file.
fn load_lsp_from_file(plugin_name: &str, lsp_path: &Path) -> Result<Vec<PluginLspConfig>, String> {
    let content = std::fs::read_to_string(lsp_path).map_err(|e| format!("read error: {e}"))?;

    let raw: Value = serde_json::from_str(&content).map_err(|e| format!("parse error: {e}"))?;

    parse_lsp_servers(plugin_name, &raw, lsp_path)
}

/// Parse LSP server entries from a JSON value.
fn parse_lsp_servers(
    plugin_name: &str,
    raw: &Value,
    source_path: &Path,
) -> Result<Vec<PluginLspConfig>, String> {
    let obj = raw
        .as_object()
        .ok_or_else(|| "expected a JSON object".to_owned())?;

    let mut servers = Vec::new();

    for (name, value) in obj {
        let entry = value
            .as_object()
            .ok_or_else(|| format!("entry '{name}' must be an object"))?;

        let command = entry
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("entry '{name}' must have 'command'"))?
            .to_owned();

        let args = entry
            .get("args")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_owned()))
                    .collect()
            })
            .unwrap_or_default();

        let languages = entry
            .get("languages")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_owned()))
                    .collect()
            })
            .unwrap_or_default();

        servers.push(PluginLspConfig {
            name: name.clone(),
            command,
            args,
            languages,
            plugin_name: plugin_name.to_owned(),
            source_path: source_path.to_path_buf(),
        });
    }

    Ok(servers)
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

    fn create_plugin_with_lsp(root: &Path, name: &str) {
        let manifest_dir = root.join(crate::PLUGIN_MANIFEST_DIR);
        fs::create_dir_all(&manifest_dir).expect("create dir");
        fs::write(
            manifest_dir.join(crate::PLUGIN_MANIFEST_FILE),
            format!(r#"{{"name":"{name}","version":"0.1.0"}}"#),
        )
        .expect("write manifest");
    }

    #[test]
    fn load_plugin_lsp_servers_from_lsp_json() {
        let temp = ok(tempdir());
        create_plugin_with_lsp(temp.path(), "test-plugin");
        fs::write(
            temp.path().join(".lsp.json"),
            r#"{
                "rust-analyzer": {
                    "command": "rust-analyzer",
                    "languages": ["rust"]
                }
            }"#,
        )
        .expect("write lsp");

        let result = load_plugin_lsp_servers("test-plugin", temp.path());
        assert_eq!(result.servers.len(), 1);
        assert!(result.errors.is_empty());
        assert_eq!(result.servers[0].name, "rust-analyzer");
        assert_eq!(result.servers[0].command, "rust-analyzer");
        assert_eq!(result.servers[0].languages, vec!["rust"]);
    }

    #[test]
    fn load_plugin_lsp_servers_no_config() {
        let temp = ok(tempdir());
        create_plugin_with_lsp(temp.path(), "test-plugin");

        let result = load_plugin_lsp_servers("test-plugin", temp.path());
        assert!(result.servers.is_empty());
        assert!(result.errors.is_empty());
    }

    #[test]
    fn load_plugin_lsp_servers_invalid_lsp_json() {
        let temp = ok(tempdir());
        create_plugin_with_lsp(temp.path(), "test-plugin");
        fs::write(temp.path().join(".lsp.json"), "not json").expect("write");

        let result = load_plugin_lsp_servers("test-plugin", temp.path());
        assert!(result.servers.is_empty());
        assert_eq!(result.errors.len(), 1);
    }

    #[test]
    fn load_plugin_lsp_servers_from_manifest() {
        let temp = ok(tempdir());
        let manifest_dir = temp.path().join(crate::PLUGIN_MANIFEST_DIR);
        fs::create_dir_all(&manifest_dir).expect("create dir");
        fs::write(
            manifest_dir.join(crate::PLUGIN_MANIFEST_FILE),
            r#"{
                "name": "test-plugin",
                "version": "0.1.0",
                "lspServers": {
                    "pyright": {
                        "command": "pyright-langserver",
                        "args": ["--stdio"],
                        "languages": ["python"]
                    }
                }
            }"#,
        )
        .expect("write manifest");

        let result = load_plugin_lsp_servers("test-plugin", temp.path());
        assert_eq!(result.servers.len(), 1);
        assert_eq!(result.servers[0].name, "pyright");
    }
}
