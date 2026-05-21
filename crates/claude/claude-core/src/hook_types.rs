//! Hook type system — comprehensive types for the hook lifecycle.
//!
//! Covers all hook types (command, prompt, agent, http, callback, function),
//! input/output structures, response parsing, and event-specific output variants.
//! Modeled after the upstream `types/hooks.ts` and `schemas/hooks.ts`.

use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::hooks::HookEventKind;

// ── Hook type discriminator ──────────────────────────────────────────────

/// Discriminator for the kind of hook backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HookType {
    /// Shell command hook.
    Command,
    /// LLM prompt evaluation hook.
    Prompt,
    /// Agentic verifier hook.
    Agent,
    /// HTTP POST hook.
    Http,
    /// In-process callback hook (not serializable).
    Callback,
    /// Named function hook (not serializable).
    Function,
}

impl HookType {
    /// Return the string tag used in serialized hook definitions.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Command => "command",
            Self::Prompt => "prompt",
            Self::Agent => "agent",
            Self::Http => "http",
            Self::Callback => "callback",
            Self::Function => "function",
        }
    }
}

// ── Shell interpreter ────────────────────────────────────────────────────

/// Shell interpreter for command hooks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HookShell {
    /// POSIX-compatible shell (bash / zsh / sh).
    Bash,
    /// Windows PowerShell.
    PowerShell,
}

impl HookShell {
    /// Return the lowercase shell name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bash => "bash",
            Self::PowerShell => "powershell",
        }
    }

    /// Return the platform default shell.
    #[must_use]
    pub fn platform_default() -> Self {
        if cfg!(windows) {
            Self::PowerShell
        } else {
            Self::Bash
        }
    }
}

// ── Individual hook definitions ──────────────────────────────────────────

/// A shell command hook.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HookCommand {
    /// Shell command to execute.
    pub command: String,
    /// Optional shell interpreter override.
    #[serde(default)]
    pub shell: Option<HookShell>,
    /// Timeout in seconds.
    #[serde(default)]
    pub timeout: Option<u64>,
    /// Permission-rule syntax condition (e.g. `"Bash(git *)"`).
    #[serde(default, rename = "if")]
    pub if_condition: Option<String>,
    /// Custom status message while the hook runs.
    #[serde(default)]
    pub status_message: Option<String>,
    /// Run only once per session then remove.
    #[serde(default)]
    pub once: bool,
    /// Run asynchronously (non-blocking).
    #[serde(default)]
    pub r#async: bool,
    /// Async with re-wake on exit code 2.
    #[serde(default)]
    pub async_rewake: bool,
}

/// An LLM prompt evaluation hook.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HookPrompt {
    /// Prompt text (may contain `$ARGUMENTS` placeholder).
    pub prompt: String,
    /// Model to use (defaults to fast model).
    #[serde(default)]
    pub model: Option<String>,
    /// Timeout in seconds.
    #[serde(default)]
    pub timeout: Option<u64>,
    /// Permission-rule syntax condition.
    #[serde(default, rename = "if")]
    pub if_condition: Option<String>,
    /// Custom status message.
    #[serde(default)]
    pub status_message: Option<String>,
    /// Run only once.
    #[serde(default)]
    pub once: bool,
}

/// An agentic verifier hook.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HookAgent {
    /// Prompt describing what to verify.
    pub prompt: String,
    /// Model to use.
    #[serde(default)]
    pub model: Option<String>,
    /// Timeout in seconds (default 60).
    #[serde(default)]
    pub timeout: Option<u64>,
    /// Permission-rule syntax condition.
    #[serde(default, rename = "if")]
    pub if_condition: Option<String>,
    /// Custom status message.
    #[serde(default)]
    pub status_message: Option<String>,
    /// Run only once.
    #[serde(default)]
    pub once: bool,
}

/// An HTTP POST hook.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HookHttp {
    /// URL to POST hook input JSON to.
    pub url: String,
    /// HTTP method (defaults to POST).
    #[serde(default)]
    pub method: Option<String>,
    /// Additional headers (values may reference `$VAR` / `${VAR}`).
    #[serde(default)]
    pub headers: HashMap<String, String>,
    /// Environment variable names allowed for interpolation in header values.
    #[serde(default)]
    pub allowed_env_vars: Vec<String>,
    /// Timeout in seconds.
    #[serde(default)]
    pub timeout: Option<u64>,
    /// Permission-rule syntax condition.
    #[serde(default, rename = "if")]
    pub if_condition: Option<String>,
    /// Custom status message.
    #[serde(default)]
    pub status_message: Option<String>,
    /// Run only once.
    #[serde(default)]
    pub once: bool,
}

