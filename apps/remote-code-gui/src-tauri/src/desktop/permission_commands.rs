use super::*;

#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub(super) async fn resolve_permission_request(
    app: AppHandle,
    state: State<'_, AppState>,
    request_id: String,
    allowed: bool,
    message: Option<String>,
    updated_input: Option<serde_json::Value>,
    permission_updates: Option<Vec<claude_permissions::PermissionUpdate>>,
    feedback: Option<String>,
    content_blocks: Option<Vec<serde_json::Value>>,
    codex_response: Option<serde_json::Value>,
    allow_all: Option<bool>,
) -> std::result::Result<bool, String> {
    let mut decision = if allowed {
        PermissionDecision::allow()
    } else {
        PermissionDecision::deny(
            message
                .clone()
                .unwrap_or_else(|| "Permission denied by GUI.".to_owned()),
        )
    };
    if allowed {
        decision.message = message.clone();
    }
    decision.updated_input = updated_input.clone();
    decision.permission_updates = permission_updates.clone().unwrap_or_default();
    decision.feedback = feedback.clone();
    decision.content_blocks = content_blocks.clone().unwrap_or_default();

    let sender = {
        let mut pending = state.pending_permissions.lock().await;
        pending.remove(&request_id)
    };
    if let Some(sender) = sender {
        let _ = sender.send(decision);
        Ok(true)
    } else {
        let pending_codex = {
            let pending = state.pending_codex_permissions.lock().await;
            pending.get(&request_id).cloned()
        };
        let Some(pending_codex) = pending_codex else {
            // Check Roo permissions.
            let pending_roo = {
                let pending = state.pending_roo_permissions.lock().await;
                pending.get(&request_id).cloned()
            };
            let Some(pending_roo) = pending_roo else {
                // Check Claude adapter permissions as last resort.
                let pending_claude = {
                    let pending = state.pending_claude_permissions.lock().await;
                    pending.get(&request_id).cloned()
                };
                let Some(pending_claude) = pending_claude else {
                    return Ok(false);
                };

                {
                    let mut adapters = state.active_claude_adapters.lock().await;
                    if let Some(adapter) = adapters.get_mut(&pending_claude.session_id) {
                        let decision = if allow_all.unwrap_or(false) {
                            AgentPermissionDecision::AllowAll
                        } else if allowed {
                            AgentPermissionDecision::Allow
                        } else {
                            AgentPermissionDecision::Deny
                        };
                        let _ = adapter
                            .resolve_permission(
                                &pending_claude.session_id,
                                &pending_claude.request_id,
                                decision,
                            )
                            .await;
                    }
                }

                {
                    let mut pending = state.pending_claude_permissions.lock().await;
                    pending.remove(&request_id);
                }

                let _ = app.emit(
                    APP_EVENT_PERMISSION_RESOLVED,
                    PermissionDecisionDto {
                        request_id,
                        allowed,
                        message: message.clone(),
                        updated_input: None,
                        permission_updates: vec![],
                        feedback: None,
                        content_blocks: vec![],
                    },
                );

                return Ok(true);
            };

            {
                let mut adapters = state.active_roo_adapters.lock().await;
                if let Some(adapter) = adapters.get_mut(&pending_roo.session_id) {
                    let decision = if allow_all.unwrap_or(false) && allowed {
                        AgentPermissionDecision::AllowAll
                    } else if allowed {
                        AgentPermissionDecision::Allow
                    } else {
                        AgentPermissionDecision::Deny
                    };
                    let response = feedback.clone().or_else(|| message.clone());
                    let _ = adapter
                        .resolve_roo_permission(
                            &pending_roo.request_id,
                            decision,
                            response,
                            Some(&pending_roo.request_kind),
                        )
                        .await;
                }
            }

            {
                let mut pending = state.pending_roo_permissions.lock().await;
                pending.remove(&request_id);
            }

            let _ = app.emit(
                APP_EVENT_PERMISSION_RESOLVED,
                PermissionDecisionDto {
                    request_id,
                    allowed,
                    message: message.clone(),
                    updated_input: None,
                    permission_updates: vec![],
                    feedback,
                    content_blocks: vec![],
                },
            );

            return Ok(true);
        };

        let permission_updates_for_emit = permission_updates.unwrap_or_default();
        let codex_decision = if allow_all.unwrap_or(false) && allowed {
            AgentPermissionDecision::AllowAll
        } else {
            codex_permission_decision(allowed, &permission_updates_for_emit)
        };

        {
            let mut adapters = state.active_codex_adapters.lock().await;
            let adapter = adapters
                .get_mut(&pending_codex.session_id)
                .ok_or_else(|| "Codex adapter not found for permission request".to_owned())?;
            adapter
                .resolve_codex_server_request(
                    &pending_codex.request_id,
                    codex_decision,
                    CodexServerRequestResolution {
                        allow_all: allow_all.unwrap_or(false),
                        response: codex_response,
                    },
                )
                .await
                .map_err(|error| {
                    format!("Failed to resolve Codex permission request: {error:#}")
                })?;
        }

        {
            let mut pending = state.pending_codex_permissions.lock().await;
            pending.remove(&request_id);
        }

        let _ = app.emit(
            APP_EVENT_PERMISSION_RESOLVED,
            PermissionDecisionDto {
                request_id,
                allowed,
                message,
                updated_input,
                permission_updates: permission_updates_for_emit,
                feedback,
                content_blocks: content_blocks.unwrap_or_default(),
            },
        );

        Ok(true)
    }
}

