//! MCP lifecycle events, hooks, and connection state machine.
//!
//! Defines events emitted during the MCP server connection lifecycle, a
//! trait for observing these events, and a full connection state machine
//! (`McpConnectionLifecycle`) that manages state transitions with automatic
//! reconnect logic and connection timeout handling.

use std::collections::HashMap;
use std::fmt;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// MCP server surface whose advertised list can change at runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpListChangedSurface {
    /// `notifications/tools/list_changed`
    Tools,
    /// `notifications/prompts/list_changed`
    Prompts,
    /// `notifications/resources/list_changed`
    Resources,
}

impl McpListChangedSurface {
    /// Return the JSON-RPC notification method for this surface.
    #[must_use]
    pub const fn notification_method(self) -> &'static str {
        match self {
            Self::Tools => "notifications/tools/list_changed",
            Self::Prompts => "notifications/prompts/list_changed",
            Self::Resources => "notifications/resources/list_changed",
        }
    }
}

impl fmt::Display for McpListChangedSurface {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Tools => write!(f, "tools"),
            Self::Prompts => write!(f, "prompts"),
            Self::Resources => write!(f, "resources"),
        }
    }
}

// ── Lifecycle event ──────────────────────────────────────────────────────────

/// MCP lifecycle event.
#[derive(Debug, Clone)]
pub enum McpLifecycleEvent {
    /// A connection attempt is starting.
    Connecting {
        /// Server name.
        name: String,
    },
    /// The server has been successfully connected.
    Connected {
        /// Server name.
        name: String,
    },
    /// The server has been disconnected.
    Disconnected {
        /// Server name.
        name: String,
        /// Reason for disconnection.
        reason: DisconnectReason,
    },
    /// A reconnection attempt is in progress.
    Reconnecting {
        /// Server name.
        name: String,
        /// Current attempt number (1-based).
        attempt: u32,
        /// Maximum number of attempts.
        max_attempts: u32,
    },
    /// The server has been successfully reconnected.
    Reconnected {
        /// Server name.
        name: String,
    },
    /// The connection has permanently failed.
    Failed {
        /// Server name.
        name: String,
        /// Error message.
        error: String,
    },
    /// The server requires authentication.
    NeedsAuth {
        /// Server name.
        name: String,
    },
    /// The server has been disabled.
    Disabled {
        /// Server name.
        name: String,
    },
    /// The server has been enabled.
    Enabled {
        /// Server name.
        name: String,
    },
    /// Tools have been discovered for a server.
    ToolsDiscovered {
        /// Server name.
        name: String,
        /// Number of tools discovered.
        count: usize,
    },
    /// Resources have been discovered for a server.
    ResourcesDiscovered {
        /// Server name.
        name: String,
        /// Number of resources discovered.
        count: usize,
    },
    /// A connected server reported `notifications/*/list_changed`.
    ListChanged {
        /// Server name.
        name: String,
        /// Changed MCP surface.
        surface: McpListChangedSurface,
    },
    /// A changed MCP surface was refreshed successfully.
    ListRefreshed {
        /// Server name.
        name: String,
        /// Changed MCP surface.
        surface: McpListChangedSurface,
        /// Number of entries now visible for the changed surface.
        count: usize,
    },
}

impl McpLifecycleEvent {
    /// Return the server name associated with this event.
    #[must_use]
    pub fn server_name(&self) -> &str {
        match self {
            Self::Connecting { name }
            | Self::Connected { name }
            | Self::Disconnected { name, .. }
            | Self::Reconnecting { name, .. }
            | Self::Reconnected { name }
            | Self::Failed { name, .. }
            | Self::NeedsAuth { name }
            | Self::Disabled { name }
            | Self::Enabled { name }
            | Self::ToolsDiscovered { name, .. }
            | Self::ResourcesDiscovered { name, .. }
            | Self::ListChanged { name, .. }
            | Self::ListRefreshed { name, .. } => name,
        }
    }
}

// ── Disconnect reason ─────────────────────────────────────────────────────────

/// Reason for a server disconnection.
#[derive(Debug, Clone)]
pub enum DisconnectReason {
    /// The connection was closed normally.
    Closed,
    /// An error caused the disconnection.
    Error(String),
    /// The session expired (e.g. token timeout).
    SessionExpired,
    /// The disconnection was initiated manually.
    Manual,
}

// ── Lifecycle hook trait ──────────────────────────────────────────────────────

/// Lifecycle hook trait for observing MCP connection events.
///
/// Implementations can be registered with [`crate::manager::McpConnectionManager`]
/// to receive notifications about connection state changes.
pub trait McpLifecycleHook: Send + Sync {
    /// Called when a lifecycle event occurs.
    fn on_event(&self, event: &McpLifecycleEvent);
}

// ── Connection state ──────────────────────────────────────────────────────────

/// Connection state for an MCP server managed by the lifecycle state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpConnectionState {
    /// No active connection.
    Disconnected,
    /// Connection attempt in progress.
    Connecting,
    /// Successfully connected and initialized.
    Connected,
    /// Connection permanently failed.
    Failed,
    /// Server requires authentication before connecting.
    NeedsAuth,
    /// Reconnection attempt in progress.
    Reconnecting,
}

