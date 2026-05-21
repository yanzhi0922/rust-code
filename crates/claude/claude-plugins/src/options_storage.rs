//! Plugin options storage and substitution.
//!
//! Manages per-plugin JSON options files. Plugins declare user-configurable
//! options in `manifest.userConfig`. Storage splits by `sensitive`:
//! - `sensitive: true` → secure storage (keychain / credentials file)
//! - everything else → per-plugin JSON options file
//!
//! This module provides a simplified file-based storage layer suitable for
//! the Rust implementation, with support for loading, saving, getting, and
//! setting individual options.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::directories::PluginDirectoryManager;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Per-plugin option values stored as a JSON object.
pub type PluginOptionValues = BTreeMap<String, Value>;

/// Per-plugin option schema (maps option key to its config).
pub type PluginOptionSchema = BTreeMap<String, PluginOptionConfig>;

/// Configuration for a single plugin option.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginOptionConfig {
    /// Type of the configuration value.
    #[serde(rename = "type")]
    pub option_type: String,
    /// Human-readable label.
    pub title: String,
    /// Help text.
    pub description: String,
    /// If true, validation fails when empty.
    #[serde(default)]
    pub required: Option<bool>,
    /// Default value.
    #[serde(default)]
    pub default: Option<Value>,
    /// If true, masks input and stores in secure storage.
    #[serde(default)]
    pub sensitive: Option<bool>,
}

// ---------------------------------------------------------------------------
// PluginOptionsStore
// ---------------------------------------------------------------------------

/// Per-plugin JSON options file manager.
///
/// Each plugin's options are stored in a JSON file within the plugin data
/// directory. This store provides load/save/get/set operations.
#[derive(Debug, Clone)]
pub struct PluginOptionsStore {
    /// Directory manager for resolving paths.
    dir_manager: PluginDirectoryManager,
}

impl PluginOptionsStore {
    /// Create a new options store using the given directory manager.
    pub fn new(dir_manager: PluginDirectoryManager) -> Self {
        Self { dir_manager }
    }

    /// Load all option values for a plugin from its options file.
    ///
    /// Returns an empty map if the file doesn't exist.
    pub fn load_options(&self, plugin_id: &str) -> Result<PluginOptionValues> {
        let path = self.options_file_path(plugin_id);
        if !path.exists() {
            return Ok(BTreeMap::new());
        }

        let content = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read options file: {}", path.display()))?;

        let values: PluginOptionValues = serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse options file: {}", path.display()))?;

        Ok(values)
    }

