//! Agent router — manages multiple Agent instances and routes messages.
//!
//! The [`AgentRouter`] holds a mapping from session IDs to Agent adapters and
//! dispatches incoming messages to the correct adapter.
//!
//! All agents run in-process. Use [`register`](AgentRouter::register) with
//! the appropriate in-process adapter (from `rc-claude-adapter`, `rc-codex-adapter`,
//! or `rc-roo-adapter`).

use std::collections::HashMap;

use tracing::warn;

use crate::adapter::AgentAdapter;
use crate::error::AgentProtocolError;
use crate::events::UnifiedAgentEvent;
use crate::permission::PermissionDecision;
use crate::types::AgentType;

/// Manages multiple Agent instances and routes messages by session ID.
///
/// All agents run in-process via their respective adapters:
/// - **Remote Claude** → `ClaudeInProcessAdapter` from `rc-claude-adapter`
/// - **Remote Codex** → `CodexInProcessAdapter` from `rc-codex-adapter`
/// - **Remote Roo** → `RooInProcessAdapter` from `rc-roo-adapter`
///
/// Adapters are created externally and registered via [`register`](AgentRouter::register).
pub struct AgentRouter {
    /// Session ID → Agent adapter.
    adapters: HashMap<String, Box<dyn AgentAdapter>>,
}

impl AgentRouter {
    /// Create a new, empty router.
    pub fn new() -> Self {
        Self {
            adapters: HashMap::new(),
        }
    }

    /// Register a pre-built adapter under the given session ID.
    ///
    /// If an adapter was already registered under the same session ID, it is
    /// stopped before being replaced.
    pub async fn register(&mut self, session_id: String, adapter: Box<dyn AgentAdapter>) {
        if let Some(mut old) = self.adapters.remove(&session_id)
            && let Err(e) = old.stop().await
        {
            warn!(session_id = %session_id, error = %e, "failed to stop old adapter during register");
        }
        self.adapters.insert(session_id, adapter);
    }

    /// Send a message to the Agent bound to `session_id`.
    ///
    /// Returns a receiver that yields [`UnifiedAgentEvent`]s from the Agent.
    pub async fn send_message(
        &mut self,
        session_id: &str,
        message: &str,
    ) -> anyhow::Result<tokio::sync::mpsc::Receiver<UnifiedAgentEvent>> {
        let adapter =
            self.adapters
                .get_mut(session_id)
                .ok_or_else(|| AgentProtocolError::ProtocolError {
                    message: format!("no adapter found for session {session_id}"),
                })?;
        adapter.send_message(session_id, message).await
    }

    /// Cancel the current operation for the given session.
    pub async fn cancel(&mut self, session_id: &str) -> anyhow::Result<()> {
        let adapter =
            self.adapters
                .get_mut(session_id)
                .ok_or_else(|| AgentProtocolError::ProtocolError {
                    message: format!("no adapter found for session {session_id}"),
                })?;
        adapter.cancel(session_id).await
    }

    /// Resolve a permission request for the given session.
    pub async fn resolve_permission(
        &mut self,
        session_id: &str,
        request_id: &str,
        decision: PermissionDecision,
    ) -> anyhow::Result<()> {
        let adapter =
            self.adapters
                .get_mut(session_id)
                .ok_or_else(|| AgentProtocolError::ProtocolError {
                    message: format!("no adapter found for session {session_id}"),
                })?;
        adapter
            .resolve_permission(session_id, request_id, decision)
            .await
    }

    /// Close and remove the session, stopping the underlying Agent.
    pub async fn close_session(&mut self, session_id: &str) -> anyhow::Result<()> {
        if let Some(mut adapter) = self.adapters.remove(session_id) {
            adapter.stop().await?;
        } else {
            warn!(session_id, "attempted to close unknown session");
        }
        Ok(())
    }

    /// Returns the number of active sessions.
    pub fn session_count(&self) -> usize {
        self.adapters.len()
    }

    /// Returns `true` if the router has an adapter for the given session.
    pub fn has_session(&self, session_id: &str) -> bool {
        self.adapters.contains_key(session_id)
    }

    /// Returns `true` if the adapter for the given session is alive and responsive.
    pub fn is_adapter_alive(&self, session_id: &str) -> bool {
        self.adapters
            .get(session_id)
            .map(|adapter| adapter.is_alive())
            .unwrap_or(false)
    }

    /// Returns all session IDs whose adapter matches the given agent type.
    pub fn session_ids_by_type(&self, agent_type: AgentType) -> Vec<String> {
        let mut session_ids = self
            .adapters
            .iter()
            .filter(|(_, adapter)| adapter.agent_type() == agent_type)
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        session_ids.sort_unstable();
        session_ids
    }
}

impl Default for AgentRouter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn router_new_is_empty() {
        let router = AgentRouter::new();
        assert_eq!(router.session_count(), 0);
        assert!(!router.has_session("nonexistent"));
    }

    #[test]
    fn router_default() {
        let router = AgentRouter::default();
        assert_eq!(router.session_count(), 0);
    }

    #[tokio::test]
    async fn router_send_message_unknown_session_fails() {
        let mut router = AgentRouter::new();
        let result = router.send_message("unknown", "hello").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn router_cancel_unknown_session_fails() {
        let mut router = AgentRouter::new();
        let result = router.cancel("unknown").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn router_close_unknown_session_ok() {
        let mut router = AgentRouter::new();
        // Closing a non-existent session should not error.
        let result = router.close_session("unknown").await;
        assert!(result.is_ok());
    }
}
