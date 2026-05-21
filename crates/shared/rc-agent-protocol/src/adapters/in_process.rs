//! Shared in-process adapter implementation.
//!
//! [`InProcessAdapter`] is a generic callback-based adapter used primarily in
//! integration tests. Each Agent type is exposed as a type alias via its own
//! module (`RemoteClaudeAdapter`, `RemoteRooAdapter`, `RemoteCodexAdapter`).

use std::collections::HashSet;

use async_trait::async_trait;
use tokio::sync::mpsc;
use tracing::info;

use crate::adapter::AgentAdapter;
use crate::events::UnifiedAgentEvent;
use crate::permission::PermissionDecision;
use crate::types::{AgentCapability, AgentConfig, AgentInfo, AgentStatus, AgentType};

// ---------------------------------------------------------------------------
// Callback type aliases
// ---------------------------------------------------------------------------

pub(crate) type SendMessageFn =
    Box<dyn Fn(&str, &str) -> anyhow::Result<Vec<UnifiedAgentEvent>> + Send + Sync>;

pub(crate) type CancelFn = Box<dyn Fn(&str) -> anyhow::Result<()> + Send + Sync>;

pub(crate) type ResolvePermissionFn =
    Box<dyn Fn(&str, &str, PermissionDecision) -> anyhow::Result<()> + Send + Sync>;

// ---------------------------------------------------------------------------
// InProcessAdapter
// ---------------------------------------------------------------------------

/// Generic in-process adapter backed by callback functions.
///
/// Used primarily as a test double in integration tests. Production code uses
/// the dedicated adapters from `rc-claude-adapter`, `rc-codex-adapter`, and
/// `rc-roo-adapter` crates.
pub struct InProcessAdapter {
    /// Static agent metadata.
    pub(crate) info: AgentInfo,
    /// Runtime status.
    pub(crate) status: AgentStatus,
    /// The agent type discriminator.
    pub(crate) agent_type: AgentType,
    /// Callback invoked by [`send_message`](AgentAdapter::send_message).
    pub(crate) on_send_message: Option<SendMessageFn>,
    /// Callback invoked by [`cancel`](AgentAdapter::cancel).
    pub(crate) on_cancel: Option<CancelFn>,
    /// Callback invoked by [`resolve_permission`](AgentAdapter::resolve_permission).
    pub(crate) on_resolve_permission: Option<ResolvePermissionFn>,
}

// -----
// Construction
// -----

impl InProcessAdapter {
    /// Create a new `InProcessAdapter` in the **Starting** state with no
    /// callbacks configured.
    ///
    /// Use the `with_*` builder methods to attach callbacks, then call
    /// [`start`](AgentAdapter::start) to transition to **Ready**.
    #[must_use]
    pub fn new(name: &str, agent_type: AgentType, capabilities: HashSet<AgentCapability>) -> Self {
        Self {
            info: AgentInfo {
                name: name.into(),
                version: env!("CARGO_PKG_VERSION").into(),
                capabilities,
                status: AgentStatus::Starting,
            },
            status: AgentStatus::Starting,
            agent_type,
            on_send_message: None,
            on_cancel: None,
            on_resolve_permission: None,
        }
    }

    // ----- Factory helpers for each concrete Agent type -----

    /// Create a new **Remote Claude** adapter with the standard capability set.
    #[must_use]
    pub fn new_claude() -> Self {
        let mut caps = HashSet::new();
        caps.insert(AgentCapability::Streaming);
        caps.insert(AgentCapability::ToolUse);
        caps.insert(AgentCapability::McpSupport);
        caps.insert(AgentCapability::Subtasks);
        caps.insert(AgentCapability::Permissions);
        Self::new("Remote Claude", AgentType::RemoteClaude, caps)
    }

    /// Create a new **Remote Roo** adapter with the standard capability set.
    #[must_use]
    pub fn new_roo() -> Self {
        let mut caps = HashSet::new();
        caps.insert(AgentCapability::Streaming);
        caps.insert(AgentCapability::ToolUse);
        caps.insert(AgentCapability::McpSupport);
        caps.insert(AgentCapability::Subtasks);
        caps.insert(AgentCapability::Permissions);
        Self::new("Remote Roo", AgentType::RemoteRoo, caps)
    }

    /// Create a new **Remote Codex** adapter with the standard capability set.
    ///
    /// Note: Codex does not advertise `McpSupport`.
    #[must_use]
    pub fn new_codex() -> Self {
        let mut caps = HashSet::new();
        caps.insert(AgentCapability::Streaming);
        caps.insert(AgentCapability::ToolUse);
        caps.insert(AgentCapability::Subtasks);
        caps.insert(AgentCapability::Permissions);
        Self::new("Remote Codex", AgentType::RemoteCodex, caps)
    }

    // ----- Builder methods -----

