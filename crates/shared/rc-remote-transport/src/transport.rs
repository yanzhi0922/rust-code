//! Core transport trait shared by all 5 strategies.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{ConnectionState, EndpointHealth, TransportConfig, TransportMetrics};

/// A command sent from the mobile app to the runner/control plane.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TransportCommand {
    SendPrompt {
        content: String,
    },
    Interrupt,
    RespondToApproval {
        approval_id: String,
        decision: TransportApprovalDecision,
        note: Option<String>,
    },
}

/// Typed approval decision shared by every remote transport.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportApprovalDecision {
    Approved,
    Denied,
    Cancelled,
}

/// Acknowledgement for a command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandAck {
    pub accepted: bool,
    pub message: String,
}

/// Health probe result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthStatus {
    pub endpoints: Vec<EndpointHealth>,
    pub recommended_strategy: Option<String>,
}

/// The core transport trait — every strategy implements this.
#[async_trait]
pub trait RemoteTransport: Send + Sync {
    /// Establish a connection using the given config.
    async fn connect(&mut self, config: TransportConfig) -> anyhow::Result<()>;

    /// Send a command to the runner/control plane.
    async fn send_command(&self, command: TransportCommand) -> anyhow::Result<CommandAck>;

    /// Probe the health of reachable endpoints.
    async fn health_probe(&self) -> HealthStatus;

    /// Gracefully disconnect.
    async fn disconnect(&mut self) -> anyhow::Result<()>;

    /// Current connection state.
    fn state(&self) -> ConnectionState;

    /// Which strategy is currently active.
    fn active_strategy(&self) -> &str;

    /// Performance metrics.
    fn metrics(&self) -> TransportMetrics;
}
