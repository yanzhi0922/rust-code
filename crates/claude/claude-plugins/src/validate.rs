//! Plugin validation — manifest, structure, and security checks.
//!
//! Rust equivalent of `validatePlugin.ts`. Validates plugin manifests
//! (`plugin.json`), marketplace manifests (`marketplace.json`), and
//! plugin directory structures. Detects path traversal, missing fields,
//! and common authoring mistakes.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::schemas::PluginManifestSchema;

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

/// Errors during plugin validation.
#[derive(Debug, Error)]
pub enum ValidateError {
    /// I/O error reading a file.
    #[error("validation I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// Failed to parse JSON.
    #[error("invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
}

// ---------------------------------------------------------------------------
// Validation result types
// ---------------------------------------------------------------------------

/// The type of file being validated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileType {
    /// A plugin manifest (plugin.json).
    Plugin,
    /// A marketplace manifest (marketplace.json).
    Marketplace,
    /// A skill file.
    Skill,
    /// An agent definition file.
    Agent,
    /// A command definition file.
    Command,
    /// A hooks configuration file.
    Hooks,
}

/// A single validation error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationError {
    /// JSON path or field path (e.g., `"name"`, `"commands[0]"`).
    pub path: String,
    /// Human-readable error message.
    pub message: String,
    /// Optional error code.
    #[serde(default)]
    pub code: Option<String>,
}

/// A single validation warning.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationWarning {
    /// JSON path or field path.
    pub path: String,
    /// Human-readable warning message.
    pub message: String,
}

/// The result of validating a plugin or marketplace manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginValidationResult {
    /// Whether validation passed (no errors).
    pub success: bool,
    /// Validation errors (blocking).
    pub errors: Vec<ValidationError>,
    /// Validation warnings (non-blocking).
    pub warnings: Vec<ValidationWarning>,
    /// Path to the validated file.
    pub file_path: PathBuf,
    /// Type of the validated file.
    pub file_type: FileType,
}

// ---------------------------------------------------------------------------
// Marketplace-only fields
// ---------------------------------------------------------------------------

/// Fields that belong in marketplace.json entries but NOT in plugin.json.
/// Plugin authors commonly copy one into the other; surfaced as warnings.
static MARKETPLACE_ONLY_FIELDS: &[&str] = &["category", "source", "tags", "strict", "id"];

// ---------------------------------------------------------------------------
// Manifest type detection
// ---------------------------------------------------------------------------

/// Detect whether a file is a plugin manifest or marketplace manifest.
pub fn detect_manifest_type(path: &Path) -> FileType {
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();

    if file_name == "plugin.json" {
        return FileType::Plugin;
    }
    if file_name == "marketplace.json" {
        return FileType::Marketplace;
    }

    // Check if parent directory is .codex-plugin or .claude-plugin
    if let Some(parent) = path.parent() {
        let dir_name = parent
            .file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if dir_name == ".codex-plugin" || dir_name == ".claude-plugin" {
            return FileType::Plugin;
        }
    }

    FileType::Plugin // Default assumption
}

// ---------------------------------------------------------------------------
// Path traversal checks
// ---------------------------------------------------------------------------

/// Check for parent-directory segments (`..`) in a path string.
fn check_path_traversal(
    p: &str,
    field: &str,
    errors: &mut Vec<ValidationError>,
    hint: Option<&str>,
) {
    if p.contains("..") {
        let message = match hint {
            Some(h) => format!("Path contains \"..\": {p}. {h}"),
            None => format!("Path contains \"..\" which could be a path traversal attempt: {p}"),
        };
        errors.push(ValidationError {
            path: field.to_string(),
            message,
            code: None,
        });
    }
}

/// Build a marketplace source hint for `..` in source paths.
fn marketplace_source_hint(p: &str) -> String {
    let stripped = p.replace("../", "");
    let stripped = stripped.trim_start_matches('/');
    let corrected = if stripped != p {
        format!("./{stripped}")
    } else {
        "./plugins/my-plugin".to_string()
    };
    format!(
        "Plugin source paths are resolved relative to the marketplace root \
         (the directory containing .claude-plugin/), not relative to \
         marketplace.json. Use \"{corrected}\" instead of \"{p}\"."
    )
}

// ---------------------------------------------------------------------------
// Plugin manifest validation
// ---------------------------------------------------------------------------