/// A callback hook (in-process, not serializable).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HookCallback {
    /// Callback identifier for lookup.
    pub callback_id: String,
    /// Timeout in seconds.
    #[serde(default)]
    pub timeout: Option<u64>,
}

/// A named function hook (in-process, not serializable).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HookFunction {
    /// Function identifier for lookup.
    pub function_id: String,
    /// Timeout in seconds.
    #[serde(default)]
    pub timeout: Option<u64>,
}

// ── Tagged hook union ────────────────────────────────────────────────────

/// Tagged union of all hook definitions.
///
/// Mirrors the upstream discriminated-union on the `type` field.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum HookDefinition {
    /// Shell command hook.
    Command(HookCommand),
    /// LLM prompt hook.
    Prompt(HookPrompt),
    /// Agent verifier hook.
    Agent(HookAgent),
    /// HTTP POST hook.
    Http(HookHttp),
    /// Callback hook.
    Callback(HookCallback),
    /// Function hook.
    Function(HookFunction),
}

impl HookDefinition {
    /// Return the hook type discriminator.
    #[must_use]
    pub fn hook_type(&self) -> HookType {
        match self {
            Self::Command(_) => HookType::Command,
            Self::Prompt(_) => HookType::Prompt,
            Self::Agent(_) => HookType::Agent,
            Self::Http(_) => HookType::Http,
            Self::Callback(_) => HookType::Callback,
            Self::Function(_) => HookType::Function,
        }
    }

    /// Return the optional `if` condition string.
    #[must_use]
    pub fn if_condition(&self) -> Option<&str> {
        match self {
            Self::Command(h) => h.if_condition.as_deref(),
            Self::Prompt(h) => h.if_condition.as_deref(),
            Self::Agent(h) => h.if_condition.as_deref(),
            Self::Http(h) => h.if_condition.as_deref(),
            Self::Callback(_) | Self::Function(_) => None,
        }
    }

    /// Return the timeout in seconds if set.
    #[must_use]
    pub fn timeout(&self) -> Option<u64> {
        match self {
            Self::Command(h) => h.timeout,
            Self::Prompt(h) => h.timeout,
            Self::Agent(h) => h.timeout,
            Self::Http(h) => h.timeout,
            Self::Callback(h) => h.timeout,
            Self::Function(h) => h.timeout,
        }
    }

    /// Return the resolved timeout as a [`Duration`], falling back to `default_secs`.
    #[must_use]
    pub fn timeout_duration(&self, default_secs: u64) -> Duration {
        Duration::from_secs(self.timeout().unwrap_or(default_secs))
    }

    /// Return the status message if set.
    #[must_use]
    pub fn status_message(&self) -> Option<&str> {
        match self {
            Self::Command(h) => h.status_message.as_deref(),
            Self::Prompt(h) => h.status_message.as_deref(),
            Self::Agent(h) => h.status_message.as_deref(),
            Self::Http(h) => h.status_message.as_deref(),
            Self::Callback(_) | Self::Function(_) => None,
        }
    }

    /// Whether the hook should fire only once.
    #[must_use]
    pub fn is_once(&self) -> bool {
        match self {
            Self::Command(h) => h.once,
            Self::Prompt(h) => h.once,
            Self::Agent(h) => h.once,
            Self::Http(h) => h.once,
            Self::Callback(_) | Self::Function(_) => false,
        }
    }

    /// A unique key for deduplication (command text / prompt text / URL).
    #[must_use]
    pub fn dedup_key(&self) -> String {
        match self {
            Self::Command(h) => format!("cmd:{}", h.command),
            Self::Prompt(h) => format!("prompt:{}", h.prompt),
            Self::Agent(h) => format!("agent:{}", h.prompt),
            Self::Http(h) => format!("http:{}:{}", h.url, h.method.as_deref().unwrap_or("POST")),
            Self::Callback(h) => format!("cb:{}", h.callback_id),
            Self::Function(h) => format!("fn:{}", h.function_id),
        }
    }
}

// ── Matcher ──────────────────────────────────────────────────────────────

