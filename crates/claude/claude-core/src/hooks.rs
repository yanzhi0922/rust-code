use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Expanded hook event catalog used by the v2 engine surface.
///
/// This supplements the legacy `claude_core::HookEvent` enum without changing its
/// existing wire contract, so current apps keep compiling while v2 systems can
/// depend on a broader event taxonomy.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum HookEventKind {
    SessionStart,
    SessionEnd,
    PreToolUse,
    PostToolUse,
    PostToolUseFailure,
    Notification,
    UserPromptSubmit,
    AssistantMessageStart,
    AssistantMessageDelta,
    AssistantMessageStop,
    PermissionRequest,
    PermissionResolved,
    CompactStarted,
    CompactCompleted,
    AgentStarted,
    AgentCompleted,
    AgentFailed,
    McpConnectionOpened,
    McpConnectionClosed,
    BackgroundTaskStarted,
    BackgroundTaskCompleted,
    ReviewRequested,
    ReviewCompleted,
    MemoryLoaded,
    MemorySaved,
    StopHookSummary,
    // ── Phase 9: Additional hook events ─────────────────────────────────
    SubagentStart,
    SubagentStop,
    PreCompact,
    PostCompact,
    TeammateIdle,
    Elicitation,
    ElicitationResult,
    ConfigChange,
    WorktreeCreate,
    WorktreeRemove,
    InstructionsLoaded,
    CwdChanged,
    FileChanged,
    Stop,
    StopFailure,
    Setup,
    TaskCreated,
    TaskCompleted,
    /// Fired when a permission request is denied.
    PermissionDenied,
}

/// All standard hook event kinds (27 events matching upstream HOOK_EVENTS).
pub const HOOK_EVENTS: [HookEventKind; 27] = [
    HookEventKind::PreToolUse,
    HookEventKind::PostToolUse,
    HookEventKind::PostToolUseFailure,
    HookEventKind::Notification,
    HookEventKind::UserPromptSubmit,
    HookEventKind::SessionStart,
    HookEventKind::SessionEnd,
    HookEventKind::Stop,
    HookEventKind::StopFailure,
    HookEventKind::SubagentStart,
    HookEventKind::SubagentStop,
    HookEventKind::PreCompact,
    HookEventKind::PostCompact,
    HookEventKind::PermissionRequest,
    HookEventKind::PermissionDenied,
    HookEventKind::Setup,
    HookEventKind::TeammateIdle,
    HookEventKind::TaskCreated,
    HookEventKind::TaskCompleted,
    HookEventKind::Elicitation,
    HookEventKind::ElicitationResult,
    HookEventKind::ConfigChange,
    HookEventKind::WorktreeCreate,
    HookEventKind::WorktreeRemove,
    HookEventKind::InstructionsLoaded,
    HookEventKind::CwdChanged,
    HookEventKind::FileChanged,
];

/// Check if a string matches a known hook event name.
#[must_use]
pub fn is_hook_event(value: &str) -> bool {
    HOOK_EVENTS.iter().any(|e| e.as_str() == value)
}

impl HookEventKind {
    /// Return the upstream-style event name used for prompts/logging.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SessionStart => "SessionStart",
            Self::SessionEnd => "SessionEnd",
            Self::PreToolUse => "PreToolUse",
            Self::PostToolUse => "PostToolUse",
            Self::PostToolUseFailure => "PostToolUseFailure",
            Self::Notification => "Notification",
            Self::UserPromptSubmit => "UserPromptSubmit",
            Self::AssistantMessageStart => "AssistantMessageStart",
            Self::AssistantMessageDelta => "AssistantMessageDelta",
            Self::AssistantMessageStop => "AssistantMessageStop",
            Self::PermissionRequest => "PermissionRequest",
            Self::PermissionResolved => "PermissionResolved",
            Self::CompactStarted => "CompactStarted",
            Self::CompactCompleted => "CompactCompleted",
            Self::AgentStarted => "AgentStarted",
            Self::AgentCompleted => "AgentCompleted",
            Self::AgentFailed => "AgentFailed",
            Self::McpConnectionOpened => "McpConnectionOpened",
            Self::McpConnectionClosed => "McpConnectionClosed",
            Self::BackgroundTaskStarted => "BackgroundTaskStarted",
            Self::BackgroundTaskCompleted => "BackgroundTaskCompleted",
            Self::ReviewRequested => "ReviewRequested",
            Self::ReviewCompleted => "ReviewCompleted",
            Self::MemoryLoaded => "MemoryLoaded",
            Self::MemorySaved => "MemorySaved",
            Self::StopHookSummary => "StopHookSummary",
            // Phase 9
            Self::SubagentStart => "SubagentStart",
            Self::SubagentStop => "SubagentStop",
            Self::PreCompact => "PreCompact",
            Self::PostCompact => "PostCompact",
            Self::TeammateIdle => "TeammateIdle",
            Self::Elicitation => "Elicitation",
            Self::ElicitationResult => "ElicitationResult",
            Self::ConfigChange => "ConfigChange",
            Self::WorktreeCreate => "WorktreeCreate",
            Self::WorktreeRemove => "WorktreeRemove",
            Self::InstructionsLoaded => "InstructionsLoaded",
            Self::CwdChanged => "CwdChanged",
            Self::FileChanged => "FileChanged",
            Self::Stop => "Stop",
            Self::StopFailure => "StopFailure",
            Self::Setup => "Setup",
            Self::TaskCreated => "TaskCreated",
            Self::TaskCompleted => "TaskCompleted",
            Self::PermissionDenied => "PermissionDenied",
        }
    }
}

