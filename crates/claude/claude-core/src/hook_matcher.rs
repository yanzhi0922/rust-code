//! Hook matching engine — matcher evaluation, tool-name matching, dedup, and filtering.
//!
//! Mirrors the upstream `getMatchingHooks()` from `hooks.ts`.

use std::collections::HashSet;

use crate::hook_types::{HookDefinition, HookMatcherEntry};
use crate::hooks::HookEventKind;

/// Result of matching hooks against an event and optional tool name.
#[derive(Debug, Clone)]
pub struct MatchedHooks {
    /// Hooks that matched the event and tool-name criteria.
    pub hooks: Vec<HookDefinition>,
    /// Keys of hooks that were deduplicated.
    pub deduplicated_keys: Vec<String>,
}

/// Match hooks for a given event and optional tool name against a list of matcher entries.
///
/// This is the primary entry point: it iterates over all matchers, checks
/// whether the tool name matches the matcher pattern, evaluates the `if`
/// condition, and deduplicates the resulting hooks.
///
/// # Arguments
/// * `matchers` — All matcher entries registered for this event.
/// * `tool_name` — The current tool name (for PreToolUse / PostToolUse).
/// * `input_tool_name` — Tool name from the hook input (for `if` condition).
/// * `input_tool_input` — Tool input JSON (for `if` condition, future use).
pub fn match_hooks(
    matchers: &[HookMatcherEntry],
    tool_name: Option<&str>,
    input_tool_name: Option<&str>,
    input_tool_input: Option<&serde_json::Value>,
) -> MatchedHooks {
    let mut hooks = Vec::new();
    let mut seen_keys = HashSet::new();
    let mut deduplicated_keys = Vec::new();

    for matcher_entry in matchers {
        if !match_tool_name(tool_name, matcher_entry.matcher.as_deref()) {
            continue;
        }

        for hook in &matcher_entry.hooks {
            // Evaluate if_condition against the tool name and input
            if let Some(condition) = hook.if_condition()
                && !evaluate_if_condition(condition, input_tool_name, input_tool_input)
            {
                continue;
            }

            // Deduplicate
            let key = hook.dedup_key();
            if seen_keys.contains(&key) {
                deduplicated_keys.push(key);
                continue;
            }
            seen_keys.insert(key);
            hooks.push(hook.clone());
        }
    }

    MatchedHooks {
        hooks,
        deduplicated_keys,
    }
}

/// Check if a tool name matches a matcher pattern.
///
/// The matcher pattern supports pipe-separated tool names:
/// - `None` or empty → matches everything
/// - `"Write"` → matches only Write
/// - `"Write|Edit"` → matches Write or Edit
/// - `"Bash"` → matches Bash
///
/// Matching is case-sensitive and uses exact equality.
pub fn match_tool_name(tool_name: Option<&str>, matcher: Option<&str>) -> bool {
    let pattern = match matcher {
        None | Some("") => return true,
        Some(p) => p,
    };

    let tool = match tool_name {
        None | Some("") => return true,
        Some(t) => t,
    };

    // Pipe-separated alternatives
    for alt in pattern.split('|') {
        let alt = alt.trim();
        if alt == tool {
            return true;
        }
    }

    false
}

/// Evaluate an `if` condition string against a tool name and optional input.
///
/// The condition uses permission-rule syntax:
/// - `"Bash(git *)"` → matches tool "Bash" with argument pattern "git *"
/// - `"Read"` → matches tool "Read" (any arguments)
/// - `"Write|Edit"` → matches tool "Write" or "Edit"
///
/// Argument patterns support `*` as a wildcard that matches any sequence of
/// characters. For example, `Bash(git *)` matches tool "Bash" when the
/// arguments start with "git ".
pub fn evaluate_if_condition(
    condition: &str,
    tool_name: Option<&str>,
    tool_input: Option<&serde_json::Value>,
) -> bool {
    if condition.is_empty() {
        return true;
    }

    let name = match tool_name {
        None | Some("") => return true,
        Some(n) => n,
    };

    // Support pipe-separated conditions
    for cond in condition.split('|') {
        let cond = cond.trim();
        if evaluate_single_condition(cond, name, tool_input) {
            return true;
        }
    }

    false
}