/// Validate a plugin manifest file (`plugin.json`).
///
/// Reads the file, parses JSON, checks for path traversal in component
/// paths, validates against the schema, and produces warnings for common
/// authoring mistakes.
pub fn validate_plugin_manifest(file_path: &Path) -> PluginValidationResult {
    let absolute = if file_path.is_absolute() {
        file_path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|d| d.join(file_path))
            .unwrap_or_else(|_| file_path.to_path_buf())
    };

    // Read file
    let content = match std::fs::read_to_string(&absolute) {
        Ok(c) => c,
        Err(e) => {
            let kind = e.kind();
            let message = if kind == std::io::ErrorKind::NotFound {
                format!("File not found: {}", absolute.display())
            } else if kind == std::io::ErrorKind::IsADirectory {
                format!("Path is not a file: {}", absolute.display())
            } else {
                format!("Failed to read file: {e}")
            };
            return PluginValidationResult {
                success: false,
                errors: vec![ValidationError {
                    path: "file".into(),
                    message,
                    code: Some(format!("{kind:?}")),
                }],
                warnings: vec![],
                file_path: absolute,
                file_type: FileType::Plugin,
            };
        }
    };

    // Parse JSON
    let parsed: Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(e) => {
            return PluginValidationResult {
                success: false,
                errors: vec![ValidationError {
                    path: "json".into(),
                    message: format!("Invalid JSON syntax: {e}"),
                    code: None,
                }],
                warnings: vec![],
                file_path: absolute,
                file_type: FileType::Plugin,
            };
        }
    };

    let mut errors: Vec<ValidationError> = Vec::new();
    let mut warnings: Vec<ValidationWarning> = Vec::new();

    // Check path traversal in component paths
    if let Value::Object(ref obj) = parsed {
        // Check commands
        if let Some(cmds) = obj.get("commands") {
            let items = array_or_single(cmds);
            for (i, item) in items.iter().enumerate() {
                if let Value::String(s) = item {
                    check_path_traversal(s, &format!("commands[{i}]"), &mut errors, None);
                }
            }
        }

        // Check agents
        if let Some(agents) = obj.get("agents") {
            let items = array_or_single(agents);
            for (i, item) in items.iter().enumerate() {
                if let Value::String(s) = item {
                    check_path_traversal(s, &format!("agents[{i}]"), &mut errors, None);
                }
            }
        }

        // Check skills
        if let Some(skills) = obj.get("skills") {
            let items = array_or_single(skills);
            for (i, item) in items.iter().enumerate() {
                if let Value::String(s) = item {
                    check_path_traversal(s, &format!("skills[{i}]"), &mut errors, None);
                }
            }
        }
    }

    // Surface marketplace-only fields as warnings
    let mut to_validate = parsed.clone();
    if let Value::Object(ref obj) = parsed {
        let stray_keys: Vec<&str> = obj
            .keys()
            .map(|k| k.as_str())
            .filter(|k| MARKETPLACE_ONLY_FIELDS.contains(k))
            .collect();

        if !stray_keys.is_empty()
            && let Value::Object(ref mut map) = to_validate
        {
            for key in &stray_keys {
                map.remove(*key);
                warnings.push(ValidationWarning {
                    path: key.to_string(),
                    message: format!(
                        "Field '{key}' belongs in the marketplace entry \
                             (marketplace.json), not plugin.json. It's harmless \
                             here but unused — the runtime ignores it at load time."
                    ),
                });
            }
        }
    }

    // Validate against schema
    let schema_result: Result<PluginManifestSchema, _> = serde_json::from_value(to_validate);
    if let Err(e) = schema_result {
        errors.push(ValidationError {
            path: "schema".into(),
            message: format!("Schema validation failed: {e}"),
            code: None,
        });
    }

    // If schema passed, check for common warnings
    if let Ok(manifest) = serde_json::from_value::<PluginManifestSchema>(parsed.clone()) {
        // Warn if name isn't strict kebab-case
        let kebab_re = regex::Regex::new(r"^[a-z0-9]+(-[a-z0-9]+)*$").expect("valid regex");
        if !kebab_re.is_match(&manifest.name) {
            warnings.push(ValidationWarning {
                path: "name".into(),
                message: format!(
                    "Plugin name \"{}\" is not kebab-case. The marketplace sync \
                     requires kebab-case (lowercase letters, digits, and hyphens only).",
                    manifest.name
                ),
            });
        }

        // Warn if no version
        if manifest.version.is_none() || manifest.version.as_deref() == Some("") {
            warnings.push(ValidationWarning {
                path: "version".into(),
                message: "No version specified. Consider adding a version following semver (e.g., \"1.0.0\")".into(),
            });
        }

        // Warn if no description
        if manifest.description.is_none() || manifest.description.as_deref() == Some("") {
            warnings.push(ValidationWarning {
                path: "description".into(),
                message: "No description provided. Adding a description helps users understand what your plugin does".into(),
            });
        }

        // Warn if no author
        if manifest.author.is_none() {
            warnings.push(ValidationWarning {
                path: "author".into(),
                message: "No author information provided. Consider adding author details for plugin attribution".into(),
            });
        }
    }

    PluginValidationResult {
        success: errors.is_empty(),
        errors,
        warnings,
        file_path: absolute,
        file_type: FileType::Plugin,
    }
}