impl McpConnectionState {
    /// Returns `true` if the connection is active (Connected).
    #[must_use]
    pub fn is_connected(self) -> bool {
        matches!(self, Self::Connected)
    }

    /// Returns `true` if the connection is in a transient state (Connecting or Reconnecting).
    #[must_use]
    pub fn is_transient(self) -> bool {
        matches!(self, Self::Connecting | Self::Reconnecting)
    }

    /// Returns `true` if the connection is in a terminal failure state.
    #[must_use]
    pub fn is_failed(self) -> bool {
        matches!(self, Self::Failed)
    }

    /// Returns `true` if the connection requires authentication.
    #[must_use]
    pub fn needs_auth(self) -> bool {
        matches!(self, Self::NeedsAuth)
    }
}

impl fmt::Display for McpConnectionState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Disconnected => write!(f, "disconnected"),
            Self::Connecting => write!(f, "connecting"),
            Self::Connected => write!(f, "connected"),
            Self::Failed => write!(f, "failed"),
            Self::NeedsAuth => write!(f, "needs_auth"),
            Self::Reconnecting => write!(f, "reconnecting"),
        }
    }
}

// ── State transition error ────────────────────────────────────────────────────

/// Error returned when an invalid state transition is attempted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateTransitionError {
    /// The current state.
    pub from: McpConnectionState,
    /// The target state that was rejected.
    pub to: McpConnectionState,
    /// Description of why the transition is invalid.
    pub reason: String,
}

impl fmt::Display for StateTransitionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid state transition from {} to {}: {}",
            self.from, self.to, self.reason
        )
    }
}

impl std::error::Error for StateTransitionError {}

// ── Per-server lifecycle entry ────────────────────────────────────────────────

/// Internal tracking data for a single server's lifecycle.
#[derive(Debug, Clone)]
struct ServerLifecycle {
    /// Current connection state.
    state: McpConnectionState,
    /// Number of reconnect attempts made (reset on success).
    reconnect_attempts: u32,
    /// Maximum reconnect attempts before giving up.
    max_reconnect_attempts: u32,
    /// Initial backoff duration for reconnect.
    initial_backoff: Duration,
    /// Maximum backoff duration.
    max_backoff: Duration,
    /// Connection timeout.
    connect_timeout: Duration,
    /// When the current state was entered.
    state_entered_at: Instant,
    /// When the connection attempt was started (for timeout tracking).
    connect_started_at: Option<Instant>,
    /// Last error message (if any).
    last_error: Option<String>,
}

impl ServerLifecycle {
    fn new(
        max_reconnect_attempts: u32,
        initial_backoff: Duration,
        max_backoff: Duration,
        connect_timeout: Duration,
    ) -> Self {
        Self {
            state: McpConnectionState::Disconnected,
            reconnect_attempts: 0,
            max_reconnect_attempts,
            initial_backoff,
            max_backoff,
            connect_timeout,
            state_entered_at: Instant::now(),
            connect_started_at: None,
            last_error: None,
        }
    }

    /// Compute the backoff duration for the current reconnect attempt.
    fn current_backoff(&self) -> Duration {
        if self.reconnect_attempts == 0 {
            return Duration::ZERO;
        }
        let exponent = self.reconnect_attempts.saturating_sub(1);
        let multiplier = 2_u64.saturating_pow(exponent);
        let backoff_ms = self
            .initial_backoff
            .as_millis()
            .saturating_mul(multiplier as u128);
        let max_ms = self.max_backoff.as_millis();
        Duration::from_millis(backoff_ms.min(max_ms) as u64)
    }

    /// Check if the connection attempt has timed out.
    fn is_timed_out(&self) -> bool {
        match self.connect_started_at {
            Some(started) => started.elapsed() >= self.connect_timeout,
            None => false,
        }
    }
}

// ── Connection lifecycle manager ──────────────────────────────────────────────

/// Default maximum reconnect attempts.
const DEFAULT_MAX_RECONNECT_ATTEMPTS: u32 = 5;
/// Default initial backoff.
const DEFAULT_INITIAL_BACKOFF: Duration = Duration::from_secs(1);
/// Default maximum backoff.
const DEFAULT_MAX_BACKOFF: Duration = Duration::from_secs(30);
/// Default connection timeout.
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// MCP connection lifecycle state machine.
///
/// Manages the state transitions for multiple MCP server connections,
/// including automatic reconnect with exponential backoff and connection
/// timeout detection.
///
/// # State machine
///
/// ```text
/// Disconnected ──► Connecting ──► Connected
///      ▲               │               │
///      │               ▼               │
///      │            Failed             │
///      │               │               ▼
///      │          Reconnecting ◄── Disconnected (error)
///      │               │
///      │           NeedsAuth
///      │               │
///      └───────────────┘
/// ```
#[derive(Debug)]
pub struct McpConnectionLifecycle {
    servers: HashMap<String, ServerLifecycle>,
    max_reconnect_attempts: u32,
    initial_backoff: Duration,
    max_backoff: Duration,
    connect_timeout: Duration,
}

