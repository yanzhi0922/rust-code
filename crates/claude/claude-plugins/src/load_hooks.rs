//! Hook loading from plugin manifests.
//!
//! Extracts hook configurations from plugin directories. Hooks are defined
//! in `hooks.json` files within the plugin root.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Hook event types that plugins can subscribe to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum HookEvent {
    PreToolUse,
    PostToolUse,
    PostToolUseFailure,
    PermissionDenied,
    Notification,
    UserPromptSubmit,
    SessionStart,
    SessionEnd,
    Stop,
    StopFailure,
    SubagentStart,
    SubagentStop,
    PreCompact,
    PostCompact,
    PermissionRequest,
    Setup,
    TaskCreated,
    TaskCompleted,
}

/// A hook matcher configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginHookMatcher {
    /// Optional pattern to match against.
    #[serde(default)]
    pub matcher: Option<String>,
    /// Hook commands to execute.
    pub hooks: Vec<HookDefinition>,
}

/// A single hook definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookDefinition {
    /// Command to execute.
    pub command: String,
    /// Optional description.
    #[serde(default)]
    pub description: Option<String>,
    /// Whether the hook runs in the background.
    #[serde(default)]
    pub background: bool,
}

/// Complete hook configuration for a plugin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginHookConfig {
    /// Plugin name.
    pub plugin_name: String,
    /// Source file path.
    pub source_path: PathBuf,
    /// Hook matchers by event type.
    pub hooks: HashMap<HookEvent, Vec<PluginHookMatcher>>,
}

/// Result of loading hooks from a plugin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoadHooksResult {
    /// Hook configuration (if found).
    pub config: Option<PluginHookConfig>,
    /// Errors encountered.
    pub errors: Vec<String>,
}

// ---------------------------------------------------------------------------
// Core functions
// ---------------------------------------------------------------------------

/// Load plugin hooks from a hooks configuration file.
///
/// Reads the hooks JSON file and parses it into a structured configuration.
pub fn load_plugin_hooks(plugin_name: &str, hooks_path: &Path) -> LoadHooksResult {
    let mut errors = Vec::new();

    if !hooks_path.exists() {
        return LoadHooksResult {
            config: None,
            errors,
        };
    }

    let content = match std::fs::read_to_string(hooks_path) {
        Ok(c) => c,
        Err(e) => {
            errors.push(format!(
                "failed to read hooks config {}: {e}",
                hooks_path.display()
            ));
            return LoadHooksResult {
                config: None,
                errors,
            };
        }
    };

    let raw: Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(e) => {
            errors.push(format!(
                "failed to parse hooks config {}: {e}",
                hooks_path.display()
            ));
            return LoadHooksResult {
                config: None,
                errors,
            };
        }
    };

    let hooks = match parse_hooks_config(&raw) {
        Ok(h) => h,
        Err(e) => {
            errors.push(e);
            return LoadHooksResult {
                config: None,
                errors,
            };
        }
    };

    LoadHooksResult {
        config: Some(PluginHookConfig {
            plugin_name: plugin_name.to_owned(),
            source_path: hooks_path.to_path_buf(),
            hooks,
        }),
        errors,
    }
}

/// Parse a raw JSON hooks configuration.
fn parse_hooks_config(raw: &Value) -> Result<HashMap<HookEvent, Vec<PluginHookMatcher>>, String> {
    let mut hooks = HashMap::new();

    let obj = raw
        .as_object()
        .ok_or_else(|| "hooks config must be a JSON object".to_owned())?;

    for (event_name, event_value) in obj {
        let event = match parse_hook_event(event_name) {
            Some(e) => e,
            None => continue, // Skip unknown events
        };

        let matchers = match parse_matchers(event_value) {
            Ok(m) => m,
            Err(e) => {
                return Err(format!("invalid matchers for event '{event_name}': {e}"));
            }
        };

        hooks.insert(event, matchers);
    }

    Ok(hooks)
}

/// Parse a hook event name string.
fn parse_hook_event(name: &str) -> Option<HookEvent> {
    match name {
        "PreToolUse" => Some(HookEvent::PreToolUse),
        "PostToolUse" => Some(HookEvent::PostToolUse),
        "PostToolUseFailure" => Some(HookEvent::PostToolUseFailure),
        "PermissionDenied" => Some(HookEvent::PermissionDenied),
        "Notification" => Some(HookEvent::Notification),
        "UserPromptSubmit" => Some(HookEvent::UserPromptSubmit),
        "SessionStart" => Some(HookEvent::SessionStart),
        "SessionEnd" => Some(HookEvent::SessionEnd),
        "Stop" => Some(HookEvent::Stop),
        "StopFailure" => Some(HookEvent::StopFailure),
        "SubagentStart" => Some(HookEvent::SubagentStart),
        "SubagentStop" => Some(HookEvent::SubagentStop),
        "PreCompact" => Some(HookEvent::PreCompact),
        "PostCompact" => Some(HookEvent::PostCompact),
        "PermissionRequest" => Some(HookEvent::PermissionRequest),
        "Setup" => Some(HookEvent::Setup),
        "TaskCreated" => Some(HookEvent::TaskCreated),
        "TaskCompleted" => Some(HookEvent::TaskCompleted),
        _ => None,
    }
}