/// Structured hook event envelope for future transcript/event-stream usage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookEventEnvelope {
    pub event: HookEventKind,
    #[serde(default)]
    pub payload: Value,
}

/// Decision returned by a structured hook handler.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HookDecision {
    Continue,
    Block,
    Retry,
}

/// Hook-specific structured output payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HookSpecificOutput {
    Message { text: String },
    PermissionRule { tool_name: String, rule: String },
    Context { summary: String },
}

/// Structured hook response used by future SDK/runtime integrations.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HookResponse {
    pub decision: HookDecision,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default)]
    pub outputs: Vec<HookSpecificOutput>,
}

#[cfg(test)]
mod tests {
    use super::{
        HOOK_EVENTS, HookDecision, HookEventEnvelope, HookEventKind, HookResponse,
        HookSpecificOutput,
    };

    #[test]
    fn expanded_hook_event_catalog_uses_pascal_case_names() {
        assert_eq!(HookEventKind::CompactCompleted.as_str(), "CompactCompleted");
        assert_eq!(HookEventKind::MemorySaved.as_str(), "MemorySaved");
    }

    #[test]
    fn hook_event_envelope_round_trips() {
        let encoded = serde_json::to_string(&HookEventEnvelope {
            event: HookEventKind::PermissionResolved,
            payload: serde_json::json!({"allowed": true}),
        })
        .expect("hook event should serialize");
        let decoded: HookEventEnvelope =
            serde_json::from_str(&encoded).expect("hook event should deserialize");
        assert_eq!(decoded.event, HookEventKind::PermissionResolved);
        assert_eq!(decoded.payload["allowed"], true);
    }

    #[test]
    fn hook_response_preserves_decision_and_outputs() {
        let response = HookResponse {
            decision: HookDecision::Retry,
            reason: Some("need more context".to_owned()),
            outputs: vec![HookSpecificOutput::Message {
                text: "retrying".to_owned(),
            }],
        };

        let encoded = serde_json::to_string(&response).expect("response should serialize");
        let decoded: HookResponse =
            serde_json::from_str(&encoded).expect("response should deserialize");
        assert_eq!(decoded.decision, HookDecision::Retry);
        assert_eq!(decoded.outputs.len(), 1);
    }

    // ── Phase 9: New hook event tests ─────────────────────────────────────

    #[test]
    fn phase9_subagent_start_event_name() {
        assert_eq!(HookEventKind::SubagentStart.as_str(), "SubagentStart");
    }

    #[test]
    fn phase9_subagent_stop_event_name() {
        assert_eq!(HookEventKind::SubagentStop.as_str(), "SubagentStop");
    }

    #[test]
    fn phase9_pre_compact_event_name() {
        assert_eq!(HookEventKind::PreCompact.as_str(), "PreCompact");
    }

    #[test]
    fn phase9_post_compact_event_name() {
        assert_eq!(HookEventKind::PostCompact.as_str(), "PostCompact");
    }

    #[test]
    fn phase9_teammate_idle_event_name() {
        assert_eq!(HookEventKind::TeammateIdle.as_str(), "TeammateIdle");
    }

    #[test]
    fn phase9_elicitation_event_name() {
        assert_eq!(HookEventKind::Elicitation.as_str(), "Elicitation");
    }

    #[test]
    fn phase9_elicitation_result_event_name() {
        assert_eq!(
            HookEventKind::ElicitationResult.as_str(),
            "ElicitationResult"
        );
    }

    #[test]
    fn phase9_config_change_event_name() {
        assert_eq!(HookEventKind::ConfigChange.as_str(), "ConfigChange");
    }

    #[test]
    fn phase9_worktree_create_event_name() {
        assert_eq!(HookEventKind::WorktreeCreate.as_str(), "WorktreeCreate");
    }

    #[test]
    fn phase9_worktree_remove_event_name() {
        assert_eq!(HookEventKind::WorktreeRemove.as_str(), "WorktreeRemove");
    }

    #[test]
    fn phase9_instructions_loaded_event_name() {
        assert_eq!(
            HookEventKind::InstructionsLoaded.as_str(),
            "InstructionsLoaded"
        );
    }

    #[test]
    fn phase9_cwd_changed_event_name() {
        assert_eq!(HookEventKind::CwdChanged.as_str(), "CwdChanged");
    }

