//! Error types for the Agent protocol layer.
//!
//! # Design decision: `AgentProtocolError` vs `anyhow::Result`
//!
//! This module defines a structured [`AgentProtocolError`] enum, but most
//! public APIs in this crate return `anyhow::Result`. This is intentional:
//!
//! - `AgentProtocolError` provides structured, typed errors for **callers**
//!   that need to pattern-match on specific failure modes (e.g. timeout,
//!   config error).
//! - `anyhow::Result` is used internally for ergonomic error propagation
//!   without requiring exhaustive error mapping at every layer boundary.
//!
//! Future work may convert more methods to return `Result<T, AgentProtocolError>`
//! once the error taxonomy stabilizes.

use thiserror::Error;

/// Errors that can occur during Agent protocol operations.
#[derive(Debug, Error)]
pub enum AgentProtocolError {
    /// The Agent has not been started yet.
    #[error("agent not started")]
    AgentNotStarted,

    /// The Agent has stopped unexpectedly.
    #[error("agent stopped: {reason}")]
    AgentStopped {
        /// Why the Agent stopped.
        reason: String,
    },

    /// Communication with the Agent failed.
    #[error("communication error: {details}")]
    CommunicationError {
        /// Underlying cause of the communication failure.
        details: String,
    },

    /// A protocol-level error (malformed message, unexpected response, etc.).
    #[error("protocol error: {message}")]
    ProtocolError {
        /// Description of the protocol violation.
        message: String,
    },

    /// An operation timed out.
    #[error("timeout after {duration_ms}ms")]
    Timeout {
        /// How long we waited before giving up, in milliseconds.
        duration_ms: u64,
    },

    /// The Agent configuration is invalid.
    #[error("config error: {message}")]
    ConfigError {
        /// What is wrong with the configuration.
        message: String,
    },
}

/// Errors that can occur in Agent adapter implementations.
///
/// This enum provides a structured, typed error set for the adapter layer.
/// Because `AdapterError` implements `std::error::Error` (via thiserror),
/// it can be used with `?` directly in functions returning `anyhow::Result`
/// thanks to anyhow's blanket `From<E: Error>` conversion.
#[derive(Debug, Error)]
pub enum AdapterError {
    /// The adapter was used before [`start`](crate::adapter::AgentAdapter::start) was called.
    #[error("adapter not started — call start() first")]
    NotStarted,

    /// The requested session does not exist.
    #[error("session not found: {session_id}")]
    SessionNotFound {
        /// Identifier of the missing session.
        session_id: String,
    },

    /// The agent panicked during execution.
    #[error("agent panic: {message}")]
    Panic {
        /// Panic message or description.
        message: String,
    },

    /// Communication with the agent failed.
    #[error("transport error: {message}")]
    Transport {
        /// Underlying transport error description.
        message: String,
    },

    /// The adapter configuration is invalid or incomplete.
    #[error("adapter config error: {message}")]
    Config {
        /// What is wrong with the configuration.
        message: String,
    },

    /// Catch-all for internal errors that don't fit a more specific variant.
    #[error("internal error: {message}")]
    Internal {
        /// Description of the internal error.
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_protocol_error_display_messages() {
        assert_eq!(
            AgentProtocolError::AgentNotStarted.to_string(),
            "agent not started"
        );
        assert_eq!(
            AgentProtocolError::AgentStopped {
                reason: "crashed".into()
            }
            .to_string(),
            "agent stopped: crashed"
        );
        assert_eq!(
            AgentProtocolError::CommunicationError {
                details: "io".into()
            }
            .to_string(),
            "communication error: io"
        );
        assert_eq!(
            AgentProtocolError::ProtocolError {
                message: "bad frame".into()
            }
            .to_string(),
            "protocol error: bad frame"
        );
        assert_eq!(
            AgentProtocolError::Timeout { duration_ms: 5000 }.to_string(),
            "timeout after 5000ms"
        );
        assert_eq!(
            AgentProtocolError::ConfigError {
                message: "missing binary".into()
            }
            .to_string(),
            "config error: missing binary"
        );
    }

    #[test]
    fn adapter_error_display_messages() {
        assert_eq!(
            AdapterError::NotStarted.to_string(),
            "adapter not started — call start() first"
        );
        assert_eq!(
            AdapterError::SessionNotFound {
                session_id: "abc-123".into()
            }
            .to_string(),
            "session not found: abc-123"
        );
        assert_eq!(
            AdapterError::Panic {
                message: "index out of bounds".into()
            }
            .to_string(),
            "agent panic: index out of bounds"
        );
        assert_eq!(
            AdapterError::Transport {
                message: "connection reset".into()
            }
            .to_string(),
            "transport error: connection reset"
        );
        assert_eq!(
            AdapterError::Config {
                message: "missing api key".into()
            }
            .to_string(),
            "adapter config error: missing api key"
        );
        assert_eq!(
            AdapterError::Internal {
                message: "unexpected state".into()
            }
            .to_string(),
            "internal error: unexpected state"
        );
    }

    #[test]
    fn adapter_error_converts_to_anyhow() {
        let err: anyhow::Error = AdapterError::NotStarted.into();
        assert_eq!(err.to_string(), "adapter not started — call start() first");
    }
}
