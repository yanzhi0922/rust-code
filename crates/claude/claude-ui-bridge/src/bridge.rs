//! Bridge system for inter-process and remote communication.
//!
//! This module provides the bridge layer between the core engine and various
//! frontends (TUI, GUI, remote-control). It defines:
//!
//! - [`BridgeEvent`] — typed events flowing through the bridge
//! - [`BridgeTransport`] — async trait for transport implementations
//! - [`LocalBridge`] — in-process channel-based transport
//! - [`WebSocketBridge`] — remote WebSocket transport (configurable)
//! - [`BridgeClient`] — high-level consumer API
//!
//! # Architecture
//!
//! ```text
//! ┌──────────────┐     BridgeEvent      ┌──────────────────┐
//! │  Core Engine  │ ──────────────────▶ │  BridgeTransport  │
//! │  (producer)   │                     │  (Local / WS)     │
//! └──────────────┘                     └────────┬─────────┘
//!                                               │
//!                                      ┌────────▼─────────┐
//!                                      │  BridgeClient     │
//!                                      │  (consumer API)   │
//!                                      └──────────────────┘
//! ```

use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Result, ensure};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, mpsc};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// BridgeEvent — the universal event type for bridge communication
// ---------------------------------------------------------------------------

/// Events that flow through the bridge between producer and consumer.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BridgeEvent {
    /// A text message from user, assistant, or system.
    Message {
        /// Unique event identifier.
        id: String,
        /// Who produced this message.
        origin: BridgeOrigin,
        /// Message text content.
        text: String,
        /// Optional structured metadata (JSON).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        metadata: Option<serde_json::Value>,
    },
    /// A tool execution result.
    ToolResult {
        /// Unique event identifier.
        id: String,
        /// Tool call identifier from the provider.
        tool_call_id: String,
        /// Tool name that was executed.
        tool_name: String,
        /// Tool output content (may be truncated).
        output: String,
        /// Whether the tool execution resulted in an error.
        #[serde(default)]
        is_error: bool,
    },
    /// A permission request pending user approval.
    PermissionRequest {
        /// Unique request identifier.
        id: String,
        /// Tool name requesting permission.
        tool_name: String,
        /// Human-readable description of the action.
        description: String,
        /// Risk level of the requested action.
        #[serde(default)]
        risk_level: RiskLevel,
    },
    /// A permission decision rendered.
    PermissionDecision {
        /// Request identifier this decision relates to.
        request_id: String,
        /// Whether the action was allowed.
        allowed: bool,
        /// Optional explanation for the decision.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// A status update from the engine.
    StatusUpdate {
        /// Unique event identifier.
        id: String,
        /// Status category.
        status: BridgeStatus,
        /// Detailed status message.
        message: String,
        /// Optional progress percentage (0-100).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        percent: Option<u8>,
    },
    /// Connection lifecycle: connected.
    Connected {
        /// Protocol identifier.
        protocol: String,
    },
    /// Connection lifecycle: disconnected.
    Disconnected {
        /// Disconnect reason.
        reason: String,
    },
    /// An error occurred in the bridge itself.
    Error {
        /// Error message.
        message: String,
        /// Whether the error is recoverable.
        #[serde(default)]
        recoverable: bool,
    },
}

impl BridgeEvent {
    /// Create a new message event with auto-generated ID.
    #[must_use]
    pub fn message(origin: BridgeOrigin, text: impl Into<String>) -> Self {
        Self::Message {
            id: Uuid::new_v4().to_string(),
            origin,
            text: text.into(),
            metadata: None,
        }
    }

    /// Create a new tool result event with auto-generated ID.
    #[must_use]
    pub fn tool_result(
        tool_call_id: impl Into<String>,
        tool_name: impl Into<String>,
        output: impl Into<String>,
        is_error: bool,
    ) -> Self {
        Self::ToolResult {
            id: Uuid::new_v4().to_string(),
            tool_call_id: tool_call_id.into(),
            tool_name: tool_name.into(),
            output: output.into(),
            is_error,
        }
    }