    #[test]
    fn phase9_file_changed_event_name() {
        assert_eq!(HookEventKind::FileChanged.as_str(), "FileChanged");
    }

    #[test]
    fn phase9_stop_event_name() {
        assert_eq!(HookEventKind::Stop.as_str(), "Stop");
    }

    #[test]
    fn phase9_stop_failure_event_name() {
        assert_eq!(HookEventKind::StopFailure.as_str(), "StopFailure");
    }

    #[test]
    fn phase9_setup_event_name() {
        assert_eq!(HookEventKind::Setup.as_str(), "Setup");
    }

    #[test]
    fn phase9_task_created_event_name() {
        assert_eq!(HookEventKind::TaskCreated.as_str(), "TaskCreated");
    }

    #[test]
    fn phase9_task_completed_event_name() {
        assert_eq!(HookEventKind::TaskCompleted.as_str(), "TaskCompleted");
    }

    #[test]
    fn phase9_new_events_round_trip_envelope() {
        let events = vec![
            HookEventKind::SubagentStart,
            HookEventKind::SubagentStop,
            HookEventKind::PreCompact,
            HookEventKind::PostCompact,
            HookEventKind::TeammateIdle,
            HookEventKind::Elicitation,
            HookEventKind::ElicitationResult,
            HookEventKind::ConfigChange,
            HookEventKind::WorktreeCreate,
            HookEventKind::WorktreeRemove,
            HookEventKind::InstructionsLoaded,
            HookEventKind::CwdChanged,
            HookEventKind::FileChanged,
            HookEventKind::Stop,
            HookEventKind::StopFailure,
            HookEventKind::Setup,
            HookEventKind::TaskCreated,
            HookEventKind::TaskCompleted,
        ];

        for event in events {
            let envelope = HookEventEnvelope {
                event,
                payload: serde_json::json!({"test": true}),
            };
            let encoded = serde_json::to_string(&envelope).expect("envelope should serialize");
            let decoded: HookEventEnvelope =
                serde_json::from_str(&encoded).expect("envelope should deserialize");
            assert_eq!(decoded.event, event);
        }
    }

    #[test]
    fn phase9_all_new_events_have_unique_names() {
        let new_events = [
            HookEventKind::SubagentStart,
            HookEventKind::SubagentStop,
            HookEventKind::PreCompact,
            HookEventKind::PostCompact,
            HookEventKind::TeammateIdle,
            HookEventKind::Elicitation,
            HookEventKind::ElicitationResult,
            HookEventKind::ConfigChange,
            HookEventKind::WorktreeCreate,
            HookEventKind::WorktreeRemove,
            HookEventKind::InstructionsLoaded,
            HookEventKind::CwdChanged,
            HookEventKind::FileChanged,
            HookEventKind::Stop,
            HookEventKind::StopFailure,
            HookEventKind::Setup,
            HookEventKind::TaskCreated,
            HookEventKind::TaskCompleted,
        ];

        let names: Vec<&str> = new_events.iter().map(|e| e.as_str()).collect();
        let unique_names: std::collections::HashSet<&str> = names.iter().copied().collect();
        assert_eq!(
            names.len(),
            unique_names.len(),
            "All event names must be unique"
        );
    }

    #[test]
    fn phase9_new_events_serde_round_trip() {
        let event = HookEventKind::TaskCreated;
        let serialized = serde_json::to_string(&event).expect("serialize");
        assert!(serialized.contains("task_created"));
        let deserialized: HookEventKind = serde_json::from_str(&serialized).expect("deserialize");
        assert_eq!(deserialized, event);
    }

    // ── Phase 19: PermissionDenied + is_hook_event tests ────────────────

    #[test]
    fn permission_denied_event_name() {
        assert_eq!(HookEventKind::PermissionDenied.as_str(), "PermissionDenied");
    }

    #[test]
    fn permission_denied_serde_round_trip() {
        let event = HookEventKind::PermissionDenied;
        let serialized = serde_json::to_string(&event).expect("serialize");
        assert!(serialized.contains("permission_denied"));
        let deserialized: HookEventKind = serde_json::from_str(&serialized).expect("deserialize");
        assert_eq!(deserialized, event);
    }

    #[test]
    fn hook_events_constant_has_27_entries() {
        assert_eq!(HOOK_EVENTS.len(), 27);
    }

    #[test]
    fn is_hook_event_recognizes_standard_events() {
        assert!(super::is_hook_event("PreToolUse"));
        assert!(super::is_hook_event("PostToolUse"));
        assert!(super::is_hook_event("SessionStart"));
        assert!(super::is_hook_event("PermissionDenied"));
        assert!(super::is_hook_event("FileChanged"));
        assert!(super::is_hook_event("WorktreeCreate"));
    }

    #[test]
    fn is_hook_event_rejects_unknown() {
        assert!(!super::is_hook_event("UnknownEvent"));
        assert!(!super::is_hook_event("pre_tool_use"));
        assert!(!super::is_hook_event(""));
    }
}