/// Evaluate a single condition (no pipe separator) against a tool name and
/// optional input.
///
/// Supports:
/// - Simple tool name: `"Bash"` → exact match on tool name.
/// - Tool with argument pattern: `"Bash(git *)"` → matches tool name and
///   checks the argument pattern using wildcard (`*`) matching against the
///   tool's input arguments.
fn evaluate_single_condition(
    condition: &str,
    tool_name: &str,
    tool_input: Option<&serde_json::Value>,
) -> bool {
    // Check for parenthesized argument pattern: "ToolName(pattern)"
    if let Some(paren_pos) = condition.find('(') {
        let cond_tool = &condition[..paren_pos];
        if cond_tool != tool_name {
            return false;
        }
        // Extract the argument pattern between parentheses
        let close_paren = match condition.rfind(')') {
            Some(pos) => pos,
            None => condition.len(),
        };
        let arg_pattern = &condition[paren_pos + 1..close_paren];

        // If no tool input is available, we cannot verify the argument pattern.
        // Fall back to matching on tool name only (conservative: allow the match
        // so that argument filtering can be applied later when input is available).
        let Some(input) = tool_input else {
            return true;
        };

        // Extract the argument string from tool input.
        // For Bash-like tools the arguments are typically in "command" field.
        // For other tools, try "path", "file_path", "pattern", or fall back to
        // stringifying the entire input.
        let arg_str = extract_tool_argument(input);
        return wildcard_match(arg_pattern, &arg_str);
    }

    // Simple tool name match
    condition == tool_name
}

/// Extract a representative argument string from a tool's input JSON.
///
/// Tries common field names used across different tool types:
/// - `command` (Bash, shell tools)
/// - `file_path` / `path` (file operation tools)
/// - `pattern` (search tools)
///
/// Falls back to stringifying the entire input if no known field is found.
fn extract_tool_argument(input: &serde_json::Value) -> String {
    if let Some(s) = input.get("command").and_then(|v| v.as_str()) {
        return s.to_owned();
    }
    if let Some(s) = input.get("file_path").and_then(|v| v.as_str()) {
        return s.to_owned();
    }
    if let Some(s) = input.get("path").and_then(|v| v.as_str()) {
        return s.to_owned();
    }
    if let Some(s) = input.get("pattern").and_then(|v| v.as_str()) {
        return s.to_owned();
    }
    // Fallback: use the entire JSON as the argument string
    input.to_string()
}

/// Simple wildcard pattern matching where `*` matches any sequence of characters.
///
/// The pattern is matched against the beginning of the text (prefix match).
/// If the pattern does not contain `*`, it is compared as a prefix of the text.
fn wildcard_match(pattern: &str, text: &str) -> bool {
    if pattern.is_empty() {
        return true;
    }

    // Split the pattern by '*' and match each segment sequentially
    let mut text_remaining = text;
    let segments: Vec<&str> = pattern.split('*').collect();

    for (i, segment) in segments.iter().enumerate() {
        if segment.is_empty() {
            continue;
        }
        match text_remaining.find(segment) {
            Some(pos) => {
                // For the first segment, it must match at the start (position 0)
                // unless preceded by a '*' wildcard
                if i == 0 && pos != 0 {
                    return false;
                }
                text_remaining = &text_remaining[pos + segment.len()..];
            }
            None => return false,
        }
    }

    // If the pattern doesn't end with '*', the match must consume to the end
    // of the text — unless the last segment is empty (pattern ends with '*')
    if !pattern.ends_with('*') && !text_remaining.is_empty() {
        // Allow partial match: the pattern is a prefix of the text
        // This handles cases like "git *" where the text is "git commit -m 'msg'"
    }

    true
}

/// Deduplicate a list of hooks by their dedup keys.
///
/// Returns the deduplicated list and the count of removed duplicates.
pub fn deduplicate_hooks(hooks: &[HookDefinition]) -> (Vec<HookDefinition>, usize) {
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    let mut removed = 0;

    for hook in hooks {
        let key = hook.dedup_key();
        if seen.insert(key) {
            result.push(hook.clone());
        } else {
            removed += 1;
        }
    }

    (result, removed)
}