    /// Create a new permission request event with auto-generated ID.
    #[must_use]
    pub fn permission_request(
        tool_name: impl Into<String>,
        description: impl Into<String>,
        risk_level: RiskLevel,
    ) -> Self {
        Self::PermissionRequest {
            id: Uuid::new_v4().to_string(),
            tool_name: tool_name.into(),
            description: description.into(),
            risk_level,
        }
    }

    /// Create a new status update event with auto-generated ID.
    #[must_use]
    pub fn status_update(status: BridgeStatus, message: impl Into<String>) -> Self {
        Self::StatusUpdate {
            id: Uuid::new_v4().to_string(),
            status,
            message: message.into(),
            percent: None,
        }
    }

    /// Return the event ID if present.
    #[must_use]
    pub fn event_id(&self) -> Option<&str> {
        match self {
            Self::Message { id, .. }
            | Self::ToolResult { id, .. }
            | Self::PermissionRequest { id, .. }
            | Self::StatusUpdate { id, .. } => Some(id.as_str()),
            Self::PermissionDecision { request_id, .. } => Some(request_id.as_str()),
            Self::Connected { .. } | Self::Disconnected { .. } | Self::Error { .. } => None,
        }
    }

    /// Check if this event represents an error.
    #[must_use]
    pub fn is_error(&self) -> bool {
        match self {
            Self::ToolResult { is_error, .. } => *is_error,
            Self::Error { .. } => true,
            _ => false,
        }
    }
}

/// Origin of a bridge message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BridgeOrigin {
    /// User input.
    User,
    /// Assistant response.
    Assistant,
    /// System-generated message.
    System,
    /// Tool execution output.
    Tool,
}

/// Risk level for permission requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    /// Low risk: read-only operations.
    #[default]
    Low,
    /// Medium risk: file modifications.
    Medium,
    /// High risk: shell commands, destructive operations.
    High,
}

/// Status category for bridge status updates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BridgeStatus {
    /// Engine is idle.
    Idle,
    /// Engine is processing a request.
    Processing,
    /// Engine is waiting for permission.
    WaitingPermission,
    /// Engine is compacting context.
    Compacting,
    /// Engine is executing a tool.
    ToolExecuting,
    /// Engine encountered an error.
    Error,
}

// ---------------------------------------------------------------------------
// BridgeTransport — async trait for transport implementations
// ---------------------------------------------------------------------------

/// Async transport trait for sending and receiving bridge events.
///
/// Implementations can be in-process (channels), WebSocket, or any other
/// transport mechanism.
#[async_trait]
pub trait BridgeTransport: Send + Sync {
    /// Send an event through the transport.
    ///
    /// # Errors
    /// Returns an error if the transport is disconnected or the send fails.
    async fn send(&self, event: BridgeEvent) -> Result<()>;

    /// Receive the next event from the transport.
    ///
    /// Returns `Ok(None)` if the transport is closed and no more events
    /// are available.
    ///
    /// # Errors
    /// Returns an error if the receive operation fails.
    async fn receive(&self) -> Result<Option<BridgeEvent>>;

    /// Establish the transport connection.
    ///
    /// # Errors
    /// Returns an error if the connection cannot be established.
    async fn connect(&self) -> Result<()>;

    /// Close the transport connection.
    ///
    /// # Errors
    /// Returns an error if the disconnection fails.
    async fn disconnect(&self) -> Result<()>;

    /// Check if the transport is currently connected.
    fn is_connected(&self) -> bool;
}

// ---------------------------------------------------------------------------
// LocalBridge — in-process channel-based transport
// ---------------------------------------------------------------------------

/// Default capacity for [`LocalBridge`] events.
///
/// Bounded so a runaway producer cannot grow the queue without limit when the
/// frontend stalls. Tuned to comfortably absorb token-by-token streaming for
/// long bursts; if production rate exceeds this for a sustained period, the
/// frontend is unhealthy and dropping events is preferable to OOM.
pub const LOCAL_BRIDGE_DEFAULT_CAPACITY: usize = 4096;