/// Parse matchers from a JSON value.
fn parse_matchers(value: &Value) -> Result<Vec<PluginHookMatcher>, String> {
    let arr = value
        .as_array()
        .ok_or_else(|| "expected an array of matchers".to_owned())?;

    let mut matchers = Vec::new();
    for item in arr {
        let obj = item
            .as_object()
            .ok_or_else(|| "each matcher must be an object".to_owned())?;

        let matcher = obj
            .get("matcher")
            .and_then(|v| v.as_str())
            .map(|s| s.to_owned());

        let hooks_arr = obj
            .get("hooks")
            .and_then(|v| v.as_array())
            .ok_or_else(|| "each matcher must have a 'hooks' array".to_owned())?;

        let mut hook_defs = Vec::new();
        for hook_val in hooks_arr {
            let hook_obj = hook_val
                .as_object()
                .ok_or_else(|| "each hook must be an object".to_owned())?;

            let command = hook_obj
                .get("command")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "each hook must have a 'command'".to_owned())?
                .to_owned();

            let description = hook_obj
                .get("description")
                .and_then(|v| v.as_str())
                .map(|s| s.to_owned());

            let background = hook_obj
                .get("background")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            hook_defs.push(HookDefinition {
                command,
                description,
                background,
            });
        }

        matchers.push(PluginHookMatcher {
            matcher,
            hooks: hook_defs,
        });
    }

    Ok(matchers)
}

/// Count total hooks across all events.
pub fn count_hooks(config: &PluginHookConfig) -> usize {
    config
        .hooks
        .values()
        .map(|matchers| matchers.iter().map(|m| m.hooks.len()).sum::<usize>())
        .sum()
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
    fn load_plugin_hooks_basic() {
        let temp = ok(tempdir());
        let hooks_path = temp.path().join("hooks.json");
        fs::write(
            &hooks_path,
            r#"{
                "PreToolUse": [
                    {
                        "matcher": "Bash",
                        "hooks": [
                            {"command": "echo 'before bash'", "description": "Log bash usage"}
                        ]
                    }
                ],
                "PostToolUse": [
                    {
                        "hooks": [
                            {"command": "echo 'after tool'", "background": true}
                        ]
                    }
                ]
            }"#,
        )
        .expect("write hooks");

        let result = load_plugin_hooks("test-plugin", &hooks_path);
        assert!(result.config.is_some());
        assert!(result.errors.is_empty());

        let config = result.config.expect("config");
        assert_eq!(config.plugin_name, "test-plugin");
        assert!(config.hooks.contains_key(&HookEvent::PreToolUse));
        assert!(config.hooks.contains_key(&HookEvent::PostToolUse));

        let pre = &config.hooks[&HookEvent::PreToolUse];
        assert_eq!(pre.len(), 1);
        assert_eq!(pre[0].matcher, Some("Bash".to_owned()));
        assert_eq!(pre[0].hooks.len(), 1);
        assert_eq!(pre[0].hooks[0].command, "echo 'before bash'");
    }

    #[test]
    fn load_plugin_hooks_nonexistent() {
        let result = load_plugin_hooks("test-plugin", Path::new("/nonexistent/hooks.json"));
        assert!(result.config.is_none());
        assert!(result.errors.is_empty());
    }

    #[test]
    fn load_plugin_hooks_invalid_json() {
        let temp = ok(tempdir());
        let hooks_path = temp.path().join("hooks.json");
        fs::write(&hooks_path, "not json").expect("write");

        let result = load_plugin_hooks("test-plugin", &hooks_path);
        assert!(result.config.is_none());
        assert_eq!(result.errors.len(), 1);
    }

    #[test]
    fn count_hooks_works() {
        let config = PluginHookConfig {
            plugin_name: "test".to_owned(),
            source_path: PathBuf::from("hooks.json"),
            hooks: {
                let mut map = HashMap::new();
                map.insert(
                    HookEvent::PreToolUse,
                    vec![PluginHookMatcher {
                        matcher: None,
                        hooks: vec![
                            HookDefinition {
                                command: "cmd1".to_owned(),
                                description: None,
                                background: false,
                            },
                            HookDefinition {
                                command: "cmd2".to_owned(),
                                description: None,
                                background: false,
                            },
                        ],
                    }],
                );
                map
            },
        };
        assert_eq!(count_hooks(&config), 2);
    }

    #[test]
    fn parse_hook_event_known() {
        assert_eq!(parse_hook_event("PreToolUse"), Some(HookEvent::PreToolUse));
        assert_eq!(
            parse_hook_event("PostToolUse"),
            Some(HookEvent::PostToolUse)
        );
        assert_eq!(
            parse_hook_event("SessionStart"),
            Some(HookEvent::SessionStart)
        );
    }

    #[test]
    fn parse_hook_event_unknown() {
        assert_eq!(parse_hook_event("UnknownEvent"), None);
    }
}