    /// Save option values for a plugin, splitting by sensitive flag.
    ///
    /// Non-sensitive values go to the per-plugin JSON file.
    /// Sensitive values are noted but not persisted to disk by this
    /// module (they should be handled by a secure storage mechanism).
    pub fn save_options(
        &self,
        plugin_id: &str,
        values: &PluginOptionValues,
        schema: &PluginOptionSchema,
    ) -> Result<()> {
        let non_sensitive = self.filter_non_sensitive(values, schema);

        let path = self.options_file_path(plugin_id);

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create options directory: {}", parent.display())
            })?;
        }

        let content = serde_json::to_string_pretty(&non_sensitive)
            .with_context(|| "Failed to serialize plugin options")?;

        fs::write(&path, &content)
            .with_context(|| format!("Failed to write options file: {}", path.display()))?;

        Ok(())
    }

    /// Get a single option value for a plugin.
    pub fn get_option(&self, plugin_id: &str, key: &str) -> Result<Option<Value>> {
        let options = self.load_options(plugin_id)?;
        Ok(options.get(key).cloned())
    }

    /// Set a single option value for a plugin.
    ///
    /// Loads existing options, sets the value, and saves back.
    /// If the key is marked as sensitive in the schema, it is NOT written
    /// to the JSON file.
    pub fn set_option(
        &self,
        plugin_id: &str,
        key: &str,
        value: Value,
        schema: Option<&PluginOptionSchema>,
    ) -> Result<()> {
        let mut options = self.load_options(plugin_id)?;

        // Check if this key is sensitive — if so, skip writing to file
        if let Some(s) = schema
            && s.get(key).and_then(|c| c.sensitive).unwrap_or(false)
        {
            // Sensitive values should go to secure storage, not here
            // Remove from non-sensitive store if present
            options.remove(key);
            let filtered = self.filter_non_sensitive(&options, s);
            self.save_options(plugin_id, &filtered, s)?;
            return Ok(());
        }

        options.insert(key.to_string(), value);

        match schema {
            Some(s) => self.save_options(plugin_id, &options, s)?,
            None => self.save_all_options(plugin_id, &options)?,
        }

        Ok(())
    }

    /// Delete all stored option values for a plugin.
    ///
    /// Best-effort: failure is logged but does not throw.
    pub fn delete_options(&self, plugin_id: &str) {
        let path = self.options_file_path(plugin_id);
        if path.exists()
            && let Err(e) = fs::remove_file(&path)
        {
            tracing::warn!("Failed to delete options file {}: {e}", path.display());
        }
    }

    /// Get the path to a plugin's options file.
    pub fn options_file_path(&self, plugin_id: &str) -> PathBuf {
        self.dir_manager
            .plugin_data_dir_path(plugin_id)
            .join("options.json")
    }

    // -----------------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------------

    /// Filter out sensitive values from the options map.
    fn filter_non_sensitive(
        &self,
        values: &PluginOptionValues,
        schema: &PluginOptionSchema,
    ) -> PluginOptionValues {
        values
            .iter()
            .filter(|(key, _)| !schema.get(*key).and_then(|c| c.sensitive).unwrap_or(false))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// Save all options without schema-based filtering.
    fn save_all_options(&self, plugin_id: &str, values: &PluginOptionValues) -> Result<()> {
        let path = self.options_file_path(plugin_id);

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create options directory: {}", parent.display())
            })?;
        }

        let content = serde_json::to_string_pretty(values)
            .with_context(|| "Failed to serialize plugin options")?;

        fs::write(&path, &content)
            .with_context(|| format!("Failed to write options file: {}", path.display()))?;

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Variable substitution helpers
// ---------------------------------------------------------------------------

/// Substitute `${CLAUDE_PLUGIN_ROOT}` and `${CLAUDE_PLUGIN_DATA}` with their
/// paths.
///
/// On Windows, normalizes backslashes to forward slashes so shell commands
/// don't interpret them as escape characters.
pub fn substitute_plugin_variables(
    value: &str,
    plugin_path: &Path,
    plugin_source: Option<&str>,
) -> String {
    let normalize = |p: &Path| -> String {
        let s = p.to_string_lossy().to_string();
        if cfg!(windows) {
            s.replace('\\', "/")
        } else {
            s
        }
    };

    let out = value.replace("${CLAUDE_PLUGIN_ROOT}", &normalize(plugin_path));

    // Only substitute CLAUDE_PLUGIN_DATA if source is available
    match plugin_source {
        Some(source) => {
            let mgr = PluginDirectoryManager::new();
            let data_dir = mgr.plugin_data_dir_path(source);
            out.replace("${CLAUDE_PLUGIN_DATA}", &normalize(&data_dir))
        }
        None => out,
    }
}

/// Substitute `${user_config.KEY}` with saved option values.
///
/// Throws (returns Err) on missing keys — callers should only pass this
/// after validation succeeded.
pub fn substitute_user_config_variables(
    value: &str,
    user_config: &PluginOptionValues,
) -> Result<String> {
    let mut result = value.to_string();
    let mut start = 0;

    while let Some(begin) = result[start..].find("${user_config.") {
        let abs_begin = start + begin;
        let after_prefix = abs_begin + "${user_config.".len();

        if let Some(end) = result[after_prefix..].find('}') {
            let abs_end = after_prefix + end;
            let key = &result[after_prefix..abs_end];

            let config_value = user_config.get(key).ok_or_else(|| {
                anyhow::anyhow!(
                    "Missing required user configuration value: {key}. \
                         This should have been validated before variable substitution."
                )
            })?;

            let replacement = match config_value {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };

            result.replace_range(abs_begin..=abs_end, &replacement);
            // Continue from the replacement point
            start = abs_begin + replacement.len();
        } else {
            break;
        }
    }

    Ok(result)
}

