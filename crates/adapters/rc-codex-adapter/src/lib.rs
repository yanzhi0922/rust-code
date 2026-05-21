//! Codex in-process adapter.
//!
//! [`CodexInProcessAdapter`] wraps the Codex `AppServerClient` (either in-process
//! or remote) and implements the [`AgentAdapter`] trait from `rc-agent-protocol`.
//!
//! # Architecture
//!
//! ```text
//! ┌──────────────────────────────────────────────┐
//! │  CodexInProcessAdapter                       │
//! │  ┌──────────────┐  ┌───────────────────────┐ │
//! │  │ request_handle│  │ event_pump (bg task)  │ │
//! │  │ (Clone)       │  │ owns AppServerClient  │ │
//! │  │               │  │ loops next_event()    │ │
//! │  │ - request()   │  │ maps via event_mapper │ │
//! │  │ - resolve()   │  │ forwards to event_tx  │ │
//! │  │ - reject()    │  └───────────┬───────────┘ │
//! │  └──────┬───────┘              │             │
//! │         │          ┌───────────▼───────────┐ │
//! │         │          │ Arc<Mutex<Option<tx>>> │ │
//! │         │          │ (shared event router)  │ │
//! │         │          └───────────┬───────────┘ │
//! │  send_message() installs new rx│             │
//! │  cancel() sends TurnInterrupt  │             │
//! │  resolve_permission() resolves │             │
//! └──────────────────────────────────────────────┘
//! ```
//!
//! # Usage
//!
//! ```ignore
//! use rc_codex_adapter::CodexInProcessAdapter;
//!
//! // All-in-one: creates an isolated Codex runtime and wraps it.
//! let mut adapter = CodexInProcessAdapter::start_in_process(
//!     working_dir,
//!     Some("gpt-4o".to_string()),
//! ).await?;
//!
//! // Start the adapter (spawns event pump).
//! let config = AgentConfig { .. };
//! adapter.start(&config).await?;
//!
//! // Send a message and receive streaming events.
//! let mut rx = adapter.send_message("session-id", "Hello!").await?;
//! while let Some(event) = rx.recv().await {
//!     println!("{:?}", event);
//! }
//! ```

mod adapter;
mod anthropic_proxy;
mod config;
mod event_mapper;
mod request_routing;
mod types;

// Public API re-exports
pub use adapter::CodexInProcessAdapter;
pub use anthropic_proxy::{AnthropicProxy, AnthropicProxyConfig};
pub use config::{CodexAdapterOptions, isolated_codex_home};
pub use types::{
    CodexExecRequest, CodexFeedbackRequest, CodexPluginRefRequest, CodexServerRequestResolution,
    CodexThreadGoalRefRequest, CodexThreadGoalSetRequest, CodexThreadListRequest,
    CodexThreadRollbackRequest, CodexTurnInterruptRequest, CodexTurnSteerRequest,
};

#[cfg(test)]
mod tests {
    use super::*;
    use codex_app_server_protocol::{RequestPermissionProfile, ToolRequestUserInputQuestion};
    use rc_agent_protocol::permission::PermissionDecision;
    use serde_json::{Value, json};

    fn permissions(value: Value) -> RequestPermissionProfile {
        serde_json::from_value(value).expect("test permission profile should deserialize")
    }

    fn tool_questions(value: Value) -> Vec<ToolRequestUserInputQuestion> {
        serde_json::from_value(value).expect("test tool questions should deserialize")
    }

    fn response_json(
        kind: types::PendingServerRequestKind,
        decision: PermissionDecision,
        resolution: CodexServerRequestResolution,
    ) -> serde_json::Value {
        types::typed_server_request_response(kind, decision, resolution)
            .expect("request response should serialize")
            .expect("request kind should produce a response")
    }

    #[test]
    fn command_execution_allow_uses_turn_scope_decision() {
        let response = response_json(
            types::PendingServerRequestKind::CommandExecution,
            PermissionDecision::Allow,
            CodexServerRequestResolution::default(),
        );

        assert_eq!(response, json!({ "decision": "accept" }));
    }

    #[test]
    fn command_execution_allow_all_uses_session_scope_decision() {
        let response = response_json(
            types::PendingServerRequestKind::CommandExecution,
            PermissionDecision::AllowAll,
            CodexServerRequestResolution::default(),
        );

        assert_eq!(response, json!({ "decision": "acceptForSession" }));
    }

    #[test]
    fn resolution_allow_all_upgrades_allow_to_session_scope() {
        let response = response_json(
            types::PendingServerRequestKind::ExecCommand,
            PermissionDecision::Allow,
            CodexServerRequestResolution {
                allow_all: true,
                response: None,
            },
        );

        assert_eq!(response, json!({ "decision": "approved_for_session" }));
    }