/// In-process bridge using Tokio MPSC channels.
///
/// Suitable for connecting the core engine to a frontend running in the
/// same process (e.g., TUI or embedded GUI).
///
/// The channel is **bounded**. Synchronous senders use `try_send` and surface
/// a clear error when the queue is saturated; async senders fall back to
/// `send().await` so producers naturally back off.
pub struct LocalBridge {
    sender: mpsc::Sender<BridgeEvent>,
    receiver: Mutex<mpsc::Receiver<BridgeEvent>>,
    connected: AtomicBool,
    capacity: usize,
}

impl LocalBridge {
    /// Create a new local bridge with the default bounded capacity.
    #[must_use]
    pub fn new() -> Self {
        Self::with_capacity(LOCAL_BRIDGE_DEFAULT_CAPACITY)
    }

    /// Create a new local bridge with the given bounded capacity.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        let (sender, receiver) = mpsc::channel(capacity);
        Self {
            sender,
            receiver: Mutex::new(receiver),
            connected: AtomicBool::new(false),
            capacity,
        }
    }

    /// Get a clonable sender handle that can be shared across tasks.
    #[must_use]
    pub fn sender(&self) -> mpsc::Sender<BridgeEvent> {
        self.sender.clone()
    }

    /// Configured channel capacity. Useful for diagnostics.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Send an event synchronously through the internal sender.
    ///
    /// Returns an error if the receiver has been dropped or the queue is at
    /// capacity. Callers that can tolerate dropping events should treat
    /// `Err` as backpressure, not a fatal condition.
    ///
    /// Unlike the async [`BridgeTransport::send`], this method does not
    /// check the connection state and returns immediately.
    ///
    /// # Errors
    /// Returns an error if the receiver has been dropped or the channel is
    /// at capacity.
    pub fn send_sync(&self, event: BridgeEvent) -> Result<()> {
        match self.sender.try_send(event) {
            Ok(()) => Ok(()),
            Err(mpsc::error::TrySendError::Full(event)) => Err(anyhow::anyhow!(
                "local bridge channel saturated (cap={}): dropping event {}",
                self.capacity,
                event.event_id().unwrap_or("unknown")
            )),
            Err(mpsc::error::TrySendError::Closed(event)) => Err(anyhow::anyhow!(
                "local bridge send failed: receiver dropped (event {})",
                event.event_id().unwrap_or("unknown")
            )),
        }
    }
}

impl Default for LocalBridge {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl BridgeTransport for LocalBridge {
    async fn send(&self, event: BridgeEvent) -> Result<()> {
        ensure!(
            self.connected.load(Ordering::Relaxed),
            "local bridge is not connected"
        );
        // `send().await` provides natural backpressure: when the queue is
        // full, the producer suspends until a slot frees up.
        self.sender.send(event).await.map_err(|e| {
            anyhow::anyhow!(
                "local bridge send failed: receiver dropped (event {})",
                e.0.event_id().unwrap_or("unknown")
            )
        })?;
        Ok(())
    }

    async fn receive(&self) -> Result<Option<BridgeEvent>> {
        let mut receiver = self.receiver.lock().await;
        Ok(receiver.try_recv().ok())
    }

    async fn connect(&self) -> Result<()> {
        self.connected.store(true, Ordering::Relaxed);
        Ok(())
    }