#[tauri::command]
pub(super) async fn resolve_roo_permission_request(
    app: AppHandle,
    state: State<'_, AppState>,
    request_id: String,
    allowed: bool,
    message: Option<String>,
    feedback: Option<String>,
    allow_all: Option<bool>,
) -> std::result::Result<bool, String> {
    let pending_roo = {
        let pending = state.pending_roo_permissions.lock().await;
        pending.get(&request_id).cloned()
    };
    let Some(pending_roo) = pending_roo else {
        return Ok(false);
    };

    {
        let mut adapters = state.active_roo_adapters.lock().await;
        let adapter = adapters
            .get_mut(&pending_roo.session_id)
            .ok_or_else(|| "Roo adapter not found for permission request".to_owned())?;
        let decision = if allow_all.unwrap_or(false) && allowed {
            AgentPermissionDecision::AllowAll
        } else if allowed {
            AgentPermissionDecision::Allow
        } else {
            AgentPermissionDecision::Deny
        };
        let response = feedback.clone().or_else(|| message.clone());
        adapter
            .resolve_roo_permission(
                &pending_roo.request_id,
                decision,
                response,
                Some(&pending_roo.request_kind),
            )
            .await
            .map_err(|error| format!("Failed to resolve Roo permission request: {error:#}"))?;
    }

    {
        let mut pending = state.pending_roo_permissions.lock().await;
        pending.remove(&request_id);
    }

    let _ = app.emit(
        APP_EVENT_PERMISSION_RESOLVED,
        PermissionDecisionDto {
            request_id,
            allowed,
            message,
            updated_input: None,
            permission_updates: vec![],
            feedback,
            content_blocks: vec![],
        },
    );

    Ok(true)
}

#[tauri::command]
pub(super) async fn resolve_claude_permission_request(
    app: AppHandle,
    state: State<'_, AppState>,
    request_id: String,
    allowed: bool,
    allow_all: Option<bool>,
) -> std::result::Result<bool, String> {
    let pending_claude = {
        let pending = state.pending_claude_permissions.lock().await;
        pending.get(&request_id).cloned()
    };
    let Some(pending_claude) = pending_claude else {
        return Ok(false);
    };

    {
        let mut adapters = state.active_claude_adapters.lock().await;
        let adapter = adapters
            .get_mut(&pending_claude.session_id)
            .ok_or_else(|| "Claude adapter not found for permission request".to_owned())?;
        let decision = if allow_all.unwrap_or(false) {
            AgentPermissionDecision::AllowAll
        } else if allowed {
            AgentPermissionDecision::Allow
        } else {
            AgentPermissionDecision::Deny
        };
        adapter
            .resolve_permission(
                &pending_claude.session_id,
                &pending_claude.request_id,
                decision,
            )
            .await
            .map_err(|error| format!("Failed to resolve Claude permission request: {error:#}"))?;
    }

    {
        let mut pending = state.pending_claude_permissions.lock().await;
        pending.remove(&request_id);
    }

    let _ = app.emit(
        APP_EVENT_PERMISSION_RESOLVED,
        PermissionDecisionDto {
            request_id,
            allowed,
            message: None,
            updated_input: None,
            permission_updates: vec![],
            feedback: None,
            content_blocks: vec![],
        },
    );

    Ok(true)
}