impl McpConnectionLifecycle {
    /// Create a new lifecycle manager with default settings.
    #[must_use]
    pub fn new() -> Self {
        Self {
            servers: HashMap::new(),
            max_reconnect_attempts: DEFAULT_MAX_RECONNECT_ATTEMPTS,
            initial_backoff: DEFAULT_INITIAL_BACKOFF,
            max_backoff: DEFAULT_MAX_BACKOFF,
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
        }
    }

    /// Create a new lifecycle manager with custom settings.
    #[must_use]
    pub fn with_settings(
        max_reconnect_attempts: u32,
        initial_backoff: Duration,
        max_backoff: Duration,
        connect_timeout: Duration,
    ) -> Self {
        Self {
            servers: HashMap::new(),
            max_reconnect_attempts,
            initial_backoff,
            max_backoff,
            connect_timeout,
        }
    }

    /// Register a server for lifecycle management.
    ///
    /// The server starts in the `Disconnected` state.
    pub fn register_server(&mut self, name: &str) {
        self.servers.insert(
            name.to_owned(),
            ServerLifecycle::new(
                self.max_reconnect_attempts,
                self.initial_backoff,
                self.max_backoff,
                self.connect_timeout,
            ),
        );
    }

    /// Remove a server from lifecycle management.
    pub fn unregister_server(&mut self, name: &str) {
        self.servers.remove(name);
    }

    /// Get the current state of a server.
    #[must_use]
    pub fn state(&self, name: &str) -> Option<McpConnectionState> {
        self.servers.get(name).map(|s| s.state)
    }

    /// Get the last error for a server.
    #[must_use]
    pub fn last_error(&self, name: &str) -> Option<&str> {
        self.servers.get(name).and_then(|s| s.last_error.as_deref())
    }

    /// Get the reconnect attempt count for a server.
    #[must_use]
    pub fn reconnect_attempts(&self, name: &str) -> u32 {
        self.servers
            .get(name)
            .map(|s| s.reconnect_attempts)
            .unwrap_or(0)
    }

    /// Get the current backoff duration for a server.
    #[must_use]
    pub fn current_backoff(&self, name: &str) -> Option<Duration> {
        self.servers.get(name).map(|s| s.current_backoff())
    }

    /// Check if a server's connection attempt has timed out.
    #[must_use]
    pub fn is_timed_out(&self, name: &str) -> bool {
        self.servers.get(name).is_some_and(|s| s.is_timed_out())
    }

    /// Transition a server to the `Connecting` state.
    ///
    /// Valid from: `Disconnected`, `Failed`, `NeedsAuth`.
    ///
    /// Returns the generated lifecycle event, or an error if the transition
    /// is invalid.
    pub fn start_connecting(
        &mut self,
        name: &str,
    ) -> Result<McpLifecycleEvent, StateTransitionError> {
        let entry = self
            .servers
            .get_mut(name)
            .ok_or_else(|| StateTransitionError {
                from: McpConnectionState::Disconnected,
                to: McpConnectionState::Connecting,
                reason: format!("server '{name}' not registered"),
            })?;

        match entry.state {
            McpConnectionState::Disconnected
            | McpConnectionState::Failed
            | McpConnectionState::NeedsAuth => {
                entry.state = McpConnectionState::Connecting;
                entry.state_entered_at = Instant::now();
                entry.connect_started_at = Some(Instant::now());
                entry.last_error = None;
                Ok(McpLifecycleEvent::Connecting {
                    name: name.to_owned(),
                })
            }
            McpConnectionState::Connected => Err(StateTransitionError {
                from: entry.state,
                to: McpConnectionState::Connecting,
                reason: "already connected".to_owned(),
            }),
            McpConnectionState::Connecting => Err(StateTransitionError {
                from: entry.state,
                to: McpConnectionState::Connecting,
                reason: "already connecting".to_owned(),
            }),
            McpConnectionState::Reconnecting => Err(StateTransitionError {
                from: entry.state,
                to: McpConnectionState::Connecting,
                reason: "currently reconnecting; cancel reconnect first".to_owned(),
            }),
        }
    }

    /// Transition a server to the `Connected` state.
    ///
    /// Valid from: `Connecting`, `Reconnecting`.
    pub fn mark_connected(
        &mut self,
        name: &str,
    ) -> Result<McpLifecycleEvent, StateTransitionError> {
        let entry = self
            .servers
            .get_mut(name)
            .ok_or_else(|| StateTransitionError {
                from: McpConnectionState::Disconnected,
                to: McpConnectionState::Connected,
                reason: format!("server '{name}' not registered"),
            })?;

        let was_reconnecting = entry.state == McpConnectionState::Reconnecting;

        match entry.state {
            McpConnectionState::Connecting | McpConnectionState::Reconnecting => {
                entry.state = McpConnectionState::Connected;
                entry.state_entered_at = Instant::now();
                entry.connect_started_at = None;
                entry.reconnect_attempts = 0;
                entry.last_error = None;

                if was_reconnecting {
                    Ok(McpLifecycleEvent::Reconnected {
                        name: name.to_owned(),
                    })
                } else {
                    Ok(McpLifecycleEvent::Connected {
                        name: name.to_owned(),
                    })
                }
            }
            McpConnectionState::Connected => Err(StateTransitionError {
                from: entry.state,
                to: McpConnectionState::Connected,
                reason: "already connected".to_owned(),
            }),
            other => Err(StateTransitionError {
                from: other,
                to: McpConnectionState::Connected,
                reason: "must be in Connecting or Reconnecting state".to_owned(),
            }),
        }
    }