// ---------------------------------------------------------------------------
// Marketplace manifest validation
// ---------------------------------------------------------------------------

/// Validate a marketplace manifest file (`marketplace.json`).
pub fn validate_marketplace_manifest(file_path: &Path) -> PluginValidationResult {
    let absolute = if file_path.is_absolute() {
        file_path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|d| d.join(file_path))
            .unwrap_or_else(|_| file_path.to_path_buf())
    };

    // Read file
    let content = match std::fs::read_to_string(&absolute) {
        Ok(c) => c,
        Err(e) => {
            let kind = e.kind();
            let message = if kind == std::io::ErrorKind::NotFound {
                format!("File not found: {}", absolute.display())
            } else {
                format!("Failed to read file: {e}")
            };
            return PluginValidationResult {
                success: false,
                errors: vec![ValidationError {
                    path: "file".into(),
                    message,
                    code: Some(format!("{kind:?}")),
                }],
                warnings: vec![],
                file_path: absolute,
                file_type: FileType::Marketplace,
            };
        }
    };

    // Parse JSON
    let parsed: Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(e) => {
            return PluginValidationResult {
                success: false,
                errors: vec![ValidationError {
                    path: "json".into(),
                    message: format!("Invalid JSON syntax: {e}"),
                    code: None,
                }],
                warnings: vec![],
                file_path: absolute,
                file_type: FileType::Marketplace,
            };
        }
    };

    let mut errors: Vec<ValidationError> = Vec::new();
    let mut warnings: Vec<ValidationWarning> = Vec::new();

    // Check path traversal in plugin sources
    if let Value::Object(ref obj) = parsed
        && let Some(Value::Array(plugins)) = obj.get("plugins")
    {
        for (i, plugin) in plugins.iter().enumerate() {
            if let Some(source) = plugin.get("source") {
                // String sources (relative paths)
                if let Value::String(s) = source {
                    check_path_traversal(
                        s,
                        &format!("plugins[{i}].source"),
                        &mut errors,
                        Some(&marketplace_source_hint(s)),
                    );
                }
                // Object source with .path
                if let Value::Object(src_map) = source
                    && let Some(Value::String(p)) = src_map.get("path")
                {
                    check_path_traversal(
                        p,
                        &format!("plugins[{i}].source.path"),
                        &mut errors,
                        None,
                    );
                }
            }
        }

        // Check for duplicate plugin names
        let mut seen_names: HashSet<String> = HashSet::new();
        for (i, plugin) in plugins.iter().enumerate() {
            if let Some(Value::String(name)) = plugin.get("name") {
                if seen_names.contains(name) {
                    errors.push(ValidationError {
                        path: format!("plugins[{i}].name"),
                        message: format!("Duplicate plugin name \"{name}\" found in marketplace"),
                        code: None,
                    });
                }
                seen_names.insert(name.clone());
            }
        }

        // Warn if no plugins
        if plugins.is_empty() {
            warnings.push(ValidationWarning {
                path: "plugins".into(),
                message: "Marketplace has no plugins defined".into(),
            });
        }
    }

    PluginValidationResult {
        success: errors.is_empty(),
        errors,
        warnings,
        file_path: absolute,
        file_type: FileType::Marketplace,
    }
}

// ---------------------------------------------------------------------------
// Directory structure validation
// ---------------------------------------------------------------------------

