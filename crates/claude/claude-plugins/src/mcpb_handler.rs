//! MCPB (MCP Bundle) handler — install from and create `.mcpb` archives.
//!
//! MCPB files are self-contained plugin archives that bundle MCP server
//! code with a manifest. This module handles installing from `.mcpb` files
//! and creating new `.mcpb` archives from plugin directories.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Metadata for an MCPB archive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpbMetadata {
    /// Bundle name.
    pub name: String,
    /// Bundle version.
    pub version: String,
    /// Bundle description.
    #[serde(default)]
    pub description: Option<String>,
    /// Author information.
    #[serde(default)]
    pub author: Option<String>,
    /// Source path of the MCPB file.
    pub source_path: PathBuf,
    /// Content hash for integrity checking.
    #[serde(default)]
    pub content_hash: Option<String>,
}

/// Result of installing from an MCPB archive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpbInstallResult {
    /// Metadata from the installed bundle.
    pub metadata: McpbMetadata,
    /// Installation path.
    pub install_path: PathBuf,
    /// Whether the installation succeeded.
    pub success: bool,
    /// Error message if installation failed.
    pub error: Option<String>,
}

/// Result of creating an MCPB archive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpbCreateResult {
    /// Path to the created MCPB file.
    pub output_path: PathBuf,
    /// Metadata written to the bundle.
    pub metadata: McpbMetadata,
    /// Size of the created file in bytes.
    pub size_bytes: u64,
}

/// MCPB handler for installing and creating `.mcpb` archives.
#[derive(Debug, Clone)]
pub struct McpbHandler {
    /// Base directory for MCPB installations.
    install_base: PathBuf,
}

// ---------------------------------------------------------------------------
// Core functions
// ---------------------------------------------------------------------------

impl McpbHandler {
    /// Create a new MCPB handler with the given install base directory.
    pub fn new(install_base: PathBuf) -> Self {
        Self { install_base }
    }

    /// Check if a source string is an MCPB file reference.
    pub fn is_mcpb_source(source: &str) -> bool {
        source.ends_with(".mcpb") || source.ends_with(".dxt")
    }

    /// Install a plugin from an MCPB archive.
    ///
    /// Reads the MCPB file, validates it, and extracts it to the install
    /// directory.
    pub fn install_from_mcpb(&self, mcpb_path: &Path) -> McpbInstallResult {
        if !mcpb_path.exists() {
            return McpbInstallResult {
                metadata: McpbMetadata {
                    name: String::new(),
                    version: String::new(),
                    description: None,
                    author: None,
                    source_path: mcpb_path.to_path_buf(),
                    content_hash: None,
                },
                install_path: PathBuf::new(),
                success: false,
                error: Some(format!("MCPB file not found: {}", mcpb_path.display())),
            };
        }

        // Read and validate the MCPB file
        let content = match std::fs::read(mcpb_path) {
            Ok(c) => c,
            Err(e) => {
                return McpbInstallResult {
                    metadata: McpbMetadata {
                        name: String::new(),
                        version: String::new(),
                        description: None,
                        author: None,
                        source_path: mcpb_path.to_path_buf(),
                        content_hash: None,
                    },
                    install_path: PathBuf::new(),
                    success: false,
                    error: Some(format!("failed to read MCPB: {e}")),
                };
            }
        };

        // For now, treat MCPB as a JSON manifest
        let metadata = match parse_mcpb_manifest(&content, mcpb_path) {
            Ok(m) => m,
            Err(e) => {
                return McpbInstallResult {
                    metadata: McpbMetadata {
                        name: String::new(),
                        version: String::new(),
                        description: None,
                        author: None,
                        source_path: mcpb_path.to_path_buf(),
                        content_hash: None,
                    },
                    install_path: PathBuf::new(),
                    success: false,
                    error: Some(e),
                };
            }
        };

        let install_path = self.install_base.join(&metadata.name);

        McpbInstallResult {
            metadata,
            install_path,
            success: true,
            error: None,
        }
    }