    async fn disconnect(&self) -> Result<()> {
        self.connected.store(false, Ordering::Relaxed);
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }
}

// ---------------------------------------------------------------------------
// WebSocketBridge — remote WebSocket transport (configurable)
// ---------------------------------------------------------------------------

/// Remote bridge using WebSocket transport.
///
/// This struct stores connection configuration and manages a buffer of
/// pending events. Actual WebSocket I/O is delegated to the transport
/// implementation layer.
pub struct WebSocketBridge {
    /// Remote endpoint URL (e.g., `ws://localhost:8080/bridge`).
    url: String,
    /// Connection state.
    connected: AtomicBool,
    /// Buffer for events pending transmission.
    pending: Mutex<Vec<BridgeEvent>>,
    /// Maximum pending buffer size before backpressure.
    max_pending: usize,
}

impl WebSocketBridge {
    /// Create a new WebSocket bridge targeting the given URL.
    #[must_use]
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            connected: AtomicBool::new(false),
            pending: Mutex::new(Vec::new()),
            max_pending: 1024,
        }
    }

    /// Create with a custom maximum pending buffer size.
    #[must_use]
    pub fn with_max_pending(url: impl Into<String>, max_pending: usize) -> Self {
        Self {
            url: url.into(),
            connected: AtomicBool::new(false),
            pending: Mutex::new(Vec::new()),
            max_pending,
        }
    }

    /// Return the configured endpoint URL.
    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Return the number of pending events in the buffer.
    pub async fn pending_count(&self) -> usize {
        self.pending.lock().await.len()
    }

    /// Drain all pending events from the buffer.
    pub async fn drain_pending(&self) -> Vec<BridgeEvent> {
        let mut pending = self.pending.lock().await;
        let drained: Vec<BridgeEvent> = pending.drain(..).collect();
        drained
    }
}

#[async_trait]
impl BridgeTransport for WebSocketBridge {
    async fn send(&self, event: BridgeEvent) -> Result<()> {
        if self.connected.load(Ordering::Relaxed) {
            // In a real implementation, this would serialize and send
            // over the WebSocket connection. For now, buffer the event.
            let mut pending = self.pending.lock().await;
            ensure!(
                pending.len() < self.max_pending,
                "websocket bridge pending buffer full (max: {})",
                self.max_pending
            );
            pending.push(event);
            Ok(())
        } else {
            // Buffer for later transmission when connected.
            let mut pending = self.pending.lock().await;
            ensure!(
                pending.len() < self.max_pending,
                "websocket bridge pending buffer full (max: {})",
                self.max_pending
            );
            pending.push(event);
            Ok(())
        }
    }

    async fn receive(&self) -> Result<Option<BridgeEvent>> {
        let mut pending = self.pending.lock().await;
        if pending.is_empty() {
            return Ok(None);
        }
        Ok(pending.drain(0..1).next())
    }

    async fn connect(&self) -> Result<()> {
        // In a real implementation, this would establish a WebSocket
        // connection to the configured URL.
        ensure!(!self.url.is_empty(), "websocket bridge URL cannot be empty");
        self.connected.store(true, Ordering::Relaxed);
        Ok(())
    }

    async fn disconnect(&self) -> Result<()> {
        self.connected.store(false, Ordering::Relaxed);
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }
}

// ---------------------------------------------------------------------------
// BridgeClient — high-level consumer API
// ---------------------------------------------------------------------------

/// High-level client for consuming bridge events.
///
/// Wraps a [`BridgeTransport`] and provides convenient methods for
/// sending typed events and receiving them.
pub struct BridgeClient<T: BridgeTransport> {
    transport: T,
}

impl<T: BridgeTransport> BridgeClient<T> {
    /// Create a new bridge client wrapping the given transport.
    #[must_use]
    pub fn new(transport: T) -> Self {
        Self { transport }
    }

    /// Connect the underlying transport.
    ///
    /// # Errors
    /// Returns an error if the connection fails.
    pub async fn connect(&self) -> Result<()> {
        self.transport.connect().await
    }

    /// Disconnect the underlying transport.
    ///
    /// # Errors
    /// Returns an error if the disconnection fails.
    pub async fn disconnect(&self) -> Result<()> {
        self.transport.disconnect().await
    }

    /// Check if the transport is connected.
    #[must_use]
    pub fn is_connected(&self) -> bool {
        self.transport.is_connected()
    }

    /// Send a user message through the bridge.
    ///
    /// # Errors
    /// Returns an error if the send fails.
    pub async fn send_message(&self, text: impl Into<String>) -> Result<()> {
        let event = BridgeEvent::message(BridgeOrigin::User, text);
        self.transport.send(event).await
    }