/// Validate the directory structure of a plugin.
///
/// Checks for:
/// - Presence of `plugin.json` (or `.codex-plugin/plugin.json`)
/// - No disabled marker (`.remote-code-disabled`)
/// - No path traversal in directory names
pub fn validate_plugin_structure(plugin_dir: &Path) -> PluginValidationResult {
    let mut errors: Vec<ValidationError> = Vec::new();
    let mut warnings: Vec<ValidationWarning> = Vec::new();

    // Check directory exists
    if !plugin_dir.exists() {
        errors.push(ValidationError {
            path: "dir".into(),
            message: format!("Plugin directory not found: {}", plugin_dir.display()),
            code: None,
        });
        return PluginValidationResult {
            success: false,
            errors,
            warnings,
            file_path: plugin_dir.to_path_buf(),
            file_type: FileType::Plugin,
        };
    }

    if !plugin_dir.is_dir() {
        errors.push(ValidationError {
            path: "dir".into(),
            message: format!("Path is not a directory: {}", plugin_dir.display()),
            code: None,
        });
        return PluginValidationResult {
            success: false,
            errors,
            warnings,
            file_path: plugin_dir.to_path_buf(),
            file_type: FileType::Plugin,
        };
    }

    // Check for plugin.json
    let manifest_paths = [
        plugin_dir.join("plugin.json"),
        plugin_dir.join(".codex-plugin").join("plugin.json"),
        plugin_dir.join(".claude-plugin").join("plugin.json"),
    ];

    let has_manifest = manifest_paths.iter().any(|p| p.exists());
    if !has_manifest {
        errors.push(ValidationError {
            path: "plugin.json".into(),
            message: "No plugin.json manifest found. Expected at root or in .codex-plugin/".into(),
            code: None,
        });
    }

    // Check for disabled marker
    let disabled_marker = plugin_dir.join(crate::PLUGIN_DISABLED_MARKER);
    if disabled_marker.exists() {
        warnings.push(ValidationWarning {
            path: crate::PLUGIN_DISABLED_MARKER.into(),
            message: "Plugin is disabled (disabled marker file present)".into(),
        });
    }

    // Check for path traversal in subdirectory names
    if let Ok(entries) = std::fs::read_dir(plugin_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.contains("..") {
                errors.push(ValidationError {
                    path: name_str.to_string(),
                    message: format!("Directory/file name contains \"..\": {name_str}"),
                    code: None,
                });
            }
        }
    }

    PluginValidationResult {
        success: errors.is_empty(),
        errors,
        warnings,
        file_path: plugin_dir.to_path_buf(),
        file_type: FileType::Plugin,
    }
}

// ---------------------------------------------------------------------------
// Full plugin validation
// ---------------------------------------------------------------------------