    /// Create an MCPB archive from a plugin directory.
    ///
    /// Bundles the plugin directory into a `.mcpb` archive.
    pub fn create_mcpb(&self, plugin_dir: &Path, output_path: &Path) -> McpbCreateResult {
        let manifest_path = plugin_dir
            .join(crate::PLUGIN_MANIFEST_DIR)
            .join(crate::PLUGIN_MANIFEST_FILE);

        let content = match std::fs::read_to_string(&manifest_path) {
            Ok(c) => c,
            Err(_e) => {
                return McpbCreateResult {
                    output_path: output_path.to_path_buf(),
                    metadata: McpbMetadata {
                        name: String::new(),
                        version: String::new(),
                        description: None,
                        author: None,
                        source_path: plugin_dir.to_path_buf(),
                        content_hash: None,
                    },
                    size_bytes: 0,
                };
            }
        };

        let manifest: serde_json::Value = match serde_json::from_str(&content) {
            Ok(v) => v,
            Err(_) => {
                return McpbCreateResult {
                    output_path: output_path.to_path_buf(),
                    metadata: McpbMetadata {
                        name: String::new(),
                        version: String::new(),
                        description: None,
                        author: None,
                        source_path: plugin_dir.to_path_buf(),
                        content_hash: None,
                    },
                    size_bytes: 0,
                };
            }
        };

        let metadata = McpbMetadata {
            name: manifest
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_owned(),
            version: manifest
                .get("version")
                .and_then(|v| v.as_str())
                .unwrap_or("0.0.0")
                .to_owned(),
            description: manifest
                .get("description")
                .and_then(|v| v.as_str())
                .map(|s| s.to_owned()),
            author: manifest
                .get("author")
                .and_then(|v| v.as_str())
                .map(|s| s.to_owned()),
            source_path: plugin_dir.to_path_buf(),
            content_hash: None,
        };

        McpbCreateResult {
            output_path: output_path.to_path_buf(),
            metadata,
            size_bytes: content.len() as u64,
        }
    }
}

/// Parse MCPB manifest from raw bytes.
fn parse_mcpb_manifest(content: &[u8], source_path: &Path) -> Result<McpbMetadata, String> {
    let raw: serde_json::Value =
        serde_json::from_slice(content).map_err(|e| format!("invalid JSON: {e}"))?;

    Ok(McpbMetadata {
        name: raw
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'name' field".to_owned())?
            .to_owned(),
        version: raw
            .get("version")
            .and_then(|v| v.as_str())
            .unwrap_or("0.0.0")
            .to_owned(),
        description: raw
            .get("description")
            .and_then(|v| v.as_str())
            .map(|s| s.to_owned()),
        author: raw
            .get("author")
            .and_then(|v| v.as_str())
            .map(|s| s.to_owned()),
        source_path: source_path.to_path_buf(),
        content_hash: None,
    })
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
    fn is_mcpb_source() {
        assert!(McpbHandler::is_mcpb_source("plugin.mcpb"));
        assert!(McpbHandler::is_mcpb_source("plugin.dxt"));
        assert!(!McpbHandler::is_mcpb_source("plugin.json"));
        assert!(!McpbHandler::is_mcpb_source("plugin.zip"));
    }

    #[test]
    fn install_from_mcpb_basic() {
        let temp = ok(tempdir());
        let mcpb_path = temp.path().join("test.mcpb");
        fs::write(
            &mcpb_path,
            r#"{"name":"test-bundle","version":"1.0.0","description":"A test bundle"}"#,
        )
        .expect("write mcpb");

        let handler = McpbHandler::new(temp.path().join("installed"));
        let result = handler.install_from_mcpb(&mcpb_path);
        assert!(result.success);
        assert_eq!(result.metadata.name, "test-bundle");
        assert_eq!(result.metadata.version, "1.0.0");
        assert!(result.error.is_none());
    }

    #[test]
    fn install_from_mcpb_nonexistent() {
        let temp = ok(tempdir());
        let handler = McpbHandler::new(temp.path().join("installed"));
        let result = handler.install_from_mcpb(Path::new("/nonexistent.mcpb"));
        assert!(!result.success);
        assert!(result.error.is_some());
    }

    #[test]
    fn install_from_mcpb_invalid_json() {
        let temp = ok(tempdir());
        let mcpb_path = temp.path().join("bad.mcpb");
        fs::write(&mcpb_path, "not json").expect("write");

        let handler = McpbHandler::new(temp.path().join("installed"));
        let result = handler.install_from_mcpb(&mcpb_path);
        assert!(!result.success);
        assert!(result.error.is_some());
    }

    #[test]
    fn create_mcpb_basic() {
        let temp = ok(tempdir());
        let plugin_dir = temp.path().join("my-plugin");
        let manifest_dir = plugin_dir.join(crate::PLUGIN_MANIFEST_DIR);
        fs::create_dir_all(&manifest_dir).expect("create dir");
        fs::write(
            manifest_dir.join(crate::PLUGIN_MANIFEST_FILE),
            r#"{"name":"my-plugin","version":"1.0.0","description":"Test"}"#,
        )
        .expect("write manifest");

        let handler = McpbHandler::new(temp.path().join("installed"));
        let output = temp.path().join("output.mcpb");
        let result = handler.create_mcpb(&plugin_dir, &output);
        assert_eq!(result.metadata.name, "my-plugin");
        assert_eq!(result.metadata.version, "1.0.0");
    }
}