    #[test]
    fn hard_deny_returns_none_for_untyped_or_unavailable_response_kinds() {
        for kind in [
            types::PendingServerRequestKind::Permissions(permissions(json!({
                "network": { "enabled": true }
            }))),
            types::PendingServerRequestKind::ToolUserInput(Vec::new()),
            types::PendingServerRequestKind::DynamicTool {
                call_id: "test".to_string(),
                namespace: None,
                tool: "test_tool".to_string(),
                arguments: json!(null),
            },
            types::PendingServerRequestKind::ChatgptAuthRefresh {
                reason: "Unauthorized".to_string(),
                previous_account_id: None,
            },
        ] {
            let response = types::typed_server_request_response(
                kind,
                PermissionDecision::Deny,
                CodexServerRequestResolution::default(),
            )
            .expect("deny should not fail");

            assert_eq!(response, None);
        }
    }

    #[test]
    fn permissions_grant_preserves_requested_network_and_file_system() {
        let requested = permissions(json!({
            "network": { "enabled": true },
            "fileSystem": {
                "read": ["C:\\repo"],
                "write": ["C:\\repo\\out"]
            }
        }));

        let response = response_json(
            types::PendingServerRequestKind::Permissions(requested),
            PermissionDecision::Allow,
            CodexServerRequestResolution::default(),
        );

        assert_eq!(response["scope"], "turn");
        assert_eq!(
            response["permissions"]["network"],
            json!({ "enabled": true })
        );
        assert_eq!(
            response["permissions"]["fileSystem"],
            json!({
                "read": ["C:\\repo"],
                "write": ["C:\\repo\\out"]
            })
        );
    }

    #[test]
    fn permissions_allow_all_uses_session_scope() {
        let response = response_json(
            types::PendingServerRequestKind::Permissions(permissions(json!({
                "network": { "enabled": true }
            }))),
            PermissionDecision::AllowAll,
            CodexServerRequestResolution::default(),
        );

        assert_eq!(response["scope"], "session");
    }

    #[test]
    fn tool_user_input_defaults_to_first_option_label() {
        let response = response_json(
            types::PendingServerRequestKind::ToolUserInput(tool_questions(json!([
                {
                    "id": "approval_mode",
                    "header": "Mode",
                    "question": "Choose a mode",
                    "options": [
                        { "label": "Careful", "description": "Ask first" },
                        { "label": "Fast", "description": "Proceed" }
                    ]
                }
            ]))),
            PermissionDecision::Allow,
            CodexServerRequestResolution::default(),
        );

        assert_eq!(
            response,
            json!({
                "answers": {
                    "approval_mode": {
                        "answers": ["Careful"]
                    }
                }
            })
        );
    }

    #[test]
    fn custom_response_overrides_allow_response() {
        let custom = json!({
            "decision": "custom",
            "extra": true
        });

        let response = response_json(
            types::PendingServerRequestKind::CommandExecution,
            PermissionDecision::Allow,
            CodexServerRequestResolution {
                allow_all: false,
                response: Some(custom.clone()),
            },
        );

        assert_eq!(response, custom);
    }

    #[test]
    fn custom_response_is_ignored_for_deny() {
        let response = response_json(
            types::PendingServerRequestKind::CommandExecution,
            PermissionDecision::Deny,
            CodexServerRequestResolution {
                allow_all: false,
                response: Some(json!({ "decision": "accept" })),
            },
        );

        assert_eq!(response, json!({ "decision": "decline" }));
    }

    #[test]
    fn chatgpt_auth_refresh_returns_none_without_tokens() {
        let response = types::typed_server_request_response(
            types::PendingServerRequestKind::ChatgptAuthRefresh {
                reason: "Unauthorized".to_string(),
                previous_account_id: None,
            },
            PermissionDecision::Allow,
            CodexServerRequestResolution::default(),
        )
        .expect("auth refresh should not fail");

        assert_eq!(response, None);
    }

    #[test]
    fn chatgpt_auth_refresh_returns_tokens_from_resolution() {
        let tokens = json!({
            "accessToken": "new-token-123",
            "chatgptAccountId": "acct-456",
            "chatgptPlanType": "plus"
        });

        let response = types::typed_server_request_response(
            types::PendingServerRequestKind::ChatgptAuthRefresh {
                reason: "Unauthorized".to_string(),
                previous_account_id: None,
            },
            PermissionDecision::Allow,
            CodexServerRequestResolution {
                allow_all: false,
                response: Some(tokens.clone()),
            },
        )
        .expect("auth refresh should not fail");

        assert_eq!(response, Some(tokens));
    }
}