/// Validate a plugin completely: manifest + structure.
///
/// This is the top-level validation function that checks both the
/// directory structure and the manifest content.
pub fn validate_plugin(plugin_dir: &Path) -> PluginValidationResult {
    let mut structure_result = validate_plugin_structure(plugin_dir);

    // Find manifest path
    let manifest_paths = [
        plugin_dir.join("plugin.json"),
        plugin_dir.join(".codex-plugin").join("plugin.json"),
        plugin_dir.join(".claude-plugin").join("plugin.json"),
    ];

    for manifest_path in &manifest_paths {
        if manifest_path.exists() {
            let manifest_result = validate_plugin_manifest(manifest_path);
            // Merge results
            structure_result.errors.extend(manifest_result.errors);
            structure_result.warnings.extend(manifest_result.warnings);
            break;
        }
    }

    structure_result.success = structure_result.errors.is_empty();
    structure_result
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Convert a JSON value to a Vec — if it's an array, return its elements;
/// if it's a single value, wrap it in a one-element Vec.
fn array_or_single(value: &Value) -> Vec<Value> {
    match value {
        Value::Array(arr) => arr.clone(),
        other => vec![other.clone()],
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    // -- detect_manifest_type --

    #[test]
    fn detect_plugin_json() {
        let p = PathBuf::from("some/path/plugin.json");
        assert_eq!(detect_manifest_type(&p), FileType::Plugin);
    }

    #[test]
    fn detect_marketplace_json() {
        let p = PathBuf::from("some/path/marketplace.json");
        assert_eq!(detect_manifest_type(&p), FileType::Marketplace);
    }

    #[test]
    fn detect_in_codex_plugin_dir() {
        let p = PathBuf::from(".codex-plugin/something.json");
        assert_eq!(detect_manifest_type(&p), FileType::Plugin);
    }

    // -- validate_plugin_manifest --

    #[test]
    fn validate_valid_manifest() {
        let dir = TempDir::new().expect("tempdir");
        let manifest = dir.path().join("plugin.json");
        fs::write(
            &manifest,
            r#"{"name":"my-plugin","version":"1.0.0","description":"A test plugin"}"#,
        )
        .expect("write");

        let result = validate_plugin_manifest(&manifest);
        assert!(result.success, "Errors: {:?}", result.errors);
    }

    #[test]
    fn validate_missing_file() {
        let p = PathBuf::from("/nonexistent/plugin.json");
        let result = validate_plugin_manifest(&p);
        assert!(!result.success);
        assert!(result.errors[0].message.contains("not found"));
    }

    #[test]
    fn validate_invalid_json() {
        let dir = TempDir::new().expect("tempdir");
        let manifest = dir.path().join("plugin.json");
        fs::write(&manifest, "{invalid json}").expect("write");

        let result = validate_plugin_manifest(&manifest);
        assert!(!result.success);
        assert!(result.errors[0].message.contains("Invalid JSON"));
    }

    #[test]
    fn validate_path_traversal_in_commands() {
        let dir = TempDir::new().expect("tempdir");
        let manifest = dir.path().join("plugin.json");
        fs::write(
            &manifest,
            r#"{"name":"p","version":"1.0.0","commands":["../../../etc/passwd"]}"#,
        )
        .expect("write");

        let result = validate_plugin_manifest(&manifest);
        assert!(!result.success);
        assert!(result.errors.iter().any(|e| e.path == "commands[0]"));
    }

    #[test]
    fn validate_path_traversal_in_skills() {
        let dir = TempDir::new().expect("tempdir");
        let manifest = dir.path().join("plugin.json");
        fs::write(
            &manifest,
            r#"{"name":"p","version":"1.0.0","skills":["../../hidden"]}"#,
        )
        .expect("write");

        let result = validate_plugin_manifest(&manifest);
        assert!(!result.success);
        assert!(result.errors.iter().any(|e| e.path == "skills[0]"));
    }

    #[test]
    fn validate_marketplace_only_fields_warning() {
        let dir = TempDir::new().expect("tempdir");
        let manifest = dir.path().join("plugin.json");
        fs::write(
            &manifest,
            r#"{"name":"p","version":"1.0.0","category":"tools","source":"./src"}"#,
        )
        .expect("write");

        let result = validate_plugin_manifest(&manifest);
        let warning_paths: Vec<&str> = result.warnings.iter().map(|w| w.path.as_str()).collect();
        assert!(warning_paths.contains(&"category"));
        assert!(warning_paths.contains(&"source"));
    }

    #[test]
    fn validate_warns_no_version() {
        let dir = TempDir::new().expect("tempdir");
        let manifest = dir.path().join("plugin.json");
        fs::write(&manifest, r#"{"name":"p"}"#).expect("write");

        let result = validate_plugin_manifest(&manifest);
        assert!(result.warnings.iter().any(|w| w.path == "version"));
    }

    #[test]
    fn validate_warns_no_description() {
        let dir = TempDir::new().expect("tempdir");
        let manifest = dir.path().join("plugin.json");
        fs::write(&manifest, r#"{"name":"p","version":"1.0.0"}"#).expect("write");

        let result = validate_plugin_manifest(&manifest);
        assert!(result.warnings.iter().any(|w| w.path == "description"));
    }

    #[test]
    fn validate_warns_non_kebab_name() {
        let dir = TempDir::new().expect("tempdir");
        let manifest = dir.path().join("plugin.json");
        fs::write(
            &manifest,
            r#"{"name":"My_Plugin","version":"1.0.0","description":"test"}"#,
        )
        .expect("write");

        let result = validate_plugin_manifest(&manifest);
        assert!(result.warnings.iter().any(|w| w.path == "name"));
    }

    // -- validate_marketplace_manifest --

    #[test]
    fn validate_valid_marketplace() {
        let dir = TempDir::new().expect("tempdir");
        let manifest = dir.path().join("marketplace.json");
        fs::write(
            &manifest,
            r#"{"plugins":[{"name":"p1","source":"./plugins/p1"}]}"#,
        )
        .expect("write");

        let result = validate_marketplace_manifest(&manifest);
        assert!(result.success, "Errors: {:?}", result.errors);
    }

    #[test]
    fn validate_marketplace_path_traversal() {
        let dir = TempDir::new().expect("tempdir");
        let manifest = dir.path().join("marketplace.json");
        fs::write(
            &manifest,
            r#"{"plugins":[{"name":"p1","source":"../outside"}]}"#,
        )
        .expect("write");

        let result = validate_marketplace_manifest(&manifest);
        assert!(!result.success);
        assert!(result.errors.iter().any(|e| e.path.contains("source")));
    }

    #[test]
    fn validate_marketplace_duplicate_names() {
        let dir = TempDir::new().expect("tempdir");
        let manifest = dir.path().join("marketplace.json");
        fs::write(
            &manifest,
            r#"{"plugins":[{"name":"p1","source":"./a"},{"name":"p1","source":"./b"}]}"#,
        )
        .expect("write");

        let result = validate_marketplace_manifest(&manifest);
        assert!(!result.success);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.message.contains("Duplicate"))
        );
    }

    #[test]
    fn validate_marketplace_empty_plugins_warning() {
        let dir = TempDir::new().expect("tempdir");
        let manifest = dir.path().join("marketplace.json");
        fs::write(&manifest, r#"{"plugins":[]}"#).expect("write");

        let result = validate_marketplace_manifest(&manifest);
        assert!(result.warnings.iter().any(|w| w.path == "plugins"));
    }

    // -- validate_plugin_structure --

    #[test]
    fn validate_structure_valid() {
        let dir = TempDir::new().expect("tempdir");
        fs::write(dir.path().join("plugin.json"), r#"{"name":"p"}"#).expect("write");

        let result = validate_plugin_structure(dir.path());
        assert!(result.success, "Errors: {:?}", result.errors);
    }

    #[test]
    fn validate_structure_missing_dir() {
        let result = validate_plugin_structure(Path::new("/nonexistent/dir"));
        assert!(!result.success);
    }

    #[test]
    fn validate_structure_not_a_dir() {
        let dir = TempDir::new().expect("tempdir");
        let file = dir.path().join("not_a_dir");
        fs::write(&file, "content").expect("write");

        let result = validate_plugin_structure(&file);
        assert!(!result.success);
    }

    #[test]
    fn validate_structure_missing_manifest() {
        let dir = TempDir::new().expect("tempdir");
        fs::create_dir(dir.path().join("skills")).expect("dir");

        let result = validate_plugin_structure(dir.path());
        assert!(!result.success);
        assert!(result.errors.iter().any(|e| e.path == "plugin.json"));
    }

    #[test]
    fn validate_structure_disabled_warning() {
        let dir = TempDir::new().expect("tempdir");
        fs::write(dir.path().join("plugin.json"), r#"{"name":"p"}"#).expect("write");
        fs::write(dir.path().join(crate::PLUGIN_DISABLED_MARKER), "").expect("write");

        let result = validate_plugin_structure(dir.path());
        assert!(result.success);
        assert!(
            result
                .warnings
                .iter()
                .any(|w| w.path == crate::PLUGIN_DISABLED_MARKER)
        );
    }

    #[test]
    fn validate_structure_codex_plugin_dir() {
        let dir = TempDir::new().expect("tempdir");
        let codex_dir = dir.path().join(".codex-plugin");
        fs::create_dir(&codex_dir).expect("dir");
        fs::write(codex_dir.join("plugin.json"), r#"{"name":"p"}"#).expect("write");

        let result = validate_plugin_structure(dir.path());
        assert!(result.success, "Errors: {:?}", result.errors);
    }

    // -- validate_plugin (full) --

    #[test]
    fn full_validation_passes() {
        let dir = TempDir::new().expect("tempdir");
        fs::write(
            dir.path().join("plugin.json"),
            r#"{"name":"my-plugin","version":"1.0.0","description":"Test","author":{"name":"Test Author"}}"#,
        )
        .expect("write");

        let result = validate_plugin(dir.path());
        assert!(
            result.success,
            "Errors: {:?}, Warnings: {:?}",
            result.errors, result.warnings
        );
    }

    #[test]
    fn full_validation_catches_both_structure_and_manifest() {
        let dir = TempDir::new().expect("tempdir");
        // No plugin.json → structure error
        let result = validate_plugin(dir.path());
        assert!(!result.success);
    }
}
