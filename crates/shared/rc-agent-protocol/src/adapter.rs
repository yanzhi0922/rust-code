//! Agent adapter trait definition.
//!
//! The [`AgentAdapter`] trait is the primary abstraction that all Agent
//! implementations must satisfy. Concrete adapters (Remote Code, Roo Code,
//! Codex) implement this trait to translate their native protocols into the
//! unified event model.

use async_trait::async_trait;

use crate::events::UnifiedAgentEvent;
use crate::permission::PermissionDecision;
use crate::types::{AgentConfig, AgentInfo, AgentType};

/// Trait that every Agent adapter must implement.
///
/// Adapters are responsible for:
/// - Starting and stopping the underlying Agent (process or in-memory).
/// - Sending user messages and returning a stream of unified events.
/// - Handling permission requests from the Agent.
/// - Reporting lifecycle state.
#[async_trait]
pub trait AgentAdapter: Send + Sync {
    /// Start the Agent with the given configuration.
    ///
    /// For sub-process Agents this spawns the binary; for in-process Agents
    /// it initializes the engine.
    async fn start(&mut self, config: &AgentConfig) -> anyhow::Result<()>;

    /// Send a user message and obtain a channel of unified events.
    ///
    /// The caller receives a [`tokio::sync::mpsc::Receiver`] that yields
    /// [`UnifiedAgentEvent`] values until the Agent finishes or errors.
    async fn send_message(
        &mut self,
        session_id: &str,
        message: &str,
    ) -> anyhow::Result<tokio::sync::mpsc::Receiver<UnifiedAgentEvent>>;

    /// Cancel the current in-flight request for the given session.
    async fn cancel(&mut self, session_id: &str) -> anyhow::Result<()>;

    /// Resolve a pending permission request from the Agent.
    async fn resolve_permission(
        &mut self,
        session_id: &str,
        request_id: &str,
        decision: PermissionDecision,
    ) -> anyhow::Result<()>;

    /// Gracefully stop the Agent.
    async fn stop(&mut self) -> anyhow::Result<()>;

    /// Returns `true` if the Agent is alive and responsive.
    fn is_alive(&self) -> bool;

    /// Returns static information about the Agent.
    fn info(&self) -> &AgentInfo;

    /// Returns the [`AgentType`] of this adapter.
    fn agent_type(&self) -> AgentType;
}