    /// Attach a send_message callback.
    #[must_use]
    pub fn with_send_message<F>(mut self, f: F) -> Self
    where
        F: Fn(&str, &str) -> anyhow::Result<Vec<UnifiedAgentEvent>> + Send + Sync + 'static,
    {
        self.on_send_message = Some(Box::new(f));
        self
    }

    /// Attach a cancel callback.
    #[must_use]
    pub fn with_cancel<F>(mut self, f: F) -> Self
    where
        F: Fn(&str) -> anyhow::Result<()> + Send + Sync + 'static,
    {
        self.on_cancel = Some(Box::new(f));
        self
    }

    /// Attach a resolve_permission callback.
    #[must_use]
    pub fn with_resolve_permission<F>(mut self, f: F) -> Self
    where
        F: Fn(&str, &str, PermissionDecision) -> anyhow::Result<()> + Send + Sync + 'static,
    {
        self.on_resolve_permission = Some(Box::new(f));
        self
    }
}

// -----
// AgentAdapter implementation
// -----

#[async_trait]
impl AgentAdapter for InProcessAdapter {
    async fn start(&mut self, _config: &AgentConfig) -> anyhow::Result<()> {
        info!("{} starting", self.info.name);
        self.status = AgentStatus::Ready;
        self.info.status = AgentStatus::Ready;
        Ok(())
    }

    async fn send_message(
        &mut self,
        session_id: &str,
        message: &str,
    ) -> anyhow::Result<mpsc::Receiver<UnifiedAgentEvent>> {
        let callback = self
            .on_send_message
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("send_message callback not configured"))?;

        let events = callback(session_id, message)?;

        let (tx, rx) = mpsc::channel(256);

        for event in events {
            if tx.send(event).await.is_err() {
                break;
            }
        }