    /// Transition a server to the `Failed` state.
    ///
    /// Valid from: `Connecting`, `Reconnecting`.
    /// If reconnect attempts remain, transitions to `Reconnecting` instead.
    pub fn mark_failed(
        &mut self,
        name: &str,
        error: &str,
    ) -> Result<McpLifecycleEvent, StateTransitionError> {
        let entry = self
            .servers
            .get_mut(name)
            .ok_or_else(|| StateTransitionError {
                from: McpConnectionState::Disconnected,
                to: McpConnectionState::Failed,
                reason: format!("server '{name}' not registered"),
            })?;

        match entry.state {
            McpConnectionState::Connecting | McpConnectionState::Reconnecting => {
                entry.reconnect_attempts += 1;
                entry.last_error = Some(error.to_owned());
                entry.connect_started_at = None;

                if entry.reconnect_attempts <= entry.max_reconnect_attempts {
                    let _backoff = entry.current_backoff();
                    entry.state = McpConnectionState::Reconnecting;
                    entry.state_entered_at = Instant::now();
                    Ok(McpLifecycleEvent::Reconnecting {
                        name: name.to_owned(),
                        attempt: entry.reconnect_attempts,
                        max_attempts: entry.max_reconnect_attempts,
                    })
                } else {
                    entry.state = McpConnectionState::Failed;
                    entry.state_entered_at = Instant::now();
                    Ok(McpLifecycleEvent::Failed {
                        name: name.to_owned(),
                        error: error.to_owned(),
                    })
                }
            }
            other => Err(StateTransitionError {
                from: other,
                to: McpConnectionState::Failed,
                reason: "must be in Connecting or Reconnecting state".to_owned(),
            }),
        }
    }

    /// Transition a server to the `Disconnected` state.
    ///
    /// Valid from any state. Resets reconnect attempts.
    pub fn disconnect(
        &mut self,
        name: &str,
        reason: DisconnectReason,
    ) -> Result<McpLifecycleEvent, StateTransitionError> {
        let entry = self
            .servers
            .get_mut(name)
            .ok_or_else(|| StateTransitionError {
                from: McpConnectionState::Disconnected,
                to: McpConnectionState::Disconnected,
                reason: format!("server '{name}' not registered"),
            })?;

        let prev_state = entry.state;
        entry.state = McpConnectionState::Disconnected;
        entry.state_entered_at = Instant::now();
        entry.connect_started_at = None;
        entry.reconnect_attempts = 0;

        if prev_state == McpConnectionState::Connected {
            Ok(McpLifecycleEvent::Disconnected {
                name: name.to_owned(),
                reason,
            })
        } else {
            // Silently transition; no event for non-connected → disconnected.
            Ok(McpLifecycleEvent::Disconnected {
                name: name.to_owned(),
                reason,
            })
        }
    }

    /// Transition a server to the `NeedsAuth` state.
    ///
    /// Valid from: `Connecting`, `Reconnecting`, `Failed`.
    pub fn mark_needs_auth(
        &mut self,
        name: &str,
    ) -> Result<McpLifecycleEvent, StateTransitionError> {
        let entry = self
            .servers
            .get_mut(name)
            .ok_or_else(|| StateTransitionError {
                from: McpConnectionState::Disconnected,
                to: McpConnectionState::NeedsAuth,
                reason: format!("server '{name}' not registered"),
            })?;

        match entry.state {
            McpConnectionState::Connecting
            | McpConnectionState::Reconnecting
            | McpConnectionState::Failed => {
                entry.state = McpConnectionState::NeedsAuth;
                entry.state_entered_at = Instant::now();
                entry.connect_started_at = None;
                Ok(McpLifecycleEvent::NeedsAuth {
                    name: name.to_owned(),
                })
            }
            other => Err(StateTransitionError {
                from: other,
                to: McpConnectionState::NeedsAuth,
                reason: "must be in Connecting, Reconnecting, or Failed state".to_owned(),
            }),
        }
    }