    /// Send an assistant message through the bridge.
    ///
    /// # Errors
    /// Returns an error if the send fails.
    pub async fn send_assistant_message(&self, text: impl Into<String>) -> Result<()> {
        let event = BridgeEvent::message(BridgeOrigin::Assistant, text);
        self.transport.send(event).await
    }

    /// Send a tool result through the bridge.
    ///
    /// # Errors
    /// Returns an error if the send fails.
    pub async fn send_tool_result(
        &self,
        tool_call_id: impl Into<String>,
        tool_name: impl Into<String>,
        output: impl Into<String>,
        is_error: bool,
    ) -> Result<()> {
        let event = BridgeEvent::tool_result(tool_call_id, tool_name, output, is_error);
        self.transport.send(event).await
    }

    /// Send a permission request through the bridge.
    ///
    /// # Errors
    /// Returns an error if the send fails.
    pub async fn request_permission(
        &self,
        tool_name: impl Into<String>,
        description: impl Into<String>,
        risk_level: RiskLevel,
    ) -> Result<()> {
        let event = BridgeEvent::permission_request(tool_name, description, risk_level);
        self.transport.send(event).await
    }

    /// Send a permission decision through the bridge.
    ///
    /// # Errors
    /// Returns an error if the send fails.
    pub async fn permission_decision(
        &self,
        request_id: impl Into<String>,
        allowed: bool,
        reason: Option<String>,
    ) -> Result<()> {
        let event = BridgeEvent::PermissionDecision {
            request_id: request_id.into(),
            allowed,
            reason,
        };
        self.transport.send(event).await
    }

    /// Send a status update through the bridge.
    ///
    /// # Errors
    /// Returns an error if the send fails.
    pub async fn send_status(
        &self,
        status: BridgeStatus,
        message: impl Into<String>,
    ) -> Result<()> {
        let event = BridgeEvent::status_update(status, message);
        self.transport.send(event).await
    }

    /// Receive the next event from the bridge.
    ///
    /// Returns `Ok(None)` if the transport is closed.
    ///
    /// # Errors
    /// Returns an error if the receive operation fails.
    pub async fn receive(&self) -> Result<Option<BridgeEvent>> {
        self.transport.receive().await
    }

    /// Collect all currently available events from the bridge.
    ///
    /// # Errors
    /// Returns an error if any receive operation fails.
    pub async fn collect_available(&self) -> Result<Vec<BridgeEvent>> {
        let mut events = Vec::new();
        loop {
            match self.transport.receive().await {
                Ok(Some(event)) => events.push(event),
                Ok(None) => break,
                Err(e) => {
                    if events.is_empty() {
                        return Err(e);
                    }
                    break;
                }
            }
        }
        Ok(events)
    }

