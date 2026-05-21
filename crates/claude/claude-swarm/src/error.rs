//! Error types for the rc-swarm crate.

use std::path::PathBuf;
use thiserror::Error;

/// All errors produced by the rc-swarm crate.
#[derive(Debug, Error)]
pub enum SwarmError {
    /// I/O error during file system operations.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// JSON serialization/deserialization error.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// Team not found.
    #[error("team not found: {0}")]
    TeamNotFound(String),

    /// Teammate not found in the team.
    #[error("teammate not found: {agent_name} in team {team_name}")]
    TeammateNotFound {
        /// Name of the teammate.
        agent_name: String,
        /// Name of the team.
        team_name: String,
    },

    /// Team already exists.
    #[error("team already exists: {0}")]
    TeamAlreadyExists(String),

    /// Maximum number of teammates exceeded.
    #[error("maximum teammates ({max}) exceeded for team {team_name}")]
    MaxTeammatesExceeded {
        /// Maximum allowed teammates.
        max: usize,
        /// Team name.
        team_name: String,
    },

    /// Invalid team name.
    #[error("invalid team name: {0}")]
    InvalidTeamName(String),

    /// Invalid agent name.
    #[error("invalid agent name: {0}")]
    InvalidAgentName(String),

    /// Permission request timed out.
    #[error("permission request timed out: {request_id}")]
    PermissionTimeout {
        /// The request ID that timed out.
        request_id: String,
    },

    /// Permission request not found.
    #[error("permission request not found: {0}")]
    PermissionRequestNotFound(String),

    /// Backend unavailable.
    #[error("backend unavailable: {0}")]
    BackendUnavailable(String),

    /// Backend detection failed.
    #[error("backend detection failed: {message}")]
    BackendDetectionFailed {
        /// Error message.
        message: String,
    },

    /// Spawn failure.
    #[error("spawn failed for agent {agent_name}: {reason}")]
    SpawnFailed {
        /// Agent name.
        agent_name: String,
        /// Failure reason.
        reason: String,
    },

    /// Mailbox error.
    #[error("mailbox error for agent {agent_name}: {reason}")]
    MailboxError {
        /// Agent name.
        agent_name: String,
        /// Failure reason.
        reason: String,
    },

    /// Path-related error.
    #[error("invalid path: {0}")]
    InvalidPath(PathBuf),

    /// Configuration error.
    #[error("configuration error: {0}")]
    Config(String),

    /// The agent is not the team lead.
    #[error("agent {agent_name} is not the team lead (expected {lead_id})")]
    NotTeamLead {
        /// Agent name.
        agent_name: String,
        /// Expected lead ID.
        lead_id: String,
    },

    /// Reconnection failure.
    #[error("reconnection failed for agent {agent_name}: {reason}")]
    ReconnectionFailed {
        /// Agent name.
        agent_name: String,
        /// Failure reason.
        reason: String,
    },
}

/// Convenience alias for results in this crate.
pub type SwarmResult<T> = Result<T, SwarmError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_display_team_not_found() {
        let err = SwarmError::TeamNotFound("my-team".to_owned());
        assert!(err.to_string().contains("my-team"));
    }

    #[test]
    fn error_display_teammate_not_found() {
        let err = SwarmError::TeammateNotFound {
            agent_name: "worker-1".to_owned(),
            team_name: "my-team".to_owned(),
        };
        let msg = err.to_string();
        assert!(msg.contains("worker-1"));
        assert!(msg.contains("my-team"));
    }

    #[test]
    fn error_display_max_teammates() {
        let err = SwarmError::MaxTeammatesExceeded {
            max: 10,
            team_name: "big-team".to_owned(),
        };
        let msg = err.to_string();
        assert!(msg.contains("10"));
        assert!(msg.contains("big-team"));
    }

    #[test]
    fn error_display_permission_timeout() {
        let err = SwarmError::PermissionTimeout {
            request_id: "req-123".to_owned(),
        };
        assert!(err.to_string().contains("req-123"));
    }

    #[test]
    fn error_display_backend_unavailable() {
        let err = SwarmError::BackendUnavailable("tmux".to_owned());
        assert!(err.to_string().contains("tmux"));
    }

    #[test]
    fn error_display_spawn_failed() {
        let err = SwarmError::SpawnFailed {
            agent_name: "agent-1".to_owned(),
            reason: "port in use".to_owned(),
        };
        let msg = err.to_string();
        assert!(msg.contains("agent-1"));
        assert!(msg.contains("port in use"));
    }

    #[test]
    fn error_display_mailbox_error() {
        let err = SwarmError::MailboxError {
            agent_name: "agent-2".to_owned(),
            reason: "disk full".to_owned(),
        };
        let msg = err.to_string();
        assert!(msg.contains("agent-2"));
        assert!(msg.contains("disk full"));
    }

    #[test]
    fn error_display_not_team_lead() {
        let err = SwarmError::NotTeamLead {
            agent_name: "worker".to_owned(),
            lead_id: "lead-uuid".to_owned(),
        };
        let msg = err.to_string();
        assert!(msg.contains("worker"));
        assert!(msg.contains("lead-uuid"));
    }

    #[test]
    fn error_from_io() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file missing");
        let err: SwarmError = io_err.into();
        assert!(err.to_string().contains("file missing"));
    }

    #[test]
    fn error_from_json() {
        let json_err = serde_json::from_str::<serde_json::Value>("not json{{{");
        let err: SwarmError = json_err.expect_err("should fail").into();
        assert!(err.to_string().contains("JSON"));
    }

    #[test]
    fn swarm_result_ok() {
        let result: SwarmResult<i32> = Ok(42);
        assert!(matches!(result, Ok(42)));
    }

    #[test]
    fn swarm_result_err() {
        let result: SwarmResult<i32> = Err(SwarmError::InvalidTeamName("bad/name".to_owned()));
        assert!(result.is_err());
    }
}