    /// Initiate a reconnect for a server.
    ///
    /// Valid from: `Disconnected`, `Failed`.
    /// Transitions to `Reconnecting` and returns the backoff duration.
    pub fn start_reconnect(
        &mut self,
        name: &str,
    ) -> Result<(McpLifecycleEvent, Duration), StateTransitionError> {
        let entry = self
            .servers
            .get_mut(name)
            .ok_or_else(|| StateTransitionError {
                from: McpConnectionState::Disconnected,
                to: McpConnectionState::Reconnecting,
                reason: format!("server '{name}' not registered"),
            })?;

        match entry.state {
            McpConnectionState::Disconnected | McpConnectionState::Failed => {
                entry.reconnect_attempts += 1;

                if entry.reconnect_attempts > entry.max_reconnect_attempts {
                    entry.state = McpConnectionState::Failed;
                    entry.state_entered_at = Instant::now();
                    return Ok((
                        McpLifecycleEvent::Failed {
                            name: name.to_owned(),
                            error: "max reconnect attempts exceeded".to_owned(),
                        },
                        Duration::ZERO,
                    ));
                }

                let backoff = entry.current_backoff();
                entry.state = McpConnectionState::Reconnecting;
                entry.state_entered_at = Instant::now();
                entry.connect_started_at = Some(Instant::now());
                Ok((
                    McpLifecycleEvent::Reconnecting {
                        name: name.to_owned(),
                        attempt: entry.reconnect_attempts,
                        max_attempts: entry.max_reconnect_attempts,
                    },
                    backoff,
                ))
            }
            other => Err(StateTransitionError {
                from: other,
                to: McpConnectionState::Reconnecting,
                reason: "must be in Disconnected or Failed state".to_owned(),
            }),
        }
    }

    /// Force-reset a server to `Disconnected` state regardless of current state.
    pub fn force_reset(&mut self, name: &str) {
        if let Some(entry) = self.servers.get_mut(name) {
            entry.state = McpConnectionState::Disconnected;
            entry.state_entered_at = Instant::now();
            entry.connect_started_at = None;
            entry.reconnect_attempts = 0;
            entry.last_error = None;
        }
    }

    /// Return the number of registered servers.
    #[must_use]
    pub fn server_count(&self) -> usize {
        self.servers.len()
    }

    /// Return the number of servers in a given state.
    #[must_use]
    pub fn count_in_state(&self, state: McpConnectionState) -> usize {
        self.servers.values().filter(|s| s.state == state).count()
    }

    /// Return all server names in a given state.
    #[must_use]
    pub fn servers_in_state(&self, state: McpConnectionState) -> Vec<&str> {
        self.servers
            .iter()
            .filter(|(_, s)| s.state == state)
            .map(|(name, _)| name.as_str())
            .collect()
    }

    /// Check for all servers that have timed out during connection.
    #[must_use]
    pub fn timed_out_servers(&self) -> Vec<&str> {
        self.servers
            .iter()
            .filter(|(_, s)| s.is_timed_out())
            .map(|(name, _)| name.as_str())
            .collect()
    }

    /// Check if a server is registered.
    #[must_use]
    pub fn is_registered(&self, name: &str) -> bool {
        self.servers.contains_key(name)
    }
}