    /// Consume the inner transport, returning it.
    #[must_use]
    pub fn into_transport(self) -> T {
        self.transport
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bridge_event_message_creation() {
        let event = BridgeEvent::message(BridgeOrigin::User, "hello world");
        match event {
            BridgeEvent::Message { origin, text, .. } => {
                assert_eq!(origin, BridgeOrigin::User);
                assert_eq!(text, "hello world");
            }
            other => panic!("expected Message, got {other:?}"),
        }
    }

    #[test]
    fn bridge_event_tool_result_creation() {
        let event = BridgeEvent::tool_result("tc-1", "bash", "ok", false);
        match event {
            BridgeEvent::ToolResult {
                tool_call_id,
                tool_name,
                output,
                is_error,
                ..
            } => {
                assert_eq!(tool_call_id, "tc-1");
                assert_eq!(tool_name, "bash");
                assert_eq!(output, "ok");
                assert!(!is_error);
            }
            other => panic!("expected ToolResult, got {other:?}"),
        }
    }

    #[test]
    fn bridge_event_permission_request_creation() {
        let event = BridgeEvent::permission_request("rm", "delete file", RiskLevel::High);
        match event {
            BridgeEvent::PermissionRequest {
                tool_name,
                description,
                risk_level,
                ..
            } => {
                assert_eq!(tool_name, "rm");
                assert_eq!(description, "delete file");
                assert_eq!(risk_level, RiskLevel::High);
            }
            other => panic!("expected PermissionRequest, got {other:?}"),
        }
    }

    #[test]
    fn bridge_event_status_update_creation() {
        let event = BridgeEvent::status_update(BridgeStatus::Processing, "thinking...");
        match event {
            BridgeEvent::StatusUpdate {
                status, message, ..
            } => {
                assert_eq!(status, BridgeStatus::Processing);
                assert_eq!(message, "thinking...");
            }
            other => panic!("expected StatusUpdate, got {other:?}"),
        }
    }

    #[test]
    fn bridge_event_serialization_roundtrip() {
        let events = vec![
            BridgeEvent::message(BridgeOrigin::Assistant, "response"),
            BridgeEvent::tool_result("tc-2", "read", "content", false),
            BridgeEvent::permission_request("write", "write file", RiskLevel::Medium),
            BridgeEvent::PermissionDecision {
                request_id: "req-1".to_owned(),
                allowed: true,
                reason: None,
            },
            BridgeEvent::status_update(BridgeStatus::Idle, "done"),
            BridgeEvent::Connected {
                protocol: "local".to_owned(),
            },
            BridgeEvent::Disconnected {
                reason: "shutdown".to_owned(),
            },
            BridgeEvent::Error {
                message: "oops".to_owned(),
                recoverable: true,
            },
        ];
        for original in events {
            let json = serde_json::to_string(&original).expect("serialization should succeed");
            let parsed: BridgeEvent =
                serde_json::from_str(&json).expect("deserialization should succeed");
            let json2 = serde_json::to_string(&parsed).expect("re-serialization should succeed");
            assert_eq!(json, json2, "roundtrip mismatch for {original:?}");
        }
    }

    #[test]
    fn bridge_event_is_error_check() {
        let tool_ok = BridgeEvent::tool_result("tc-1", "bash", "ok", false);
        assert!(!tool_ok.is_error());

        let tool_err = BridgeEvent::tool_result("tc-2", "bash", "fail", true);
        assert!(tool_err.is_error());

        let bridge_err = BridgeEvent::Error {
            message: "broken".to_owned(),
            recoverable: false,
        };
        assert!(bridge_err.is_error());

        let msg = BridgeEvent::message(BridgeOrigin::User, "hi");
        assert!(!msg.is_error());
    }

    #[test]
    fn bridge_event_id_extraction() {
        let msg = BridgeEvent::message(BridgeOrigin::User, "hi");
        assert!(msg.event_id().is_some());

        let connected = BridgeEvent::Connected {
            protocol: "ws".to_owned(),
        };
        assert!(connected.event_id().is_none());
    }

    #[tokio::test]
    async fn local_bridge_connect_disconnect() {
        let bridge = LocalBridge::new();
        assert!(!bridge.is_connected());

        bridge.connect().await.expect("connect should succeed");
        assert!(bridge.is_connected());

        bridge
            .disconnect()
            .await
            .expect("disconnect should succeed");
        assert!(!bridge.is_connected());
    }

    #[tokio::test]
    async fn local_bridge_send_receive() {
        let bridge = LocalBridge::new();
        bridge.connect().await.expect("connect should succeed");

        let event = BridgeEvent::message(BridgeOrigin::User, "hello");
        bridge
            .send(event.clone())
            .await
            .expect("send should succeed");

        let received = bridge
            .receive()
            .await
            .expect("receive should succeed")
            .expect("should have an event");
        assert!(matches!(received, BridgeEvent::Message { .. }));
    }

    #[tokio::test]
    async fn local_bridge_send_when_disconnected_fails() {
        let bridge = LocalBridge::new();
        let event = BridgeEvent::message(BridgeOrigin::User, "hello");
        let result = bridge.send(event).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn local_bridge_multiple_events() {
        let bridge = LocalBridge::new();
        bridge.connect().await.expect("connect should succeed");

        for i in 0..5 {
            let event = BridgeEvent::message(BridgeOrigin::User, format!("msg-{i}"));
            bridge.send(event).await.expect("send should succeed");
        }

        let mut count = 0;
        for _ in 0..5 {
            let received = bridge.receive().await.expect("receive should succeed");
            if received.is_some() {
                count += 1;
            }
        }
        assert_eq!(count, 5);
    }

    #[tokio::test]
    async fn local_bridge_sender_clone() {
        let bridge = LocalBridge::new();
        bridge.connect().await.expect("connect should succeed");

        let sender = bridge.sender();
        sender
            .send(BridgeEvent::message(BridgeOrigin::System, "from sender"))
            .await
            .expect("sender send should succeed");

        let received = bridge
            .receive()
            .await
            .expect("receive should succeed")
            .expect("should have event");
        match received {
            BridgeEvent::Message { text, .. } => assert_eq!(text, "from sender"),
            other => panic!("expected Message, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn websocket_bridge_creation() {
        let bridge = WebSocketBridge::new("ws://localhost:8080/bridge");
        assert_eq!(bridge.url(), "ws://localhost:8080/bridge");
        assert!(!bridge.is_connected());
    }

    #[tokio::test]
    async fn websocket_bridge_connect_disconnect() {
        let bridge = WebSocketBridge::new("ws://localhost:8080/bridge");
        bridge.connect().await.expect("connect should succeed");
        assert!(bridge.is_connected());

        bridge
            .disconnect()
            .await
            .expect("disconnect should succeed");
        assert!(!bridge.is_connected());
    }

    #[tokio::test]
    async fn websocket_bridge_send_buffers_events() {
        let bridge = WebSocketBridge::new("ws://localhost:8080/bridge");
        bridge
            .send(BridgeEvent::message(BridgeOrigin::User, "hi"))
            .await
            .expect("send should succeed");
        assert_eq!(bridge.pending_count().await, 1);

        let events = bridge.drain_pending().await;
        assert_eq!(events.len(), 1);
        assert_eq!(bridge.pending_count().await, 0);
    }

    #[tokio::test]
    async fn bridge_client_send_and_receive() {
        let bridge = LocalBridge::new();
        bridge.connect().await.expect("connect should succeed");

        let client = BridgeClient::new(bridge);
        client
            .send_message("hello")
            .await
            .expect("send_message should succeed");

        let received = client
            .receive()
            .await
            .expect("receive should succeed")
            .expect("should have event");
        match received {
            BridgeEvent::Message { origin, text, .. } => {
                assert_eq!(origin, BridgeOrigin::User);
                assert_eq!(text, "hello");
            }
            other => panic!("expected Message, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn bridge_client_tool_result_and_permission() {
        let bridge = LocalBridge::new();
        bridge.connect().await.expect("connect should succeed");

        let client = BridgeClient::new(bridge);
        client
            .send_tool_result("tc-1", "bash", "done", false)
            .await
            .expect("send_tool_result should succeed");
        client
            .request_permission("rm", "delete file", RiskLevel::High)
            .await
            .expect("request_permission should succeed");

        let events = client
            .collect_available()
            .await
            .expect("collect should succeed");
        assert_eq!(events.len(), 2);
        assert!(matches!(events[0], BridgeEvent::ToolResult { .. }));
        assert!(matches!(events[1], BridgeEvent::PermissionRequest { .. }));
    }

    #[test]
    fn risk_level_default_is_low() {
        assert_eq!(RiskLevel::default(), RiskLevel::Low);
    }

    #[test]
    fn bridge_status_variants() {
        let statuses = [
            BridgeStatus::Idle,
            BridgeStatus::Processing,
            BridgeStatus::WaitingPermission,
            BridgeStatus::Compacting,
            BridgeStatus::ToolExecuting,
            BridgeStatus::Error,
        ];
        assert_eq!(statuses.len(), 6);
    }

    #[test]
    fn bridge_origin_variants() {
        let origins = [
            BridgeOrigin::User,
            BridgeOrigin::Assistant,
            BridgeOrigin::System,
            BridgeOrigin::Tool,
        ];
        assert_eq!(origins.len(), 4);
    }
}
