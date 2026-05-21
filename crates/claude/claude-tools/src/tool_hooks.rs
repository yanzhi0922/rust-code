//! Pre-tool-use and post-tool-use hook integration.
//!
//! Provides the [`ToolHookManager`] that orchestrates hook execution around
//! tool calls.  Hooks can **approve**, **deny**, or **modify** (alter input)
//! a tool invocation.  The design mirrors the upstream `toolHooks.ts`:
//!
//! * **PreToolUse** hooks run before a tool executes and can short-circuit
//!   with an allow / deny / ask decision.
//! * **PostToolUse** hooks run after a tool completes and can inspect or
//!   transform the output.
//! * **PostToolUseFailure** hooks run when a tool errors.

use std::fmt;
use std::sync::Arc;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Hook decision types
// ---------------------------------------------------------------------------

/// The behaviour a hook can dictate for a tool invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookBehavior {
    /// Allow the tool to execute (optionally with modified input).
    Allow,
    /// Deny the tool execution.
    Deny,
    /// Ask the user / normal permission flow.
    Ask,
}

impl fmt::Display for HookBehavior {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Allow => write!(f, "allow"),
            Self::Deny => write!(f, "deny"),
            Self::Ask => write!(f, "ask"),
        }
    }
}

/// The result returned by a pre-tool-use hook.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreHookResult {
    /// The behaviour the hook dictates.
    pub behavior: HookBehavior,
    /// Optional reason message (shown to user on deny / ask).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// If the hook modified the tool input, the new input is here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_input: Option<Value>,
    /// Whether the hook requests stopping further execution.
    #[serde(default)]
    pub prevent_continuation: bool,
    /// Optional stop reason when preventing continuation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
}

/// The result returned by a post-tool-use hook.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostHookResult {
    /// Optional blocking error message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocking_error: Option<String>,
    /// Whether to prevent further continuation.
    #[serde(default)]
    pub prevent_continuation: bool,
    /// Optional stop reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
    /// Additional context strings to attach.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub additional_contexts: Vec<String>,
    /// Updated MCP tool output (if the hook transformed it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_output: Option<String>,
}

/// The result returned by a post-tool-use-failure hook.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostFailureHookResult {
    /// Optional blocking error message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocking_error: Option<String>,
    /// Additional context strings.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub additional_contexts: Vec<String>,
}

// ---------------------------------------------------------------------------
// Hook trait
// ---------------------------------------------------------------------------

/// A single hook that can be registered with the [`ToolHookManager`].
pub trait ToolHook: Send + Sync + 'static {
    /// Hook name for logging / debugging.
    fn name(&self) -> &str;

    /// Run as a PreToolUse hook.  Return `None` to skip / pass through.
    fn pre_tool_use(&self, tool_name: &str, tool_input: &Value) -> Result<Option<PreHookResult>>;

    /// Run as a PostToolUse hook.  Return `None` to skip.
    fn post_tool_use(
        &self,
        tool_name: &str,
        tool_input: &Value,
        output: &str,
        is_error: bool,
    ) -> Result<Option<PostHookResult>>;

    /// Run as a PostToolUseFailure hook.  Return `None` to skip.
    ///
    /// The default implementation logs the failure at warn level.
    fn post_tool_use_failure(
        &self,
        tool_name: &str,
        tool_input: &Value,
        error: &str,
    ) -> Result<Option<PostFailureHookResult>> {
        eprintln!(
            "[hook:{}] tool '{}' failed: {}",
            self.name(),
            tool_name,
            error
        );
        let _ = tool_input;
        Ok(None)
    }
}

// ---------------------------------------------------------------------------
// ToolHookManager
// ---------------------------------------------------------------------------

/// Manages a collection of hooks and runs them around tool executions.
pub struct ToolHookManager {
    hooks: Vec<Arc<dyn ToolHook>>,
}

impl ToolHookManager {
    /// Create an empty hook manager.
    #[must_use]
    pub fn new() -> Self {
        Self { hooks: Vec::new() }
    }