/// Filter hooks by workspace trust level.
///
/// In **untrusted** workspaces, only hooks that execute in-process
/// ([`HookDefinition::Callback`] and [`HookDefinition::Function`]) are
/// permitted. All external-execution hooks (command, HTTP, prompt, agent)
/// are stripped out because they could run arbitrary code.
///
/// In **trusted** workspaces, all hooks are allowed.
pub fn filter_hooks_by_trust(hooks: &[HookDefinition], trusted: bool) -> Vec<HookDefinition> {
    if trusted {
        return hooks.to_vec();
    }
    hooks
        .iter()
        .filter(|h| matches!(h, HookDefinition::Callback(_) | HookDefinition::Function(_)))
        .cloned()
        .collect()
}

/// Filter hooks by managed policy.
///
/// Managed hooks (set by enterprise policy) are merged into the result
/// and cannot be overridden by user-defined hooks. If a managed hook has
/// the same deduplication key as a user hook, the managed version wins.
pub fn filter_hooks_by_managed(
    hooks: &[HookDefinition],
    managed_hooks: &[HookDefinition],
) -> Vec<HookDefinition> {
    if managed_hooks.is_empty() {
        return hooks.to_vec();
    }

    // Collect dedup keys of managed hooks — these take priority.
    let managed_keys: std::collections::HashSet<String> =
        managed_hooks.iter().map(|h| h.dedup_key()).collect();

    // Start with managed hooks, then add user hooks that don't conflict.
    let mut result: Vec<HookDefinition> = managed_hooks.to_vec();
    for hook in hooks {
        if !managed_keys.contains(&hook.dedup_key()) {
            result.push(hook.clone());
        }
    }
    result
}

/// Check if a hook event is one of the 26 standard hook events.
pub fn is_hook_event(value: &str) -> bool {
    HOOK_EVENT_NAMES.contains(&value)
}

/// All 26 standard hook event names (matching upstream HOOK_EVENTS).
pub static HOOK_EVENT_NAMES: &[&str] = &[
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "Notification",
    "UserPromptSubmit",
    "SessionStart",
    "SessionEnd",
    "Stop",
    "StopFailure",
    "SubagentStart",
    "SubagentStop",
    "PreCompact",
    "PostCompact",
    "PermissionRequest",
    "PermissionDenied",
    "Setup",
    "TeammateIdle",
    "TaskCreated",
    "TaskCompleted",
    "Elicitation",
    "ElicitationResult",
    "ConfigChange",
    "WorktreeCreate",
    "WorktreeRemove",
    "InstructionsLoaded",
    "CwdChanged",
    "FileChanged",
];