/// A hook matcher groups a pattern with its associated hooks.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HookMatcherEntry {
    /// Optional tool-name pattern (e.g. `"Write"` or `"Bash|Edit"`).
    #[serde(default)]
    pub matcher: Option<String>,
    /// Hooks to run when the matcher fires.
    #[serde(default)]
    pub hooks: Vec<HookDefinition>,
}

// ── Hook input / output ──────────────────────────────────────────────────

/// Input passed to every hook invocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookInput {
    /// The event that triggered this hook.
    pub event: HookEventKind,
    /// Tool name (for PreToolUse / PostToolUse).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    /// Tool input arguments (JSON).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_input: Option<Value>,
    /// Session ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Working directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// User prompt text (for UserPromptSubmit).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_prompt: Option<String>,
    /// Tool use ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,
    /// Tool result payload (for PostToolUse / PostToolUseFailure).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_result: Option<Value>,
}

/// Raw output from a hook process.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HookOutput {
    /// Process exit code.
    pub exit_code: Option<i32>,
    /// Captured stdout.
    pub stdout: String,
    /// Captured stderr.
    pub stderr: String,
    /// Parsed JSON from stdout (if valid JSON).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parsed_json: Option<Value>,
}

impl HookOutput {
    /// Whether the hook exited with code 0.
    #[must_use]
    pub fn is_success(&self) -> bool {
        self.exit_code == Some(0)
    }

    /// Try to parse stdout as JSON, returning the parsed value.
    pub fn parse_stdout(&mut self) {
        if let Ok(val) = serde_json::from_str::<Value>(self.stdout.trim()) {
            self.parsed_json = Some(val);
        }
    }
}

// ── Hook response (parsed from stdout JSON) ──────────────────────────────

/// Decision returned by a hook.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookResponseDecision {
    /// Approve / continue.
    Approve,
    /// Block the action.
    Block,
}

/// The parsed hook response from a hook's stdout JSON.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HookResponse {
    /// Whether to continue (default true). When false, stop processing.
    #[serde(default = "default_true")]
    pub r#continue: bool,
    /// Whether to suppress stdout from transcript.
    #[serde(default)]
    pub suppress_output: bool,
    /// Message shown when continue is false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
    /// Explicit approve/block decision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<HookResponseDecision>,
    /// Explanation for the decision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Warning message shown to the user.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_message: Option<String>,
    /// Event-specific output payload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hook_specific_output: Option<HookSpecificOutputV2>,
    /// Additional context appended to the prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_context: Option<String>,
}

fn default_true() -> bool {
    true
}

impl Default for HookResponse {
    fn default() -> Self {
        Self {
            r#continue: true,
            suppress_output: false,
            stop_reason: None,
            decision: None,
            reason: None,
            system_message: None,
            hook_specific_output: None,
            additional_context: None,
        }
    }
}

impl HookResponse {
    /// Parse a hook response from raw JSON bytes.
    ///
    /// Returns `Ok(None)` if the input is empty or not valid JSON (not an error).
    pub fn from_json_bytes(data: &[u8]) -> anyhow::Result<Option<Self>> {
        if data.is_empty() {
            return Ok(None);
        }
        let trimmed = String::from_utf8_lossy(data);
        if trimmed.trim().is_empty() {
            return Ok(None);
        }
        let val: Value = match serde_json::from_slice(data) {
            Ok(v) => v,
            Err(_) => return Ok(None),
        };
        // If it's an async response, skip it
        if val.get("async").and_then(|v| v.as_bool()) == Some(true) {
            return Ok(None);
        }
        let response: Self = serde_json::from_value(val)?;
        Ok(Some(response))
    }

    /// Whether the hook wants to block the current action.
    #[must_use]
    pub fn is_blocking(&self) -> bool {
        !self.r#continue || self.decision == Some(HookResponseDecision::Block)
    }
}

// ── Event-specific output ────────────────────────────────────────────────

/// Permission behavior for hook-specific output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionBehavior {
    /// Ask the user.
    Ask,
    /// Deny automatically.
    Deny,
    /// Allow automatically.
    Allow,
    /// Pass through without modification.
    Passthrough,
}