/// Content-safe variant for skill/agent prose.
///
/// - Sensitive-marked keys substitute to a descriptive placeholder.
/// - Unknown keys stay literal (no error).
pub fn substitute_user_config_in_content(
    content: &str,
    options: &PluginOptionValues,
    schema: &PluginOptionSchema,
) -> String {
    let mut result = content.to_string();
    let mut start = 0;

    while let Some(begin) = result[start..].find("${user_config.") {
        let abs_begin = start + begin;
        let after_prefix = abs_begin + "${user_config.".len();

        if let Some(end) = result[after_prefix..].find('}') {
            let abs_end = after_prefix + end;
            let key = &result[after_prefix..abs_end];

            let replacement = if schema.get(key).and_then(|c| c.sensitive).unwrap_or(false) {
                format!("[sensitive option '{key}' not available in skill content]")
            } else if let Some(val) = options.get(key) {
                match val {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                }
            } else {
                // Unknown key — leave literal
                start = abs_end + 1;
                continue;
            };

            result.replace_range(abs_begin..=abs_end, &replacement);
            start = abs_begin + replacement.len();
        } else {
            break;
        }
    }

    result
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn test_store() -> (PluginOptionsStore, tempfile::TempDir) {
        let tmp = tempfile::tempdir().expect("create temp dir");
        let base = tmp.path().join("plugins");
        let mgr = PluginDirectoryManager::with_base_dir(base);
        let store = PluginOptionsStore::new(mgr);
        (store, tmp)
    }

    #[test]
    fn test_load_options_empty() {
        let (store, _tmp) = test_store();
        let options = store.load_options("test-plugin").expect("load");
        assert!(options.is_empty());
    }

    #[test]
    fn test_save_and_load_options() {
        let (store, _tmp) = test_store();

        let mut values = BTreeMap::new();
        values.insert("api_key".into(), json!("secret123"));
        values.insert("timeout".into(), json!(30));

        let schema = BTreeMap::new(); // No sensitive fields
        store
            .save_options("test-plugin", &values, &schema)
            .expect("save");

        let loaded = store.load_options("test-plugin").expect("load");
        assert_eq!(loaded.get("api_key"), Some(&json!("secret123")));
        assert_eq!(loaded.get("timeout"), Some(&json!(30)));
    }

    #[test]
    fn test_save_options_filters_sensitive() {
        let (store, _tmp) = test_store();

        let mut values = BTreeMap::new();
        values.insert("public_key".into(), json!("visible"));
        values.insert("secret_key".into(), json!("hidden"));

        let mut schema = BTreeMap::new();
        schema.insert(
            "secret_key".into(),
            PluginOptionConfig {
                option_type: "string".into(),
                title: "Secret".into(),
                description: "A secret".into(),
                required: None,
                default: None,
                sensitive: Some(true),
            },
        );

        store
            .save_options("test-plugin", &values, &schema)
            .expect("save");

        let loaded = store.load_options("test-plugin").expect("load");
        assert_eq!(loaded.get("public_key"), Some(&json!("visible")));
        assert_eq!(loaded.get("secret_key"), None); // Filtered out
    }

    #[test]
    fn test_get_option() {
        let (store, _tmp) = test_store();

        let mut values = BTreeMap::new();
        values.insert("key".into(), json!("value"));
        let schema = BTreeMap::new();
        store
            .save_options("test-plugin", &values, &schema)
            .expect("save");

        let val = store.get_option("test-plugin", "key").expect("get");
        assert_eq!(val, Some(json!("value")));
    }

    #[test]
    fn test_get_option_missing() {
        let (store, _tmp) = test_store();
        let val = store.get_option("test-plugin", "nonexistent").expect("get");
        assert_eq!(val, None);
    }

    #[test]
    fn test_set_option() {
        let (store, _tmp) = test_store();

        store
            .set_option("test-plugin", "color", json!("blue"), None)
            .expect("set");

        let val = store.get_option("test-plugin", "color").expect("get");
        assert_eq!(val, Some(json!("blue")));
    }

    #[test]
    fn test_set_option_updates_existing() {
        let (store, _tmp) = test_store();

        store
            .set_option("test-plugin", "count", json!(1), None)
            .expect("set first");
        store
            .set_option("test-plugin", "count", json!(2), None)
            .expect("set second");

        let val = store.get_option("test-plugin", "count").expect("get");
        assert_eq!(val, Some(json!(2)));
    }

    #[test]
    fn test_delete_options() {
        let (store, _tmp) = test_store();

        let mut values = BTreeMap::new();
        values.insert("key".into(), json!("value"));
        let schema = BTreeMap::new();
        store
            .save_options("test-plugin", &values, &schema)
            .expect("save");

        store.delete_options("test-plugin");

        let loaded = store.load_options("test-plugin").expect("load");
        assert!(loaded.is_empty());
    }

    #[test]
    fn test_delete_options_nonexistent() {
        let (store, _tmp) = test_store();
        // Should not panic
        store.delete_options("nonexistent-plugin");
    }

    #[test]
    fn test_substitute_user_config_variables() {
        let mut config = BTreeMap::new();
        config.insert("name".into(), json!("world"));

        let result = substitute_user_config_variables("Hello ${user_config.name}!", &config)
            .expect("substitute");
        assert_eq!(result, "Hello world!");
    }

    #[test]
    fn test_substitute_user_config_variables_missing_key() {
        let config = BTreeMap::new();
        let result = substitute_user_config_variables("Hello ${user_config.name}!", &config);
        assert!(result.is_err());
    }

    #[test]
    fn test_substitute_user_config_variables_multiple() {
        let mut config = BTreeMap::new();
        config.insert("first".into(), json!("Alice"));
        config.insert("last".into(), json!("Smith"));

        let result =
            substitute_user_config_variables("${user_config.first} ${user_config.last}", &config)
                .expect("substitute");
        assert_eq!(result, "Alice Smith");
    }

    #[test]
    fn test_substitute_user_config_in_content_sensitive() {
        let mut options = BTreeMap::new();
        options.insert("api_key".into(), json!("secret123"));
        options.insert("name".into(), json!("Alice"));

        let mut schema = BTreeMap::new();
        schema.insert(
            "api_key".into(),
            PluginOptionConfig {
                option_type: "string".into(),
                title: "API Key".into(),
                description: "Secret API key".into(),
                required: None,
                default: None,
                sensitive: Some(true),
            },
        );

        let result = substitute_user_config_in_content(
            "Key: ${user_config.api_key}, Name: ${user_config.name}",
            &options,
            &schema,
        );
        assert_eq!(
            result,
            "Key: [sensitive option 'api_key' not available in skill content], Name: Alice"
        );
    }

    #[test]
    fn test_substitute_user_config_in_content_unknown_key() {
        let options = BTreeMap::new();
        let schema = BTreeMap::new();

        let result =
            substitute_user_config_in_content("Hello ${user_config.unknown}", &options, &schema);
        assert_eq!(result, "Hello ${user_config.unknown}");
    }

    #[test]
    fn test_substitute_plugin_variables() {
        let plugin_path = Path::new("/home/user/.claude/plugins/test");
        let result = substitute_plugin_variables("Path: ${CLAUDE_PLUGIN_ROOT}", plugin_path, None);
        assert!(result.contains("/home/user/.claude/plugins/test"));
        assert!(!result.contains("${CLAUDE_PLUGIN_ROOT}"));
    }
}