    /// Register a hook.
    pub fn register(&mut self, hook: Arc<dyn ToolHook>) {
        self.hooks.push(hook);
    }

    /// Number of registered hooks.
    #[must_use]
    pub fn hook_count(&self) -> usize {
        self.hooks.len()
    }

    /// Return the names of all registered hooks.
    #[must_use]
    pub fn hook_names(&self) -> Vec<&str> {
        self.hooks.iter().map(|h| h.name()).collect()
    }

    // -- Pre-tool-use -------------------------------------------------------

    /// Run all PreToolUse hooks and aggregate their decisions.
    ///
    /// The first hook that returns `Deny` short-circuits with a deny.
    /// If any hook returns `Allow`, the tool is approved (but later hooks
    /// may still override).  If no hook returns a decision, `None` is
    /// returned, meaning the normal permission flow should proceed.
    pub fn run_pre_hooks(
        &self,
        tool_name: &str,
        tool_input: &Value,
    ) -> Result<AggregatedPreHookResult> {
        let mut aggregated = AggregatedPreHookResult::default();
        let mut current_input = tool_input.clone();

        for hook in &self.hooks {
            let result = hook.pre_tool_use(tool_name, &current_input)?;

            let Some(result) = result else {
                continue;
            };

            // Track the most recent updated input
            if let Some(ref updated) = result.updated_input {
                current_input = updated.clone();
                aggregated.final_input = Some(current_input.clone());
            }

            if result.prevent_continuation {
                aggregated.prevent_continuation = true;
                aggregated.stop_reason = result.stop_reason.clone();
                // Don't return early – let other hooks see this too
            }

            match result.behavior {
                HookBehavior::Deny => {
                    aggregated.decision = Some(HookBehavior::Deny);
                    aggregated.deny_message = result.message.clone();
                    // Deny short-circuits
                    return Ok(aggregated);
                }
                HookBehavior::Allow => {
                    if aggregated.decision != Some(HookBehavior::Deny) {
                        aggregated.decision = Some(HookBehavior::Allow);
                    }
                }
                HookBehavior::Ask => {
                    if aggregated.decision.is_none() {
                        aggregated.decision = Some(HookBehavior::Ask);
                        aggregated.ask_message = result.message.clone();
                    }
                }
            }
        }

        Ok(aggregated)
    }

    // -- Post-tool-use ------------------------------------------------------

    /// Run all PostToolUse hooks.
    pub fn run_post_hooks(
        &self,
        tool_name: &str,
        tool_input: &Value,
        output: &str,
        is_error: bool,
    ) -> Result<AggregatedPostHookResult> {
        let mut aggregated = AggregatedPostHookResult::default();
        let mut current_output = output.to_owned();

        for hook in &self.hooks {
            let result = hook.post_tool_use(tool_name, tool_input, &current_output, is_error)?;

            let Some(result) = result else {
                continue;
            };

            if let Some(err) = &result.blocking_error {
                aggregated.blocking_errors.push(err.clone());
            }
            if result.prevent_continuation {
                aggregated.prevent_continuation = true;
                aggregated.stop_reason = result.stop_reason.clone();
            }
            aggregated
                .additional_contexts
                .extend(result.additional_contexts);
            if let Some(updated) = &result.updated_output {
                current_output = updated.clone();
                aggregated.updated_output = Some(current_output.clone());
            }
        }

        Ok(aggregated)
    }

    // -- Post-tool-use-failure ----------------------------------------------

    /// Run all PostToolUseFailure hooks.
    pub fn run_post_failure_hooks(
        &self,
        tool_name: &str,
        tool_input: &Value,
        error: &str,
    ) -> Result<AggregatedPostFailureHookResult> {
        let mut aggregated = AggregatedPostFailureHookResult::default();

        for hook in &self.hooks {
            let result = hook.post_tool_use_failure(tool_name, tool_input, error)?;

            let Some(result) = result else {
                continue;
            };

            if let Some(err) = &result.blocking_error {
                aggregated.blocking_errors.push(err.clone());
            }
            aggregated
                .additional_contexts
                .extend(result.additional_contexts);
        }

        Ok(aggregated)
    }
}