/// Event-specific output payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "hookEventName", rename_all = "PascalCase")]
pub enum HookSpecificOutputV2 {
    /// Output for PreToolUse events.
    PreToolUse {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        permission_decision: Option<PermissionBehavior>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        permission_decision_reason: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        updated_input: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        additional_context: Option<String>,
    },
    /// Output for PostToolUse events.
    PostToolUse {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        additional_context: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        updated_mcp_tool_output: Option<Value>,
    },
    /// Output for PostToolUseFailure events.
    PostToolUseFailure {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        additional_context: Option<String>,
    },
    /// Output for UserPromptSubmit events.
    UserPromptSubmit {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        additional_context: Option<String>,
    },
    /// Output for SessionStart events.
    SessionStart {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        additional_context: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        initial_user_message: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        watch_paths: Option<Vec<String>>,
    },
    /// Output for Notification events.
    Notification {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        additional_context: Option<String>,
    },
    /// Output for PermissionRequest events.
    PermissionRequest {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        decision: Option<PermissionRequestDecision>,
    },
    /// Output for PermissionDenied events.
    PermissionDenied {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        retry: Option<bool>,
    },
    /// Output for SubagentStart events.
    SubagentStart {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        additional_context: Option<String>,
    },
    /// Output for Setup events.
    Setup {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        additional_context: Option<String>,
    },
    /// Output for Elicitation events.
    Elicitation {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        action: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content: Option<Value>,
    },
    /// Output for ElicitationResult events.
    ElicitationResult {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        action: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content: Option<Value>,
    },
    /// Output for CwdChanged events.
    CwdChanged {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        watch_paths: Option<Vec<String>>,
    },
    /// Output for FileChanged events.
    FileChanged {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        watch_paths: Option<Vec<String>>,
    },
    /// Output for WorktreeCreate events.
    WorktreeCreate { worktree_path: String },
}

/// Decision for PermissionRequest hook-specific output.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "behavior", rename_all = "snake_case")]
pub enum PermissionRequestDecision {
    /// Allow the action.
    Allow {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        updated_input: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        updated_permissions: Option<Vec<PermissionUpdate>>,
    },
    /// Deny the action.
    Deny {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        interrupt: Option<bool>,
    },
}

/// Permission update from a hook.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PermissionUpdate {
    /// The permission rule.
    pub rule: String,
    /// The behavior to apply.
    pub behavior: PermissionBehavior,
}

// ── Aggregated hook result ───────────────────────────────────────────────

/// Aggregated result from executing multiple hooks for one event.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AggregatedHookResult {
    /// Whether execution should be blocked.
    #[serde(default)]
    pub blocked: bool,
    /// Blocking error messages.
    #[serde(default)]
    pub blocking_errors: Vec<String>,
    /// Whether to prevent continuation.
    #[serde(default)]
    pub prevent_continuation: bool,
    /// Stop reason message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
    /// Permission decision reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_decision_reason: Option<String>,
    /// Permission behavior override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_behavior: Option<PermissionBehavior>,
    /// Additional context strings.
    #[serde(default)]
    pub additional_contexts: Vec<String>,
    /// Initial user message (from SessionStart hooks).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_user_message: Option<String>,
    /// Updated tool input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_input: Option<Value>,
    /// Updated MCP tool output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_mcp_tool_output: Option<Value>,
    /// Permission request result.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_request_result: Option<PermissionRequestDecision>,
    /// Whether to retry (from PermissionDenied hooks).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<bool>,
}