/// Convert a string to a [`HookEventKind`] if it matches a known event.
pub fn parse_hook_event(value: &str) -> Option<HookEventKind> {
    match value {
        "PreToolUse" => Some(HookEventKind::PreToolUse),
        "PostToolUse" => Some(HookEventKind::PostToolUse),
        "PostToolUseFailure" => Some(HookEventKind::PostToolUseFailure),
        "Notification" => Some(HookEventKind::Notification),
        "UserPromptSubmit" => Some(HookEventKind::UserPromptSubmit),
        "SessionStart" => Some(HookEventKind::SessionStart),
        "SessionEnd" => Some(HookEventKind::SessionEnd),
        "Stop" => Some(HookEventKind::Stop),
        "StopFailure" => Some(HookEventKind::StopFailure),
        "SubagentStart" => Some(HookEventKind::SubagentStart),
        "SubagentStop" => Some(HookEventKind::SubagentStop),
        "PreCompact" => Some(HookEventKind::PreCompact),
        "PostCompact" => Some(HookEventKind::PostCompact),
        "PermissionRequest" => Some(HookEventKind::PermissionRequest),
        "PermissionDenied" => Some(HookEventKind::PermissionDenied),
        "Setup" => Some(HookEventKind::Setup),
        "TeammateIdle" => Some(HookEventKind::TeammateIdle),
        "TaskCreated" => Some(HookEventKind::TaskCreated),
        "TaskCompleted" => Some(HookEventKind::TaskCompleted),
        "Elicitation" => Some(HookEventKind::Elicitation),
        "ElicitationResult" => Some(HookEventKind::ElicitationResult),
        "ConfigChange" => Some(HookEventKind::ConfigChange),
        "WorktreeCreate" => Some(HookEventKind::WorktreeCreate),
        "WorktreeRemove" => Some(HookEventKind::WorktreeRemove),
        "InstructionsLoaded" => Some(HookEventKind::InstructionsLoaded),
        "CwdChanged" => Some(HookEventKind::CwdChanged),
        "FileChanged" => Some(HookEventKind::FileChanged),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hook_types::{
        HookCallback, HookCommand, HookFunction, HookMatcherEntry, HookPrompt,
    };

    fn make_command_hook(cmd: &str) -> HookDefinition {
        HookDefinition::Command(HookCommand {
            command: cmd.to_string(),
            shell: None,
            timeout: None,
            if_condition: None,
            status_message: None,
            once: false,
            r#async: false,
            async_rewake: false,
        })
    }

    fn make_command_hook_with_condition(cmd: &str, condition: &str) -> HookDefinition {
        HookDefinition::Command(HookCommand {
            command: cmd.to_string(),
            shell: None,
            timeout: None,
            if_condition: Some(condition.to_string()),
            status_message: None,
            once: false,
            r#async: false,
            async_rewake: false,
        })
    }

    // ── match_tool_name tests ────────────────────────────────────────────

    #[test]
    fn match_tool_name_none_matcher_matches_all() {
        assert!(match_tool_name(Some("Bash"), None));
        assert!(match_tool_name(Some("Write"), None));
        assert!(match_tool_name(None, None));
    }

    #[test]
    fn match_tool_name_empty_matcher_matches_all() {
        assert!(match_tool_name(Some("Bash"), Some("")));
        assert!(match_tool_name(None, Some("")));
    }

    #[test]
    fn match_tool_name_exact_match() {
        assert!(match_tool_name(Some("Write"), Some("Write")));
        assert!(!match_tool_name(Some("Write"), Some("Edit")));
    }

    #[test]
    fn match_tool_name_pipe_separated() {
        assert!(match_tool_name(Some("Write"), Some("Write|Edit")));
        assert!(match_tool_name(Some("Edit"), Some("Write|Edit")));
        assert!(match_tool_name(Some("Bash"), Some("Bash|Read|Write")));
        assert!(!match_tool_name(Some("Grep"), Some("Write|Edit")));
    }

    #[test]
    fn match_tool_name_none_tool_always_matches() {
        assert!(match_tool_name(None, Some("Write")));
    }

    #[test]
    fn match_tool_name_case_sensitive() {
        assert!(!match_tool_name(Some("write"), Some("Write")));
        assert!(match_tool_name(Some("Write"), Some("Write")));
    }

    // ── evaluate_if_condition tests ──────────────────────────────────────

    #[test]
    fn evaluate_if_empty_condition() {
        assert!(evaluate_if_condition("", Some("Bash"), None));
    }

    #[test]
    fn evaluate_if_none_tool_name() {
        assert!(evaluate_if_condition("Bash", None, None));
    }

    #[test]
    fn evaluate_if_simple_tool_name() {
        assert!(evaluate_if_condition("Bash", Some("Bash"), None));
        assert!(!evaluate_if_condition("Bash", Some("Write"), None));
    }

    #[test]
    fn evaluate_if_parenthesized_pattern() {
        assert!(evaluate_if_condition("Bash(git *)", Some("Bash"), None));
        assert!(!evaluate_if_condition("Bash(git *)", Some("Write"), None));
    }

    #[test]
    fn evaluate_if_pipe_separated() {
        assert!(evaluate_if_condition("Bash|Write", Some("Bash"), None));
        assert!(evaluate_if_condition("Bash|Write", Some("Write"), None));
        assert!(!evaluate_if_condition("Bash|Write", Some("Edit"), None));
    }

    // ── match_hooks tests ────────────────────────────────────────────────

    #[test]
    fn match_hooks_no_matchers() {
        let result = match_hooks(&[], Some("Bash"), Some("Bash"), None);
        assert!(result.hooks.is_empty());
    }

    #[test]
    fn match_hooks_matching_tool() {
        let matchers = vec![HookMatcherEntry {
            matcher: Some("Bash".to_string()),
            hooks: vec![make_command_hook("echo bash-hook")],
        }];
        let result = match_hooks(&matchers, Some("Bash"), Some("Bash"), None);
        assert_eq!(result.hooks.len(), 1);
    }

    #[test]
    fn match_hooks_non_matching_tool() {
        let matchers = vec![HookMatcherEntry {
            matcher: Some("Write".to_string()),
            hooks: vec![make_command_hook("echo write-hook")],
        }];
        let result = match_hooks(&matchers, Some("Bash"), Some("Bash"), None);
        assert!(result.hooks.is_empty());
    }

    #[test]
    fn match_hooks_no_matcher_matches_all() {
        let matchers = vec![HookMatcherEntry {
            matcher: None,
            hooks: vec![make_command_hook("echo global-hook")],
        }];
        let result = match_hooks(&matchers, Some("Bash"), Some("Bash"), None);
        assert_eq!(result.hooks.len(), 1);
    }

    #[test]
    fn match_hooks_deduplicates() {
        let matchers = vec![
            HookMatcherEntry {
                matcher: None,
                hooks: vec![make_command_hook("echo dup")],
            },
            HookMatcherEntry {
                matcher: None,
                hooks: vec![make_command_hook("echo dup")],
            },
        ];
        let result = match_hooks(&matchers, Some("Bash"), Some("Bash"), None);
        assert_eq!(result.hooks.len(), 1);
        assert_eq!(result.deduplicated_keys.len(), 1);
    }

    #[test]
    fn match_hooks_with_if_condition_pass() {
        let matchers = vec![HookMatcherEntry {
            matcher: None,
            hooks: vec![make_command_hook_with_condition("echo cond", "Bash")],
        }];
        let result = match_hooks(&matchers, Some("Bash"), Some("Bash"), None);
        assert_eq!(result.hooks.len(), 1);
    }

    #[test]
    fn match_hooks_with_if_condition_fail() {
        let matchers = vec![HookMatcherEntry {
            matcher: None,
            hooks: vec![make_command_hook_with_condition("echo cond", "Write")],
        }];
        let result = match_hooks(&matchers, Some("Bash"), Some("Bash"), None);
        assert!(result.hooks.is_empty());
    }

    #[test]
    fn match_hooks_multiple_matchers() {
        let matchers = vec![
            HookMatcherEntry {
                matcher: Some("Bash".to_string()),
                hooks: vec![make_command_hook("bash-hook")],
            },
            HookMatcherEntry {
                matcher: None,
                hooks: vec![make_command_hook("global-hook")],
            },
        ];
        let result = match_hooks(&matchers, Some("Bash"), Some("Bash"), None);
        assert_eq!(result.hooks.len(), 2);
    }

    // ── deduplicate_hooks tests ──────────────────────────────────────────

    #[test]
    fn deduplicate_no_duplicates() {
        let hooks = vec![make_command_hook("a"), make_command_hook("b")];
        let (deduped, removed) = deduplicate_hooks(&hooks);
        assert_eq!(deduped.len(), 2);
        assert_eq!(removed, 0);
    }

    #[test]
    fn deduplicate_with_duplicates() {
        let hooks = vec![
            make_command_hook("a"),
            make_command_hook("b"),
            make_command_hook("a"),
        ];
        let (deduped, removed) = deduplicate_hooks(&hooks);
        assert_eq!(deduped.len(), 2);
        assert_eq!(removed, 1);
    }

    #[test]
    fn deduplicate_different_types() {
        let hooks = vec![
            make_command_hook("a"),
            HookDefinition::Prompt(HookPrompt {
                prompt: "test".to_string(),
                model: None,
                timeout: None,
                if_condition: None,
                status_message: None,
                once: false,
            }),
        ];
        let (deduped, removed) = deduplicate_hooks(&hooks);
        assert_eq!(deduped.len(), 2);
        assert_eq!(removed, 0);
    }

    // ── is_hook_event tests ──────────────────────────────────────────────

    #[test]
    fn is_hook_event_valid() {
        assert!(is_hook_event("PreToolUse"));
        assert!(is_hook_event("PostToolUse"));
        assert!(is_hook_event("SessionStart"));
        assert!(is_hook_event("FileChanged"));
        assert!(is_hook_event("WorktreeCreate"));
    }

    #[test]
    fn is_hook_event_invalid() {
        assert!(!is_hook_event("UnknownEvent"));
        assert!(!is_hook_event("pre_tool_use"));
        assert!(!is_hook_event(""));
    }

    #[test]
    fn is_hook_event_all_26() {
        // Verify all 26+ events are recognized
        for &name in HOOK_EVENT_NAMES {
            assert!(
                is_hook_event(name),
                "Expected '{name}' to be a valid hook event"
            );
        }
    }

    // ── parse_hook_event tests ───────────────────────────────────────────

    #[test]
    fn parse_hook_event_valid() {
        assert_eq!(
            parse_hook_event("PreToolUse"),
            Some(HookEventKind::PreToolUse)
        );
        assert_eq!(
            parse_hook_event("SessionStart"),
            Some(HookEventKind::SessionStart)
        );
        assert_eq!(
            parse_hook_event("FileChanged"),
            Some(HookEventKind::FileChanged)
        );
    }

    #[test]
    fn parse_hook_event_invalid() {
        assert_eq!(parse_hook_event("Unknown"), None);
        assert_eq!(parse_hook_event(""), None);
    }

    // ── filter_hooks_by_trust tests ──────────────────────────────────────

    #[test]
    fn filter_by_trust_allows_all_when_trusted() {
        let hooks = vec![make_command_hook("a"), make_command_hook("b")];
        let filtered = filter_hooks_by_trust(&hooks, true);
        assert_eq!(filtered.len(), 2);
    }

    #[test]
    fn filter_by_trust_strips_external_hooks_when_untrusted() {
        let hooks = vec![make_command_hook("a")];
        let filtered = filter_hooks_by_trust(&hooks, false);
        assert!(
            filtered.is_empty(),
            "command hooks should be filtered in untrusted workspace"
        );
    }

    #[test]
    fn filter_by_trust_keeps_callback_and_function_when_untrusted() {
        let cb = HookDefinition::Callback(HookCallback {
            callback_id: "cb1".to_string(),
            timeout: None,
        });
        let fn_ = HookDefinition::Function(HookFunction {
            function_id: "fn1".to_string(),
            timeout: None,
        });
        let hooks = vec![make_command_hook("cmd"), cb.clone(), fn_.clone()];
        let filtered = filter_hooks_by_trust(&hooks, false);
        assert_eq!(filtered.len(), 2);
        assert!(filtered.contains(&cb));
        assert!(filtered.contains(&fn_));
    }

    // ── filter_hooks_by_managed tests ────────────────────────────────────

    #[test]
    fn filter_by_managed_no_managed_returns_all() {
        let hooks = vec![make_command_hook("a")];
        let filtered = filter_hooks_by_managed(&hooks, &[]);
        assert_eq!(filtered.len(), 1);
    }

    #[test]
    fn filter_by_managed_merges_managed_hooks() {
        let hooks = vec![make_command_hook("a")];
        let managed = vec![make_command_hook("m")];
        let filtered = filter_hooks_by_managed(&hooks, &managed);
        // Both should be present: 1 managed + 1 user
        assert_eq!(filtered.len(), 2);
    }

    #[test]
    fn filter_by_managed_managed_overrides_user() {
        let user_hook = make_command_hook("lint.sh");
        let managed_hook = make_command_hook("lint.sh");
        let filtered = filter_hooks_by_managed(
            std::slice::from_ref(&user_hook),
            std::slice::from_ref(&managed_hook),
        );
        // Managed wins — only 1 hook with that dedup key
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0], managed_hook);
    }

    // ── HOOK_EVENT_NAMES tests ───────────────────────────────────────────

    #[test]
    fn hook_event_names_count() {
        // Should have at least 26 events (some may be extended)
        assert!(HOOK_EVENT_NAMES.len() >= 26);
    }

    #[test]
    fn hook_event_names_unique() {
        let set: HashSet<&&str> = HOOK_EVENT_NAMES.iter().collect();
        assert_eq!(set.len(), HOOK_EVENT_NAMES.len());
    }
}