impl Default for ToolHookManager {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Aggregated results
// ---------------------------------------------------------------------------

/// Aggregated result from all PreToolUse hooks.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AggregatedPreHookResult {
    /// The winning hook decision (if any).
    pub decision: Option<HookBehavior>,
    /// Deny message (if decision is Deny).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deny_message: Option<String>,
    /// Ask message (if decision is Ask).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ask_message: Option<String>,
    /// Modified input from hooks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_input: Option<Value>,
    /// Whether hooks requested stopping.
    #[serde(default)]
    pub prevent_continuation: bool,
    /// Stop reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
}

/// Aggregated result from all PostToolUse hooks.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AggregatedPostHookResult {
    /// Blocking errors from hooks.
    pub blocking_errors: Vec<String>,
    /// Whether hooks requested stopping.
    pub prevent_continuation: bool,
    /// Stop reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
    /// Additional context strings.
    pub additional_contexts: Vec<String>,
    /// Updated output from hooks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_output: Option<String>,
}

/// Aggregated result from all PostToolUseFailure hooks.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AggregatedPostFailureHookResult {
    /// Blocking errors from hooks.
    pub blocking_errors: Vec<String>,
    /// Additional context strings.
    pub additional_contexts: Vec<String>,
}

// ---------------------------------------------------------------------------
// Built-in test hooks
// ---------------------------------------------------------------------------

/// A hook that always allows.
pub struct AlwaysAllowHook;

impl ToolHook for AlwaysAllowHook {
    fn name(&self) -> &str {
        "always_allow"
    }

    fn pre_tool_use(&self, _tool_name: &str, _tool_input: &Value) -> Result<Option<PreHookResult>> {
        Ok(Some(PreHookResult {
            behavior: HookBehavior::Allow,
            message: None,
            updated_input: None,
            prevent_continuation: false,
            stop_reason: None,
        }))
    }

    fn post_tool_use(
        &self,
        _tool_name: &str,
        _tool_input: &Value,
        _output: &str,
        _is_error: bool,
    ) -> Result<Option<PostHookResult>> {
        Ok(None)
    }
}

/// A hook that always denies.
pub struct AlwaysDenyHook {
    pub reason: String,
}

impl ToolHook for AlwaysDenyHook {
    fn name(&self) -> &str {
        "always_deny"
    }

    fn pre_tool_use(&self, _tool_name: &str, _tool_input: &Value) -> Result<Option<PreHookResult>> {
        Ok(Some(PreHookResult {
            behavior: HookBehavior::Deny,
            message: Some(self.reason.clone()),
            updated_input: None,
            prevent_continuation: false,
            stop_reason: None,
        }))
    }

    fn post_tool_use(
        &self,
        _tool_name: &str,
        _tool_input: &Value,
        _output: &str,
        _is_error: bool,
    ) -> Result<Option<PostHookResult>> {
        Ok(None)
    }
}

/// A hook that modifies the tool input.
pub struct InputModifierHook {
    pub key: String,
    pub value: Value,
}

impl ToolHook for InputModifierHook {
    fn name(&self) -> &str {
        "input_modifier"
    }

    fn pre_tool_use(&self, _tool_name: &str, tool_input: &Value) -> Result<Option<PreHookResult>> {
        let mut input = tool_input.clone();
        if let Some(obj) = input.as_object_mut() {
            obj.insert(self.key.clone(), self.value.clone());
        }
        Ok(Some(PreHookResult {
            behavior: HookBehavior::Allow,
            message: None,
            updated_input: Some(input),
            prevent_continuation: false,
            stop_reason: None,
        }))
    }