impl AggregatedHookResult {
    /// Create a new empty result.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Merge a single hook response into this aggregated result.
    pub fn merge_response(&mut self, response: &HookResponse, command_desc: &str) {
        if response.is_blocking() {
            self.blocked = true;
            let msg = response
                .stop_reason
                .clone()
                .unwrap_or_else(|| format!("Hook blocked: {command_desc}"));
            self.blocking_errors.push(msg);
        }
        if !response.r#continue {
            self.prevent_continuation = true;
        }
        if let Some(ref reason) = response.stop_reason {
            self.stop_reason = Some(reason.clone());
        }
        if let Some(ref reason) = response.reason {
            self.permission_decision_reason = Some(reason.clone());
        }
        if let Some(ref ctx) = response.additional_context {
            self.additional_contexts.push(ctx.clone());
        }
        if let Some(ref specific) = response.hook_specific_output {
            self.merge_specific_output(specific);
        }
    }

    /// Merge event-specific output into this result.
    pub fn merge_specific_output(&mut self, output: &HookSpecificOutputV2) {
        match output {
            HookSpecificOutputV2::PreToolUse {
                permission_decision,
                permission_decision_reason,
                updated_input,
                additional_context,
                ..
            } => {
                if let Some(behavior) = permission_decision {
                    self.permission_behavior = Some(*behavior);
                }
                if let Some(reason) = permission_decision_reason {
                    self.permission_decision_reason = Some(reason.clone());
                }
                if let Some(input) = updated_input {
                    self.updated_input = Some(input.clone());
                }
                if let Some(ctx) = additional_context {
                    self.additional_contexts.push(ctx.clone());
                }
            }
            HookSpecificOutputV2::PostToolUse {
                additional_context,
                updated_mcp_tool_output,
                ..
            } => {
                if let Some(ctx) = additional_context {
                    self.additional_contexts.push(ctx.clone());
                }
                if let Some(out) = updated_mcp_tool_output {
                    self.updated_mcp_tool_output = Some(out.clone());
                }
            }
            HookSpecificOutputV2::SessionStart {
                additional_context,
                initial_user_message,
                watch_paths: _,
            } => {
                if let Some(ctx) = additional_context {
                    self.additional_contexts.push(ctx.clone());
                }
                if let Some(msg) = initial_user_message {
                    self.initial_user_message = Some(msg.clone());
                }
            }
            HookSpecificOutputV2::PermissionRequest { decision: Some(d) } => {
                self.permission_request_result = Some(d.clone());
            }
            HookSpecificOutputV2::PermissionRequest { decision: None } => {}
            HookSpecificOutputV2::PermissionDenied { retry: Some(r) } => {
                self.retry = Some(*r);
            }
            HookSpecificOutputV2::PermissionDenied { retry: None } => {}
            HookSpecificOutputV2::WorktreeCreate { worktree_path: _ } => {}
            _ => {
                // Generic: extract additional_context if present
            }
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── HookType tests ───────────────────────────────────────────────────

    #[test]
    fn hook_type_as_str() {
        assert_eq!(HookType::Command.as_str(), "command");
        assert_eq!(HookType::Prompt.as_str(), "prompt");
        assert_eq!(HookType::Agent.as_str(), "agent");
        assert_eq!(HookType::Http.as_str(), "http");
        assert_eq!(HookType::Callback.as_str(), "callback");
        assert_eq!(HookType::Function.as_str(), "function");
    }

    #[test]
    fn hook_type_serde_round_trip() {
        let types = [
            HookType::Command,
            HookType::Prompt,
            HookType::Agent,
            HookType::Http,
            HookType::Callback,
            HookType::Function,
        ];
        for ht in types {
            let json = serde_json::to_string(&ht).expect("serialize");
            let decoded: HookType = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(decoded, ht);
        }
    }

    // ── HookShell tests ──────────────────────────────────────────────────

    #[test]
    fn hook_shell_as_str() {
        assert_eq!(HookShell::Bash.as_str(), "bash");
        assert_eq!(HookShell::PowerShell.as_str(), "powershell");
    }

    #[test]
    fn hook_shell_serde_round_trip() {
        let json = serde_json::to_string(&HookShell::Bash).expect("serialize");
        assert_eq!(json, "\"bash\"");
        let decoded: HookShell = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, HookShell::Bash);
    }

    // ── HookCommand tests ────────────────────────────────────────────────

    #[test]
    fn hook_command_full_serialization() {
        let hook = HookCommand {
            command: "echo hello".to_string(),
            shell: Some(HookShell::Bash),
            timeout: Some(30),
            if_condition: Some("Bash(git *)".to_string()),
            status_message: Some("Running git hook".to_string()),
            once: true,
            r#async: false,
            async_rewake: false,
        };
        let json = serde_json::to_string(&hook).expect("serialize");
        assert!(json.contains("echo hello"));
        assert!(json.contains("git *"));
        let decoded: HookCommand = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, hook);
    }

    #[test]
    fn hook_command_minimal() {
        let json = r#"{"command":"echo test"}"#;
        let hook: HookCommand = serde_json::from_str(json).expect("deserialize");
        assert_eq!(hook.command, "echo test");
        assert!(hook.shell.is_none());
        assert!(hook.timeout.is_none());
        assert!(!hook.once);
    }

    // ── HookPrompt tests ─────────────────────────────────────────────────

    #[test]
    fn hook_prompt_serialization() {
        let hook = HookPrompt {
            prompt: "Check if tests pass".to_string(),
            model: Some("claude-sonnet-4-6".to_string()),
            timeout: Some(60),
            if_condition: None,
            status_message: None,
            once: false,
        };
        let json = serde_json::to_string(&hook).expect("serialize");
        assert!(json.contains("claude-sonnet-4-6"));
        let decoded: HookPrompt = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded.prompt, "Check if tests pass");
    }

    // ── HookAgent tests ──────────────────────────────────────────────────

    #[test]
    fn hook_agent_serialization() {
        let hook = HookAgent {
            prompt: "Verify build passes".to_string(),
            model: None,
            timeout: Some(120),
            if_condition: Some("Bash(cargo *)".to_string()),
            status_message: Some("Verifying...".to_string()),
            once: true,
        };
        let json = serde_json::to_string(&hook).expect("serialize");
        assert!(json.contains("Verify build passes"));
        let decoded: HookAgent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, hook);
    }

    // ── HookHttp tests ───────────────────────────────────────────────────

    #[test]
    fn hook_http_serialization() {
        let mut headers = HashMap::new();
        headers.insert("Authorization".to_string(), "Bearer $TOKEN".to_string());
        let hook = HookHttp {
            url: "https://example.com/hook".to_string(),
            method: Some("POST".to_string()),
            headers,
            allowed_env_vars: vec!["TOKEN".to_string()],
            timeout: Some(10),
            if_condition: None,
            status_message: None,
            once: false,
        };
        let json = serde_json::to_string(&hook).expect("serialize");
        assert!(json.contains("example.com"));
        let decoded: HookHttp = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded.url, "https://example.com/hook");
    }

    // ── HookDefinition tests ─────────────────────────────────────────────

    #[test]
    fn hook_definition_command_tagged() {
        let json = r#"{"type":"command","command":"echo hi"}"#;
        let def: HookDefinition = serde_json::from_str(json).expect("deserialize");
        assert_eq!(def.hook_type(), HookType::Command);
        assert_eq!(def.if_condition(), None);
    }

    #[test]
    fn hook_definition_prompt_tagged() {
        let json = r#"{"type":"prompt","prompt":"review code"}"#;
        let def: HookDefinition = serde_json::from_str(json).expect("deserialize");
        assert_eq!(def.hook_type(), HookType::Prompt);
    }

    #[test]
    fn hook_definition_http_tagged() {
        let json = r#"{"type":"http","url":"https://api.example.com"}"#;
        let def: HookDefinition = serde_json::from_str(json).expect("deserialize");
        assert_eq!(def.hook_type(), HookType::Http);
    }

    #[test]
    fn hook_definition_agent_tagged() {
        let json = r#"{"type":"agent","prompt":"verify tests"}"#;
        let def: HookDefinition = serde_json::from_str(json).expect("deserialize");
        assert_eq!(def.hook_type(), HookType::Agent);
    }

    #[test]
    fn hook_definition_dedup_key() {
        let cmd = HookDefinition::Command(HookCommand {
            command: "echo".to_string(),
            shell: None,
            timeout: None,
            if_condition: None,
            status_message: None,
            once: false,
            r#async: false,
            async_rewake: false,
        });
        assert_eq!(cmd.dedup_key(), "cmd:echo");

        let http = HookDefinition::Http(HookHttp {
            url: "https://x.com".to_string(),
            method: None,
            headers: HashMap::new(),
            allowed_env_vars: vec![],
            timeout: None,
            if_condition: None,
            status_message: None,
            once: false,
        });
        assert_eq!(http.dedup_key(), "http:https://x.com:POST");
    }

    #[test]
    fn hook_definition_timeout_duration() {
        let def = HookDefinition::Command(HookCommand {
            command: "test".to_string(),
            shell: None,
            timeout: Some(5),
            if_condition: None,
            status_message: None,
            once: false,
            r#async: false,
            async_rewake: false,
        });
        assert_eq!(def.timeout_duration(30), Duration::from_secs(5));

        let def_no_timeout = HookDefinition::Command(HookCommand {
            command: "test".to_string(),
            shell: None,
            timeout: None,
            if_condition: None,
            status_message: None,
            once: false,
            r#async: false,
            async_rewake: false,
        });
        assert_eq!(def_no_timeout.timeout_duration(30), Duration::from_secs(30));
    }

    #[test]
    fn hook_definition_is_once() {
        let once_hook = HookDefinition::Command(HookCommand {
            command: "test".to_string(),
            shell: None,
            timeout: None,
            if_condition: None,
            status_message: None,
            once: true,
            r#async: false,
            async_rewake: false,
        });
        assert!(once_hook.is_once());

        let normal_hook = HookDefinition::Command(HookCommand {
            command: "test".to_string(),
            shell: None,
            timeout: None,
            if_condition: None,
            status_message: None,
            once: false,
            r#async: false,
            async_rewake: false,
        });
        assert!(!normal_hook.is_once());
    }

    // ── HookMatcherEntry tests ───────────────────────────────────────────

    #[test]
    fn hook_matcher_entry_serialization() {
        let entry = HookMatcherEntry {
            matcher: Some("Write|Edit".to_string()),
            hooks: vec![HookDefinition::Command(HookCommand {
                command: "lint.sh".to_string(),
                shell: None,
                timeout: None,
                if_condition: None,
                status_message: None,
                once: false,
                r#async: false,
                async_rewake: false,
            })],
        };
        let json = serde_json::to_string(&entry).expect("serialize");
        assert!(json.contains("Write|Edit"));
        let decoded: HookMatcherEntry = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, entry);
    }

    // ── HookOutput tests ─────────────────────────────────────────────────

    #[test]
    fn hook_output_is_success() {
        let output = HookOutput {
            exit_code: Some(0),
            stdout: "ok".to_string(),
            stderr: String::new(),
            parsed_json: None,
        };
        assert!(output.is_success());

        let failed = HookOutput {
            exit_code: Some(1),
            stdout: String::new(),
            stderr: "error".to_string(),
            parsed_json: None,
        };
        assert!(!failed.is_success());
    }

    #[test]
    fn hook_output_parse_stdout_json() {
        let mut output = HookOutput {
            exit_code: Some(0),
            stdout: r#"{"decision":"approve"}"#.to_string(),
            stderr: String::new(),
            parsed_json: None,
        };
        output.parse_stdout();
        assert!(output.parsed_json.is_some());
        let parsed_json = output.parsed_json.as_ref().expect("stdout should parse");
        assert_eq!(parsed_json["decision"], "approve");
    }

    #[test]
    fn hook_output_parse_stdout_non_json() {
        let mut output = HookOutput {
            exit_code: Some(0),
            stdout: "plain text output".to_string(),
            stderr: String::new(),
            parsed_json: None,
        };
        output.parse_stdout();
        assert!(output.parsed_json.is_none());
    }

    // ── HookResponse tests ───────────────────────────────────────────────

    #[test]
    fn hook_response_default_is_continue() {
        let resp = HookResponse::default();
        assert!(resp.r#continue);
        assert!(!resp.is_blocking());
        assert!(!resp.suppress_output);
    }

    #[test]
    fn hook_response_from_json_bytes_empty() {
        let result = HookResponse::from_json_bytes(b"").expect("empty should be ok");
        assert!(result.is_none());
    }

    #[test]
    fn hook_response_from_json_bytes_valid() {
        let json = br#"{"continue":false,"stop_reason":"blocked by policy"}"#;
        let result = HookResponse::from_json_bytes(json)
            .expect("valid json should parse")
            .expect("should have response");
        assert!(!result.r#continue);
        assert_eq!(result.stop_reason.as_deref(), Some("blocked by policy"));
        assert!(result.is_blocking());
    }

    #[test]
    fn hook_response_from_json_bytes_async_skipped() {
        let json = br#"{"async":true,"asyncTimeout":5000}"#;
        let result = HookResponse::from_json_bytes(json).expect("should parse");
        assert!(result.is_none());
    }

    #[test]
    fn hook_response_from_json_bytes_invalid() {
        let result = HookResponse::from_json_bytes(b"not json").expect("should be ok");
        assert!(result.is_none());
    }

    #[test]
    fn hook_response_blocking_decision() {
        let resp = HookResponse {
            r#continue: true,
            decision: Some(HookResponseDecision::Block),
            ..Default::default()
        };
        assert!(resp.is_blocking());
    }

    // ── HookSpecificOutputV2 tests ───────────────────────────────────────

    #[test]
    fn specific_output_pre_tool_use() {
        let json = r#"{
            "hookEventName": "PreToolUse",
            "permission_decision": "allow",
            "additional_context": "extra info"
        }"#;
        let output: HookSpecificOutputV2 = serde_json::from_str(json).expect("deserialize");
        match output {
            HookSpecificOutputV2::PreToolUse {
                permission_decision,
                additional_context,
                ..
            } => {
                assert_eq!(permission_decision, Some(PermissionBehavior::Allow));
                assert_eq!(additional_context.as_deref(), Some("extra info"));
            }
            _ => panic!("expected PreToolUse variant"),
        }
    }

    #[test]
    fn specific_output_session_start() {
        let json = r#"{
            "hookEventName": "SessionStart",
            "initial_user_message": "hello",
            "watch_paths": ["/tmp"]
        }"#;
        let output: HookSpecificOutputV2 = serde_json::from_str(json).expect("deserialize");
        match output {
            HookSpecificOutputV2::SessionStart {
                initial_user_message,
                watch_paths,
                ..
            } => {
                assert_eq!(initial_user_message.as_deref(), Some("hello"));
                let watch_paths = watch_paths.as_ref().expect("watch paths should exist");
                assert_eq!(watch_paths.len(), 1);
                assert_eq!(watch_paths[0], "/tmp");
            }
            _ => panic!("expected SessionStart variant"),
        }
    }

    #[test]
    fn specific_output_worktree_create() {
        let json = r#"{"hookEventName":"WorktreeCreate","worktree_path":"/tmp/wt"}"#;
        let output: HookSpecificOutputV2 = serde_json::from_str(json).expect("deserialize");
        match output {
            HookSpecificOutputV2::WorktreeCreate { worktree_path } => {
                assert_eq!(worktree_path, "/tmp/wt");
            }
            _ => panic!("expected WorktreeCreate variant"),
        }
    }

    // ── AggregatedHookResult tests ───────────────────────────────────────

    #[test]
    fn aggregated_result_default_not_blocked() {
        let result = AggregatedHookResult::new();
        assert!(!result.blocked);
        assert!(result.blocking_errors.is_empty());
        assert!(!result.prevent_continuation);
    }

    #[test]
    fn aggregated_result_merge_blocking_response() {
        let mut result = AggregatedHookResult::new();
        let response = HookResponse {
            r#continue: false,
            stop_reason: Some("policy violation".to_string()),
            ..Default::default()
        };
        result.merge_response(&response, "test-hook");
        assert!(result.blocked);
        assert!(result.prevent_continuation);
        assert_eq!(result.stop_reason.as_deref(), Some("policy violation"));
    }

    #[test]
    fn aggregated_result_merge_approve_response() {
        let mut result = AggregatedHookResult::new();
        let response = HookResponse {
            r#continue: true,
            additional_context: Some("useful info".to_string()),
            ..Default::default()
        };
        result.merge_response(&response, "test-hook");
        assert!(!result.blocked);
        assert_eq!(result.additional_contexts.len(), 1);
        assert_eq!(result.additional_contexts[0], "useful info");
    }

    #[test]
    fn aggregated_result_merge_permission_denied() {
        let mut result = AggregatedHookResult::new();
        let response = HookResponse {
            r#continue: true,
            hook_specific_output: Some(HookSpecificOutputV2::PermissionDenied {
                retry: Some(true),
            }),
            ..Default::default()
        };
        result.merge_response(&response, "test-hook");
        assert_eq!(result.retry, Some(true));
    }

    #[test]
    fn permission_behavior_serde() {
        let json = "\"allow\"";
        let behavior: PermissionBehavior = serde_json::from_str(json).expect("deserialize");
        assert_eq!(behavior, PermissionBehavior::Allow);
    }

    #[test]
    fn permission_request_decision_allow() {
        let json = r#"{"behavior":"allow","updated_input":{"key":"val"}}"#;
        let decision: PermissionRequestDecision = serde_json::from_str(json).expect("deserialize");
        match decision {
            PermissionRequestDecision::Allow { updated_input, .. } => {
                assert!(updated_input.is_some());
            }
            PermissionRequestDecision::Deny { .. } => {
                panic!("expected Allow");
            }
        }
    }

    #[test]
    fn permission_request_decision_deny() {
        let json = r#"{"behavior":"deny","message":"forbidden"}"#;
        let decision: PermissionRequestDecision = serde_json::from_str(json).expect("deserialize");
        match decision {
            PermissionRequestDecision::Deny { message, .. } => {
                assert_eq!(message.as_deref(), Some("forbidden"));
            }
            PermissionRequestDecision::Allow { .. } => {
                panic!("expected Deny");
            }
        }
    }
}