        Ok(rx)
    }

    async fn cancel(&mut self, session_id: &str) -> anyhow::Result<()> {
        let callback = self
            .on_cancel
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("cancel callback not configured"))?;

        callback(session_id)
    }

    async fn resolve_permission(
        &mut self,
        session_id: &str,
        request_id: &str,
        decision: PermissionDecision,
    ) -> anyhow::Result<()> {
        let callback = self
            .on_resolve_permission
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("resolve_permission callback not configured"))?;

        callback(session_id, request_id, decision)
    }

    async fn stop(&mut self) -> anyhow::Result<()> {
        info!("{} stopping", self.info.name);
        self.status = AgentStatus::Stopped;
        self.info.status = AgentStatus::Stopped;
        Ok(())
    }

    fn is_alive(&self) -> bool {
        !matches!(self.status, AgentStatus::Stopped | AgentStatus::Error)
    }

    fn info(&self) -> &AgentInfo {
        &self.info
    }

    fn agent_type(&self) -> AgentType {
        self.agent_type
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config(agent_type: AgentType) -> AgentConfig {
        AgentConfig {
            agent_type,
            binary_path: None,
            args: vec![],
            env: vec![],
            working_dir: None,
            model: None,
            provider: None,
            api_key: None,
            base_url: None,
        }
    }

    #[test]
    fn claude_has_correct_type() {
        let adapter = InProcessAdapter::new_claude();
        assert_eq!(adapter.agent_type(), AgentType::RemoteClaude);
    }

    #[test]
    fn claude_info_has_all_capabilities() {
        let adapter = InProcessAdapter::new_claude();
        let info = adapter.info();
        assert_eq!(info.name, "Remote Claude");
        assert!(info.capabilities.contains(&AgentCapability::Streaming));
        assert!(info.capabilities.contains(&AgentCapability::ToolUse));
        assert!(info.capabilities.contains(&AgentCapability::McpSupport));
        assert!(info.capabilities.contains(&AgentCapability::Subtasks));
        assert!(info.capabilities.contains(&AgentCapability::Permissions));
        assert_eq!(info.capabilities.len(), 5);
    }

    #[test]
    fn roo_has_correct_type() {
        let adapter = InProcessAdapter::new_roo();
        assert_eq!(adapter.agent_type(), AgentType::RemoteRoo);
    }

    #[test]
    fn roo_info_has_all_capabilities() {
        let adapter = InProcessAdapter::new_roo();
        let info = adapter.info();
        assert_eq!(info.name, "Remote Roo");
        assert!(info.capabilities.contains(&AgentCapability::Streaming));
        assert!(info.capabilities.contains(&AgentCapability::ToolUse));
        assert!(info.capabilities.contains(&AgentCapability::McpSupport));
        assert!(info.capabilities.contains(&AgentCapability::Subtasks));
        assert!(info.capabilities.contains(&AgentCapability::Permissions));
        assert_eq!(info.capabilities.len(), 5);
    }

    #[test]
    fn codex_has_correct_type() {
        let adapter = InProcessAdapter::new_codex();
        assert_eq!(adapter.agent_type(), AgentType::RemoteCodex);
    }

    #[test]
    fn codex_info_has_correct_capabilities() {
        let adapter = InProcessAdapter::new_codex();
        let info = adapter.info();
        assert_eq!(info.name, "Remote Codex");
        assert!(info.capabilities.contains(&AgentCapability::Streaming));
        assert!(info.capabilities.contains(&AgentCapability::ToolUse));
        assert!(info.capabilities.contains(&AgentCapability::Subtasks));
        assert!(info.capabilities.contains(&AgentCapability::Permissions));
        assert!(!info.capabilities.contains(&AgentCapability::McpSupport));
        assert_eq!(info.capabilities.len(), 4);
    }

    #[tokio::test]
    async fn start_sets_status_to_ready() {
        let mut adapter = InProcessAdapter::new_claude();
        assert_eq!(adapter.status, AgentStatus::Starting);

        adapter
            .start(&test_config(AgentType::RemoteClaude))
            .await
            .unwrap();
        assert_eq!(adapter.status, AgentStatus::Ready);
        assert_eq!(adapter.info().status, AgentStatus::Ready);
    }

    #[tokio::test]
    async fn send_message_without_callback_returns_error() {
        let mut adapter = InProcessAdapter::new_claude();
        adapter
            .start(&test_config(AgentType::RemoteClaude))
            .await
            .unwrap();

        let result = adapter.send_message("sess-1", "hello").await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("send_message callback not configured"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn send_message_with_callback_returns_events() {
        let adapter = InProcessAdapter::new_claude().with_send_message(|_sid, msg| {
            Ok(vec![
                UnifiedAgentEvent::MessageDelta {
                    session_id: "sess-1".into(),
                    delta: msg.into(),
                },
                UnifiedAgentEvent::Completed {
                    session_id: "sess-1".into(),
                    result: crate::events::AgentResult {
                        response_text: msg.into(),
                        tool_calls: vec![],
                        usage: crate::events::UsageInfo::default(),
                        cost: None,
                    },
                },
            ])
        });

        let mut adapter = adapter;
        adapter
            .start(&test_config(AgentType::RemoteClaude))
            .await
            .unwrap();

        let mut rx = adapter.send_message("sess-1", "hello world").await.unwrap();

        let ev1 = rx.recv().await.expect("should receive first event");
        assert!(matches!(ev1, UnifiedAgentEvent::MessageDelta { .. }));

        let ev2 = rx.recv().await.expect("should receive second event");
        assert!(matches!(ev2, UnifiedAgentEvent::Completed { .. }));

        let ev3 = rx.recv().await;
        assert!(ev3.is_none(), "channel should be closed");
    }

    #[tokio::test]
    async fn cancel_without_callback_returns_error() {
        let mut adapter = InProcessAdapter::new_claude();
        adapter
            .start(&test_config(AgentType::RemoteClaude))
            .await
            .unwrap();

        let result = adapter.cancel("sess-1").await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("cancel callback not configured"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn cancel_with_callback_succeeds() {
        let adapter = InProcessAdapter::new_claude().with_cancel(|_sid| Ok(()));

        let mut adapter = adapter;
        adapter
            .start(&test_config(AgentType::RemoteClaude))
            .await
            .unwrap();

        let result = adapter.cancel("sess-1").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn stop_sets_status_to_stopped() {
        let mut adapter = InProcessAdapter::new_claude();
        adapter
            .start(&test_config(AgentType::RemoteClaude))
            .await
            .unwrap();
        assert_eq!(adapter.status, AgentStatus::Ready);

        adapter.stop().await.unwrap();
        assert_eq!(adapter.status, AgentStatus::Stopped);
        assert_eq!(adapter.info().status, AgentStatus::Stopped);
    }

    #[test]
    fn is_alive_reflects_status() {
        let mut adapter = InProcessAdapter::new_claude();
        assert!(adapter.is_alive());

        adapter.status = AgentStatus::Ready;
        assert!(adapter.is_alive());

        adapter.status = AgentStatus::Busy;
        assert!(adapter.is_alive());

        adapter.status = AgentStatus::Idle;
        assert!(adapter.is_alive());

        adapter.status = AgentStatus::Stopped;
        assert!(!adapter.is_alive());

        adapter.status = AgentStatus::Error;
        assert!(!adapter.is_alive());
    }

    #[test]
    fn builder_pattern_works() {
        let adapter = InProcessAdapter::new_claude()
            .with_send_message(|_sid, _msg| Ok(vec![]))
            .with_cancel(|_sid| Ok(()))
            .with_resolve_permission(|_sid, _rid, _dec| Ok(()));

        assert_eq!(adapter.agent_type(), AgentType::RemoteClaude);
        assert!(adapter.on_send_message.is_some());
        assert!(adapter.on_cancel.is_some());
        assert!(adapter.on_resolve_permission.is_some());
    }
}