impl Default for McpConnectionLifecycle {
    fn default() -> Self {
        Self::new()
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── McpLifecycleEvent tests ───────────────────────────────────────────

    #[test]
    fn event_server_name_connecting() {
        let event = McpLifecycleEvent::Connecting {
            name: "test-server".to_owned(),
        };
        assert_eq!(event.server_name(), "test-server");
    }

    #[test]
    fn event_server_name_connected() {
        let event = McpLifecycleEvent::Connected {
            name: "my-server".to_owned(),
        };
        assert_eq!(event.server_name(), "my-server");
    }

    #[test]
    fn event_server_name_disconnected() {
        let event = McpLifecycleEvent::Disconnected {
            name: "remote".to_owned(),
            reason: DisconnectReason::Closed,
        };
        assert_eq!(event.server_name(), "remote");
    }

    #[test]
    fn event_server_name_reconnecting() {
        let event = McpLifecycleEvent::Reconnecting {
            name: "retry-srv".to_owned(),
            attempt: 2,
            max_attempts: 5,
        };
        assert_eq!(event.server_name(), "retry-srv");
    }

    #[test]
    fn event_server_name_failed() {
        let event = McpLifecycleEvent::Failed {
            name: "bad".to_owned(),
            error: "timeout".to_owned(),
        };
        assert_eq!(event.server_name(), "bad");
    }

    #[test]
    fn event_server_name_tools_discovered() {
        let event = McpLifecycleEvent::ToolsDiscovered {
            name: "tools-srv".to_owned(),
            count: 10,
        };
        assert_eq!(event.server_name(), "tools-srv");
    }

    #[test]
    fn disconnect_reason_variants() {
        let reasons = [
            DisconnectReason::Closed,
            DisconnectReason::Error("connection reset".to_owned()),
            DisconnectReason::SessionExpired,
            DisconnectReason::Manual,
        ];
        assert_eq!(reasons.len(), 4);
    }

    /// A no-op lifecycle hook for testing the trait object.
    struct NullHook;
    impl McpLifecycleHook for NullHook {
        fn on_event(&self, _event: &McpLifecycleEvent) {}
    }

    #[test]
    fn lifecycle_hook_trait_object() {
        let hook: Box<dyn McpLifecycleHook> = Box::new(NullHook);
        let event = McpLifecycleEvent::Connected {
            name: "test".to_owned(),
        };
        hook.on_event(&event);
    }

    // ── McpConnectionState tests ──────────────────────────────────────────

    #[test]
    fn connection_state_is_connected() {
        assert!(McpConnectionState::Connected.is_connected());
        assert!(!McpConnectionState::Disconnected.is_connected());
        assert!(!McpConnectionState::Connecting.is_connected());
        assert!(!McpConnectionState::Failed.is_connected());
        assert!(!McpConnectionState::NeedsAuth.is_connected());
        assert!(!McpConnectionState::Reconnecting.is_connected());
    }

    #[test]
    fn connection_state_is_transient() {
        assert!(McpConnectionState::Connecting.is_transient());
        assert!(McpConnectionState::Reconnecting.is_transient());
        assert!(!McpConnectionState::Connected.is_transient());
        assert!(!McpConnectionState::Disconnected.is_transient());
        assert!(!McpConnectionState::Failed.is_transient());
        assert!(!McpConnectionState::NeedsAuth.is_transient());
    }

    #[test]
    fn connection_state_is_failed() {
        assert!(McpConnectionState::Failed.is_failed());
        assert!(!McpConnectionState::Connected.is_failed());
    }

    #[test]
    fn connection_state_needs_auth() {
        assert!(McpConnectionState::NeedsAuth.needs_auth());
        assert!(!McpConnectionState::Connected.needs_auth());
    }

    #[test]
    fn connection_state_display() {
        assert_eq!(McpConnectionState::Disconnected.to_string(), "disconnected");
        assert_eq!(McpConnectionState::Connecting.to_string(), "connecting");
        assert_eq!(McpConnectionState::Connected.to_string(), "connected");
        assert_eq!(McpConnectionState::Failed.to_string(), "failed");
        assert_eq!(McpConnectionState::NeedsAuth.to_string(), "needs_auth");
        assert_eq!(McpConnectionState::Reconnecting.to_string(), "reconnecting");
    }

    #[test]
    fn connection_state_serde_roundtrip() {
        let states = vec![
            McpConnectionState::Disconnected,
            McpConnectionState::Connecting,
            McpConnectionState::Connected,
            McpConnectionState::Failed,
            McpConnectionState::NeedsAuth,
            McpConnectionState::Reconnecting,
        ];
        for state in &states {
            let json = serde_json::to_string(state).expect("serialize");
            let back: McpConnectionState = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(&back, state);
        }
    }

    // ── McpConnectionLifecycle tests ──────────────────────────────────────

    #[test]
    fn new_lifecycle_is_empty() {
        let lc = McpConnectionLifecycle::new();
        assert_eq!(lc.server_count(), 0);
    }

    #[test]
    fn register_server_starts_disconnected() {
        let mut lc = McpConnectionLifecycle::new();
        lc.register_server("test-server");
        assert_eq!(
            lc.state("test-server"),
            Some(McpConnectionState::Disconnected)
        );
        assert_eq!(lc.server_count(), 1);
    }

    #[test]
    fn unregister_server_removes_it() {
        let mut lc = McpConnectionLifecycle::new();
        lc.register_server("srv");
        assert!(lc.is_registered("srv"));
        lc.unregister_server("srv");
        assert!(!lc.is_registered("srv"));
        assert_eq!(lc.state("srv"), None);
    }

    #[test]
    fn full_connect_cycle() {
        let mut lc = McpConnectionLifecycle::new();
        lc.register_server("srv");

        let event = lc.start_connecting("srv").expect("start connecting");
        assert!(matches!(event, McpLifecycleEvent::Connecting { .. }));
        assert_eq!(lc.state("srv"), Some(McpConnectionState::Connecting));

        let event = lc.mark_connected("srv").expect("mark connected");
        assert!(matches!(event, McpLifecycleEvent::Connected { .. }));
        assert_eq!(lc.state("srv"), Some(McpConnectionState::Connected));
    }

    #[test]
    fn connect_failure_triggers_reconnect() {
        let mut lc = McpConnectionLifecycle::new();
        lc.register_server("srv");

        lc.start_connecting("srv").expect("start connecting");
        let event = lc
            .mark_failed("srv", "connection refused")
            .expect("mark failed");
        assert!(matches!(
            event,
            McpLifecycleEvent::Reconnecting { attempt: 1, .. }
        ));
        assert_eq!(lc.state("srv"), Some(McpConnectionState::Reconnecting));
        assert_eq!(lc.reconnect_attempts("srv"), 1);
    }

    #[test]
    fn max_reconnect_attempts_exceeded() {
        let mut lc = McpConnectionLifecycle::with_settings(
            2,
            Duration::from_millis(10),
            Duration::from_millis(100),
            Duration::from_secs(5),
        );
        lc.register_server("srv");

        // First attempt: Connecting → Failed → Reconnecting
        lc.start_connecting("srv").expect("connect 1");
        lc.mark_failed("srv", "err1").expect("fail 1");
        assert_eq!(lc.state("srv"), Some(McpConnectionState::Reconnecting));

        // Second attempt: Reconnecting → Failed → Reconnecting
        lc.mark_failed("srv", "err2").expect("fail 2");
        assert_eq!(lc.state("srv"), Some(McpConnectionState::Reconnecting));

        // Third attempt: Reconnecting → Failed → Failed (max exceeded)
        let event = lc.mark_failed("srv", "err3").expect("fail 3");
        assert!(matches!(event, McpLifecycleEvent::Failed { .. }));
        assert_eq!(lc.state("srv"), Some(McpConnectionState::Failed));
    }

    #[test]
    fn reconnect_resets_on_success() {
        let mut lc = McpConnectionLifecycle::new();
        lc.register_server("srv");

        lc.start_connecting("srv").expect("connect");
        lc.mark_failed("srv", "err").expect("fail");
        assert_eq!(lc.reconnect_attempts("srv"), 1);

        // Reconnect succeeds
        lc.mark_connected("srv").expect("reconnect success");
        assert_eq!(lc.reconnect_attempts("srv"), 0);
        assert_eq!(lc.state("srv"), Some(McpConnectionState::Connected));
    }

    #[test]
    fn disconnect_resets_reconnect() {
        let mut lc = McpConnectionLifecycle::new();
        lc.register_server("srv");

        lc.start_connecting("srv").expect("connect");
        lc.mark_connected("srv").expect("connected");
        lc.disconnect("srv", DisconnectReason::Manual)
            .expect("disconnect");
        assert_eq!(lc.state("srv"), Some(McpConnectionState::Disconnected));
        assert_eq!(lc.reconnect_attempts("srv"), 0);
    }

    #[test]
    fn needs_auth_transition() {
        let mut lc = McpConnectionLifecycle::new();
        lc.register_server("srv");

        lc.start_connecting("srv").expect("connect");
        let event = lc.mark_needs_auth("srv").expect("needs auth");
        assert!(matches!(event, McpLifecycleEvent::NeedsAuth { .. }));
        assert_eq!(lc.state("srv"), Some(McpConnectionState::NeedsAuth));

        // Can reconnect from NeedsAuth
        let event = lc.start_connecting("srv").expect("reconnect from auth");
        assert!(matches!(event, McpLifecycleEvent::Connecting { .. }));
    }

    #[test]
    fn invalid_transition_connected_to_connecting() {
        let mut lc = McpConnectionLifecycle::new();
        lc.register_server("srv");
        lc.start_connecting("srv").expect("connect");
        lc.mark_connected("srv").expect("connected");

        let result = lc.start_connecting("srv");
        assert!(result.is_err());
        let err = result.expect_err("should fail");
        assert_eq!(err.from, McpConnectionState::Connected);
        assert_eq!(err.to, McpConnectionState::Connecting);
    }

    #[test]
    fn invalid_transition_disconnected_to_connected() {
        let mut lc = McpConnectionLifecycle::new();
        lc.register_server("srv");

        let result = lc.mark_connected("srv");
        assert!(result.is_err());
    }

    #[test]
    fn unregistered_server_returns_error() {
        let mut lc = McpConnectionLifecycle::new();
        let result = lc.start_connecting("unknown");
        assert!(result.is_err());
    }

    #[test]
    fn start_reconnect_from_disconnected() {
        let mut lc = McpConnectionLifecycle::new();
        lc.register_server("srv");

        let (event, backoff) = lc.start_reconnect("srv").expect("reconnect");
        assert!(matches!(
            event,
            McpLifecycleEvent::Reconnecting { attempt: 1, .. }
        ));
        assert_eq!(lc.state("srv"), Some(McpConnectionState::Reconnecting));
        assert!(backoff > Duration::ZERO);
    }

    #[test]
    fn start_reconnect_max_exceeded() {
        let mut lc = McpConnectionLifecycle::with_settings(
            1,
            Duration::from_millis(10),
            Duration::from_millis(100),
            Duration::from_secs(5),
        );
        lc.register_server("srv");

        // First reconnect
        let (event, _) = lc.start_reconnect("srv").expect("reconnect 1");
        assert!(matches!(event, McpLifecycleEvent::Reconnecting { .. }));

        // Fail the reconnect
        lc.mark_failed("srv", "err").expect("fail");

        // Second reconnect should exceed max
        let (event, _) = lc.start_reconnect("srv").expect("reconnect 2");
        assert!(matches!(event, McpLifecycleEvent::Failed { .. }));
    }

    #[test]
    fn force_reset_clears_state() {
        let mut lc = McpConnectionLifecycle::new();
        lc.register_server("srv");

        lc.start_connecting("srv").expect("connect");
        lc.mark_failed("srv", "err").expect("fail");
        assert_eq!(lc.state("srv"), Some(McpConnectionState::Reconnecting));

        lc.force_reset("srv");
        assert_eq!(lc.state("srv"), Some(McpConnectionState::Disconnected));
        assert_eq!(lc.reconnect_attempts("srv"), 0);
        assert!(lc.last_error("srv").is_none());
    }

    #[test]
    fn count_in_state() {
        let mut lc = McpConnectionLifecycle::new();
        lc.register_server("a");
        lc.register_server("b");
        lc.register_server("c");

        lc.start_connecting("a").expect("connect a");
        lc.start_connecting("b").expect("connect b");
        lc.mark_connected("a").expect("connected a");

        assert_eq!(lc.count_in_state(McpConnectionState::Connected), 1);
        assert_eq!(lc.count_in_state(McpConnectionState::Connecting), 1);
        assert_eq!(lc.count_in_state(McpConnectionState::Disconnected), 1);
    }

    #[test]
    fn servers_in_state() {
        let mut lc = McpConnectionLifecycle::new();
        lc.register_server("a");
        lc.register_server("b");

        lc.start_connecting("a").expect("connect a");
        let connected = lc.servers_in_state(McpConnectionState::Connecting);
        assert!(connected.contains(&"a"));
        assert!(!connected.contains(&"b"));
    }

    #[test]
    fn backoff_increases_exponentially() {
        let mut lc = McpConnectionLifecycle::with_settings(
            10,
            Duration::from_secs(1),
            Duration::from_secs(30),
            Duration::from_secs(5),
        );
        lc.register_server("srv");

        // Simulate multiple failures to increase backoff
        lc.start_connecting("srv").expect("connect");
        lc.mark_failed("srv", "err1").expect("fail 1");
        let backoff1 = lc.current_backoff("srv").expect("backoff 1");

        lc.mark_failed("srv", "err2").expect("fail 2");
        let backoff2 = lc.current_backoff("srv").expect("backoff 2");

        lc.mark_failed("srv", "err3").expect("fail 3");
        let backoff3 = lc.current_backoff("srv").expect("backoff 3");

        assert!(backoff2 > backoff1);
        assert!(backoff3 > backoff2);
    }

    #[test]
    fn last_error_preserved() {
        let mut lc = McpConnectionLifecycle::new();
        lc.register_server("srv");

        lc.start_connecting("srv").expect("connect");
        lc.mark_failed("srv", "connection refused").expect("fail");

        assert_eq!(lc.last_error("srv"), Some("connection refused"));
    }

    #[test]
    fn state_transition_error_display() {
        let err = StateTransitionError {
            from: McpConnectionState::Connected,
            to: McpConnectionState::Connecting,
            reason: "already connected".to_owned(),
        };
        let msg = err.to_string();
        assert!(msg.contains("connected"));
        assert!(msg.contains("connecting"));
        assert!(msg.contains("already connected"));
    }

    #[test]
    fn reconnect_from_failed_state() {
        let mut lc = McpConnectionLifecycle::with_settings(
            3,
            Duration::from_millis(10),
            Duration::from_millis(100),
            Duration::from_secs(5),
        );
        lc.register_server("srv");

        // Connect → fail all attempts → Failed
        lc.start_connecting("srv").expect("connect");
        lc.mark_failed("srv", "err1").expect("fail 1");
        lc.mark_failed("srv", "err2").expect("fail 2");
        lc.mark_failed("srv", "err3").expect("fail 3");
        lc.mark_failed("srv", "err4").expect("fail 4"); // exceeds max

        assert_eq!(lc.state("srv"), Some(McpConnectionState::Failed));

        // Force reset to Disconnected, then start a fresh reconnect cycle
        lc.force_reset("srv");
        assert_eq!(lc.state("srv"), Some(McpConnectionState::Disconnected));

        // Can start reconnect from Disconnected after reset
        let (event, _) = lc.start_reconnect("srv").expect("reconnect from failed");
        assert!(matches!(event, McpLifecycleEvent::Reconnecting { .. }));
    }

    #[test]
    fn needs_auth_from_failed_state() {
        let mut lc = McpConnectionLifecycle::new();
        lc.register_server("srv");

        lc.start_connecting("srv").expect("connect");
        lc.mark_failed("srv", "401 unauthorized").expect("fail");

        let event = lc.mark_needs_auth("srv").expect("needs auth");
        assert!(matches!(event, McpLifecycleEvent::NeedsAuth { .. }));
    }

    #[test]
    fn reconnected_event_on_reconnect_success() {
        let mut lc = McpConnectionLifecycle::new();
        lc.register_server("srv");

        lc.start_connecting("srv").expect("connect");
        lc.mark_connected("srv").expect("connected");
        lc.disconnect("srv", DisconnectReason::Error("network".to_owned()))
            .expect("disconnect");

        let (event, _) = lc.start_reconnect("srv").expect("reconnect");
        assert!(matches!(event, McpLifecycleEvent::Reconnecting { .. }));

        let event = lc.mark_connected("srv").expect("reconnect success");
        assert!(matches!(event, McpLifecycleEvent::Reconnected { .. }));
    }

    #[test]
    fn timed_out_servers_detection() {
        let mut lc = McpConnectionLifecycle::with_settings(
            5,
            Duration::from_secs(1),
            Duration::from_secs(30),
            Duration::from_nanos(1), // effectively instant timeout
        );
        lc.register_server("slow-srv");
        lc.register_server("fast-srv");

        lc.start_connecting("slow-srv").expect("connect slow");
        lc.start_connecting("fast-srv").expect("connect fast");

        // Both should be timed out with nanosecond timeout
        let timed_out = lc.timed_out_servers();
        assert!(timed_out.contains(&"slow-srv"));
        assert!(timed_out.contains(&"fast-srv"));
    }
}
