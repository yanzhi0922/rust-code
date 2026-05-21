//! Multi-strategy remote transport layer for mobile-to-runner connections.
//!
//! Supports 5 connection strategies with security-first design:
//! 1. Direct WebSocket (mobile → runner)
//! 2. Server-relayed WebSocket (mobile → control plane → runner)
//! 3. Outbound polling / Anthropic mode (runner → server ← mobile)
//! 4. Hybrid (auto-switch between direct and relay)
//! 5. QUIC/HTTP3 (via quinn, with connection migration)

pub mod reconnect;
pub mod tls;

#[cfg(feature = "websocket")]
pub mod direct_ws;

#[cfg(feature = "websocket")]
pub mod relay_ws;

#[cfg(feature = "polling")]
pub mod outbound_poll;

#[cfg(feature = "hybrid")]
pub mod hybrid;

#[cfg(feature = "quic")]
pub mod quic_transport;

pub mod health;
pub mod offline_queue;
pub mod token;

#[cfg(feature = "e2e")]
pub mod e2e;

pub mod transport;

#[cfg(feature = "quic")]
pub use quic_transport::QuicTransport;
pub use reconnect::ReconnectPolicy;
pub use transport::{
    CommandAck, HealthStatus, RemoteTransport, TransportApprovalDecision, TransportCommand,
};

use serde::{Deserialize, Serialize};

/// Which connection strategy to use.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "strategy", rename_all = "snake_case")]
pub enum TransportStrategy {
    /// Strategy 1: Direct WebSocket to the runner machine.
    DirectWebSocket { runner_url: String },
    /// Strategy 2: Server-relayed via control plane.
    ServerRelay { control_plane_url: String },
    /// Strategy 3: Runner polls outbound; mobile connects to control plane only.
    OutboundPolling {
        control_plane_url: String,
        /// How often the runner polls for commands (ms).
        #[serde(default = "default_poll_interval_ms")]
        poll_interval_ms: u32,
    },
    /// Strategy 4: Try direct first, fall back to relay.
    Hybrid {
        runner_url: String,
        control_plane_url: String,
    },
    /// Strategy 5: QUIC transport with connection migration.
    Quic {
        server_url: String,
        /// Optional SHA-256 fingerprint of the server certificate.
        #[serde(default)]
        server_cert_fingerprint: Option<String>,
    },
}

fn default_poll_interval_ms() -> u32 {
    5000
}

/// Connection state machine.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ConnectionState {
    Disconnected,
    Probing,
    Connecting,
    Open {
        /// Which strategy is currently active.
        active_strategy: String,
        /// Round-trip latency in ms (latest measurement).
        latency_ms: u32,
    },
    Reconnecting {
        attempt: u32,
        next_retry_ms: u32,
    },
    Closed,
    Error(String),
}

/// Configuration for a transport connection attempt.
#[derive(Debug, Clone)]
pub struct TransportConfig {
    pub strategy: TransportStrategy,
    pub auth_token: String,
    pub session_id: String,
    /// Resume streaming after this sequence number.
    pub after_sequence: u64,
    pub tls: TlsConfig,
    pub reconnect: ReconnectPolicy,
}

/// TLS security configuration.
#[derive(Debug, Clone)]
pub struct TlsConfig {
    /// Accept self-signed certificates (for LAN direct connections).
    pub accept_self_signed: bool,
    /// SHA-256 fingerprints of trusted certificates (Certificate Transparency).
    pub cert_fingerprints: Vec<String>,
    /// Reject plain HTTP connections in production.
    pub enforce_https: bool,
}

impl Default for TlsConfig {
    fn default() -> Self {
        Self {
            accept_self_signed: false,
            cert_fingerprints: Vec::new(),
            enforce_https: true,
        }
    }
}

/// Transport performance metrics.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TransportMetrics {
    pub latency_ms: u32,
    pub events_received: u64,
    pub events_dropped: u64,
    pub reconnect_count: u32,
    pub strategy_switches: u32,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    /// Unix timestamp of last successful event.
    pub last_event_at: Option<i64>,
}

/// A received transport event (generic JSON payload).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransportEvent {
    pub sequence: u64,
    pub payload: serde_json::Value,
}

/// Health probe result for a specific endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EndpointHealth {
    pub url: String,
    pub reachable: bool,
    pub latency_ms: Option<u32>,
    pub auth_valid: bool,
    pub error: Option<String>,
}