    fn post_tool_use(
        &self,
        _tool_name: &str,
        _tool_input: &Value,
        _output: &str,
        _is_error: bool,
    ) -> Result<Option<PostHookResult>> {
        Ok(None)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- HookBehavior -------------------------------------------------------

    #[test]
    fn hook_behavior_display() {
        assert_eq!(format!("{}", HookBehavior::Allow), "allow");
        assert_eq!(format!("{}", HookBehavior::Deny), "deny");
        assert_eq!(format!("{}", HookBehavior::Ask), "ask");
    }

    #[test]
    fn hook_behavior_serialization() {
        let b = HookBehavior::Allow;
        let json = serde_json::to_string(&b).expect("ser");
        assert_eq!(json, "\"allow\"");
        let back: HookBehavior = serde_json::from_str(&json).expect("de");
        assert_eq!(back, HookBehavior::Allow);
    }

    // -- ToolHookManager basic ----------------------------------------------

    #[test]
    fn empty_manager_no_decision() {
        let mgr = ToolHookManager::new();
        let result = mgr.run_pre_hooks("read_file", &Value::Null).expect("ok");
        assert!(result.decision.is_none());
    }

    #[test]
    fn register_and_count() {
        let mut mgr = ToolHookManager::new();
        mgr.register(Arc::new(AlwaysAllowHook));
        mgr.register(Arc::new(AlwaysAllowHook));
        assert_eq!(mgr.hook_count(), 2);
    }

    #[test]
    fn hook_names() {
        let mut mgr = ToolHookManager::new();
        mgr.register(Arc::new(AlwaysAllowHook));
        mgr.register(Arc::new(AlwaysDenyHook {
            reason: "nope".into(),
        }));
        let names = mgr.hook_names();
        assert_eq!(names, vec!["always_allow", "always_deny"]);
    }

    // -- PreHook: Allow -----------------------------------------------------

    #[test]
    fn always_allow_approves() {
        let mut mgr = ToolHookManager::new();
        mgr.register(Arc::new(AlwaysAllowHook));
        let result = mgr.run_pre_hooks("read_file", &Value::Null).expect("ok");
        assert_eq!(result.decision, Some(HookBehavior::Allow));
    }

    // -- PreHook: Deny ------------------------------------------------------

    #[test]
    fn always_deny_rejects() {
        let mut mgr = ToolHookManager::new();
        mgr.register(Arc::new(AlwaysDenyHook {
            reason: "forbidden".into(),
        }));
        let result = mgr.run_pre_hooks("bash", &Value::Null).expect("ok");
        assert_eq!(result.decision, Some(HookBehavior::Deny));
        assert_eq!(result.deny_message.as_deref(), Some("forbidden"));
    }

    #[test]
    fn deny_short_circuits() {
        let mut mgr = ToolHookManager::new();
        // Deny first, then allow – deny should win
        mgr.register(Arc::new(AlwaysDenyHook {
            reason: "no".into(),
        }));
        mgr.register(Arc::new(AlwaysAllowHook));
        let result = mgr.run_pre_hooks("bash", &Value::Null).expect("ok");
        assert_eq!(result.decision, Some(HookBehavior::Deny));
    }

    // -- PreHook: Input modification ----------------------------------------

    #[test]
    fn input_modifier_updates_input() {
        let mut mgr = ToolHookManager::new();
        mgr.register(Arc::new(InputModifierHook {
            key: "extra".into(),
            value: Value::Bool(true),
        }));
        let result = mgr
            .run_pre_hooks("read_file", &Value::Object(serde_json::Map::new()))
            .expect("ok");
        assert_eq!(result.decision, Some(HookBehavior::Allow));
        let input = result.final_input.expect("input");
        assert_eq!(input.get("extra"), Some(&Value::Bool(true)));
    }

    // -- PostHook -----------------------------------------------------------

    struct PostModifierHook;

    impl ToolHook for PostModifierHook {
        fn name(&self) -> &str {
            "post_modifier"
        }

        fn pre_tool_use(
            &self,
            _tool_name: &str,
            _tool_input: &Value,
        ) -> Result<Option<PreHookResult>> {
            Ok(None)
        }

        fn post_tool_use(
            &self,
            _tool_name: &str,
            _tool_input: &Value,
            output: &str,
            _is_error: bool,
        ) -> Result<Option<PostHookResult>> {
            Ok(Some(PostHookResult {
                blocking_error: None,
                prevent_continuation: false,
                stop_reason: None,
                additional_contexts: vec![format!("saw: {output}")],
                updated_output: Some(format!("modified: {output}")),
            }))
        }
    }

    #[test]
    fn post_hook_modifies_output() {
        let mut mgr = ToolHookManager::new();
        mgr.register(Arc::new(PostModifierHook));
        let result = mgr
            .run_post_hooks("read_file", &Value::Null, "hello", false)
            .expect("ok");
        assert_eq!(result.updated_output.as_deref(), Some("modified: hello"));
        assert_eq!(result.additional_contexts.len(), 1);
    }

    // -- PostFailureHook ----------------------------------------------------

    struct FailureLoggerHook {
        logged: Arc<std::sync::Mutex<Option<String>>>,
    }

    impl ToolHook for FailureLoggerHook {
        fn name(&self) -> &str {
            "failure_logger"
        }

        fn pre_tool_use(
            &self,
            _tool_name: &str,
            _tool_input: &Value,
        ) -> Result<Option<PreHookResult>> {
            Ok(None)
        }

        fn post_tool_use(
            &self,
            _tool_name: &str,
            _tool_input: &Value,
            _output: &str,
            _is_error: bool,
        ) -> Result<Option<PostHookResult>> {
            Ok(None)
        }

        fn post_tool_use_failure(
            &self,
            tool_name: &str,
            _tool_input: &Value,
            error: &str,
        ) -> Result<Option<PostFailureHookResult>> {
            *self.logged.lock().expect("lock") = Some(format!("{tool_name}: {error}"));
            Ok(Some(PostFailureHookResult {
                blocking_error: None,
                additional_contexts: vec![format!("error logged for {tool_name}")],
            }))
        }
    }

    #[test]
    fn post_failure_hook_captures_error() {
        let logged = Arc::new(std::sync::Mutex::new(None));
        let mut mgr = ToolHookManager::new();
        mgr.register(Arc::new(FailureLoggerHook {
            logged: Arc::clone(&logged),
        }));
        let result = mgr
            .run_post_failure_hooks("bash", &Value::Null, "exit code 1")
            .expect("ok");
        assert_eq!(result.additional_contexts.len(), 1);
        let inner = logged.lock().expect("lock");
        assert_eq!(inner.as_deref(), Some("bash: exit code 1"));
    }

    // -- Default impl -------------------------------------------------------

    #[test]
    fn default_manager_is_empty() {
        let mgr = ToolHookManager::default();
        assert_eq!(mgr.hook_count(), 0);
    }

    // -- Serialization ------------------------------------------------------

    #[test]
    fn aggregated_pre_hook_result_serde() {
        let result = AggregatedPreHookResult {
            decision: Some(HookBehavior::Deny),
            deny_message: Some("nope".into()),
            ask_message: None,
            final_input: None,
            prevent_continuation: false,
            stop_reason: None,
        };
        let json = serde_json::to_string(&result).expect("ser");
        let back: AggregatedPreHookResult = serde_json::from_str(&json).expect("de");
        assert_eq!(back.decision, Some(HookBehavior::Deny));
    }

    #[test]
    fn aggregated_post_hook_result_serde() {
        let result = AggregatedPostHookResult {
            blocking_errors: vec!["err1".into()],
            prevent_continuation: true,
            stop_reason: Some("done".into()),
            additional_contexts: vec![],
            updated_output: Some("new".into()),
        };
        let json = serde_json::to_string(&result).expect("ser");
        let back: AggregatedPostHookResult = serde_json::from_str(&json).expect("de");
        assert!(back.prevent_continuation);
    }
}
