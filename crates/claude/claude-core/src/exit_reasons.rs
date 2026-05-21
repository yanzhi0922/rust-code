//! Exit reason tracking for the application runtime.
//!
//! This module defines [`ExitReason`] — an enumeration of all reasons the
//! application may terminate — and [`ExitReasonTracker`] for recording and
//! querying exit events across a session.
//!
//! # Exit Reasons
//!
//! Exit reasons are categorized into:
//!
//! - **Normal**: `Completed`, `Cancelled`
//! - **Resource**: `ContextOverflow`, `RateLimit`, `Timeout`, `CostLimitExceeded`
//! - **Error**: `ModelError`, `NetworkError`, `InternalError`
//! - **Policy**: `PermissionDenied`, `HookAbort`, `MaxTurnsReached`
//! - **Lifecycle**: `SessionExpired`, `UserInterrupt`, `ToolFailure`

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// ExitReason — why the application exited
// ---------------------------------------------------------------------------

/// Reason the application or session exited.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExitReason {
    /// The session completed successfully.
    Completed,
    /// The user cancelled the session.
    Cancelled,
    /// The context window overflowed and could not be recovered.
    ContextOverflow,
    /// The LLM model returned an unrecoverable error.
    ModelError {
        /// Error message from the model.
        message: String,
    },
    /// The provider rate-limited the request.
    RateLimit {
        /// Seconds until the rate limit resets (if known).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        retry_after_seconds: Option<u64>,
    },
    /// Permission was denied for a required operation.
    PermissionDenied {
        /// Tool or operation that was denied.
        operation: String,
    },
    /// A network error occurred.
    NetworkError {
        /// Error description.
        message: String,
    },
    /// An internal error occurred.
    InternalError {
        /// Error description.
        message: String,
    },
    /// The session expired due to inactivity or timeout.
    SessionExpired,
    /// The user explicitly interrupted the session.
    UserInterrupt,
    /// A hook aborted the operation.
    HookAbort {
        /// Hook that caused the abort.
        hook_name: String,
        /// Why the hook aborted.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
    /// A tool execution failed critically.
    ToolFailure {
        /// Tool that failed.
        tool_name: String,
        /// Error message.
        message: String,
    },
    /// The maximum number of turns was reached.
    MaxTurnsReached {
        /// Maximum turns allowed.
        max_turns: u32,
        /// Turns actually used.
        turns_used: u32,
    },
    /// The cost limit was exceeded.
    CostLimitExceeded {
        /// Cost limit in USD.
        limit_usd: f64,
        /// Actual cost in USD.
        actual_usd: f64,
    },
    /// The operation timed out.
    Timeout {
        /// Timeout duration in seconds.
        timeout_seconds: u64,
    },
}

impl ExitReason {
    /// Return a human-readable label for the exit reason.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::ContextOverflow => "context_overflow",
            Self::ModelError { .. } => "model_error",
            Self::RateLimit { .. } => "rate_limit",
            Self::PermissionDenied { .. } => "permission_denied",
            Self::NetworkError { .. } => "network_error",
            Self::InternalError { .. } => "internal_error",
            Self::SessionExpired => "session_expired",
            Self::UserInterrupt => "user_interrupt",
            Self::HookAbort { .. } => "hook_abort",
            Self::ToolFailure { .. } => "tool_failure",
            Self::MaxTurnsReached { .. } => "max_turns_reached",
            Self::CostLimitExceeded { .. } => "cost_limit_exceeded",
            Self::Timeout { .. } => "timeout",
        }
    }

    /// Check if this exit reason represents a normal (non-error) exit.
    #[must_use]
    pub fn is_normal(&self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Cancelled | Self::UserInterrupt
        )
    }

    /// Check if this exit reason is recoverable (the session could potentially continue).
    #[must_use]
    pub fn is_recoverable(&self) -> bool {
        matches!(
            self,
            Self::RateLimit { .. }
                | Self::ContextOverflow
                | Self::Timeout { .. }
                | Self::UserInterrupt
        )
    }

    /// Check if this exit reason indicates an error condition.
    #[must_use]
    pub fn is_error(&self) -> bool {
        !self.is_normal()
    }

    /// Return a user-friendly description of the exit reason.
    #[must_use]
    pub fn description(&self) -> String {
        match self {
            Self::Completed => "Session completed successfully.".to_owned(),
            Self::Cancelled => "Session was cancelled.".to_owned(),
            Self::ContextOverflow => {
                "Context window overflowed and could not be recovered.".to_owned()
            }
            Self::ModelError { message } => format!("Model error: {message}"),
            Self::RateLimit {
                retry_after_seconds,
            } => {
                let retry_text = retry_after_seconds
                    .map(|s| format!(" Retry after {s}s."))
                    .unwrap_or_default();
                format!("Rate limit exceeded.{retry_text}")
            }
            Self::PermissionDenied { operation } => {
                format!("Permission denied for: {operation}")
            }
            Self::NetworkError { message } => format!("Network error: {message}"),
            Self::InternalError { message } => format!("Internal error: {message}"),
            Self::SessionExpired => "Session expired.".to_owned(),
            Self::UserInterrupt => "Session interrupted by user.".to_owned(),
            Self::HookAbort { hook_name, detail } => {
                let detail_text = detail
                    .as_deref()
                    .map(|d| format!(": {d}"))
                    .unwrap_or_default();
                format!("Hook '{hook_name}' aborted the operation{detail_text}.")
            }
            Self::ToolFailure { tool_name, message } => {
                format!("Tool '{tool_name}' failed: {message}")
            }
            Self::MaxTurnsReached {
                max_turns,
                turns_used,
            } => format!("Maximum turns reached ({turns_used}/{max_turns})."),
            Self::CostLimitExceeded {
                limit_usd,
                actual_usd,
            } => format!("Cost limit exceeded: ${actual_usd:.2} > ${limit_usd:.2}"),
            Self::Timeout { timeout_seconds } => {
                format!("Operation timed out after {timeout_seconds}s.")
            }
        }
    }

    /// Count the total number of variants.
    #[must_use]
    pub fn variant_count() -> usize {
        15
    }
}

// ---------------------------------------------------------------------------
// ExitRecord — a single recorded exit event
// ---------------------------------------------------------------------------

/// A recorded exit event with timestamp and optional context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExitRecord {
    /// The exit reason.
    pub reason: ExitReason,
    /// When the exit occurred.
    pub timestamp: DateTime<Utc>,
    /// Optional session ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Optional additional context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
}

impl ExitRecord {
    /// Create a new exit record for the given reason.
    #[must_use]
    pub fn new(reason: ExitReason) -> Self {
        Self {
            reason,
            timestamp: Utc::now(),
            session_id: None,
            context: None,
        }
    }

    /// Create a new exit record with a session ID.
    #[must_use]
    pub fn with_session(reason: ExitReason, session_id: impl Into<String>) -> Self {
        Self {
            reason,
            timestamp: Utc::now(),
            session_id: Some(session_id.into()),
            context: None,
        }
    }

    /// Add context to the exit record.
    #[must_use]
    pub fn with_context(mut self, context: impl Into<String>) -> Self {
        self.context = Some(context.into());
        self
    }
}

// ---------------------------------------------------------------------------
// ExitReasonTracker — track exit events across a session
// ---------------------------------------------------------------------------

/// Tracker for recording and querying exit reasons.
///
/// Maintains a chronological log of exit events and provides
/// methods for querying by type, recoverability, and recency.
#[derive(Debug, Clone, Default)]
pub struct ExitReasonTracker {
    records: Vec<ExitRecord>,
}

impl ExitReasonTracker {
    /// Create a new empty tracker.
    #[must_use]
    pub fn new() -> Self {
        Self {
            records: Vec::new(),
        }
    }

    /// Record an exit reason.
    pub fn track(&mut self, reason: ExitReason) {
        self.records.push(ExitRecord::new(reason));
    }

    /// Record an exit reason with a session ID.
    pub fn track_with_session(&mut self, reason: ExitReason, session_id: impl Into<String>) {
        self.records
            .push(ExitRecord::with_session(reason, session_id));
    }

    /// Return the most recent exit reason, if any.
    #[must_use]
    pub fn last_reason(&self) -> Option<&ExitReason> {
        self.records.last().map(|r| &r.reason)
    }

    /// Return the most recent exit record, if any.
    #[must_use]
    pub fn last_record(&self) -> Option<&ExitRecord> {
        self.records.last()
    }

    /// Return all exit records.
    #[must_use]
    pub fn records(&self) -> &[ExitRecord] {
        &self.records
    }

    /// Count how many times a specific exit reason type occurred.
    pub fn count_by_label(&self, label: &str) -> usize {
        self.records
            .iter()
            .filter(|r| r.reason.label() == label)
            .count()
    }

    /// Check if any recorded reason matches a predicate.
    pub fn has_reason(&self, predicate: impl Fn(&ExitReason) -> bool) -> bool {
        self.records.iter().any(|r| predicate(&r.reason))
    }

    /// Return the number of error exits.
    #[must_use]
    pub fn error_count(&self) -> usize {
        self.records.iter().filter(|r| r.reason.is_error()).count()
    }

    /// Return the number of recoverable exits.
    #[must_use]
    pub fn recoverable_count(&self) -> usize {
        self.records
            .iter()
            .filter(|r| r.reason.is_recoverable())
            .count()
    }

    /// Return the total number of recorded exits.
    #[must_use]
    pub fn count(&self) -> usize {
        self.records.len()
    }

    /// Check if no exits have been recorded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Clear all recorded exits.
    pub fn clear(&mut self) {
        self.records.clear();
    }

    /// Return all records matching a predicate.
    #[must_use]
    pub fn filter(&self, predicate: impl Fn(&ExitRecord) -> bool) -> Vec<&ExitRecord> {
        self.records.iter().filter(|r| predicate(r)).collect()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_reason_completed() {
        let reason = ExitReason::Completed;
        assert_eq!(reason.label(), "completed");
        assert!(reason.is_normal());
        assert!(!reason.is_error());
        assert!(!reason.is_recoverable());
        assert!(reason.description().contains("successfully"));
    }

    #[test]
    fn exit_reason_cancelled() {
        let reason = ExitReason::Cancelled;
        assert_eq!(reason.label(), "cancelled");
        assert!(reason.is_normal());
    }

    #[test]
    fn exit_reason_context_overflow() {
        let reason = ExitReason::ContextOverflow;
        assert_eq!(reason.label(), "context_overflow");
        assert!(reason.is_error());
        assert!(reason.is_recoverable());
    }

    #[test]
    fn exit_reason_model_error() {
        let reason = ExitReason::ModelError {
            message: "bad request".to_owned(),
        };
        assert_eq!(reason.label(), "model_error");
        assert!(reason.is_error());
        assert!(!reason.is_recoverable());
        assert!(reason.description().contains("bad request"));
    }

    #[test]
    fn exit_reason_rate_limit() {
        let reason = ExitReason::RateLimit {
            retry_after_seconds: Some(60),
        };
        assert_eq!(reason.label(), "rate_limit");
        assert!(reason.is_error());
        assert!(reason.is_recoverable());
        assert!(reason.description().contains("60s"));
    }

    #[test]
    fn exit_reason_permission_denied() {
        let reason = ExitReason::PermissionDenied {
            operation: "bash".to_owned(),
        };
        assert!(reason.is_error());
        assert!(reason.description().contains("bash"));
    }

    #[test]
    fn exit_reason_network_error() {
        let reason = ExitReason::NetworkError {
            message: "connection refused".to_owned(),
        };
        assert!(reason.is_error());
        assert!(reason.description().contains("connection refused"));
    }

    #[test]
    fn exit_reason_internal_error() {
        let reason = ExitReason::InternalError {
            message: "panic".to_owned(),
        };
        assert!(reason.is_error());
        assert!(reason.description().contains("panic"));
    }

    #[test]
    fn exit_reason_hook_abort() {
        let reason = ExitReason::HookAbort {
            hook_name: "pre-commit".to_owned(),
            detail: Some("lint failed".to_owned()),
        };
        assert!(reason.is_error());
        assert!(reason.description().contains("pre-commit"));
        assert!(reason.description().contains("lint failed"));
    }

    #[test]
    fn exit_reason_tool_failure() {
        let reason = ExitReason::ToolFailure {
            tool_name: "bash".to_owned(),
            message: "exit code 1".to_owned(),
        };
        assert!(reason.is_error());
        assert!(reason.description().contains("bash"));
    }

    #[test]
    fn exit_reason_max_turns_reached() {
        let reason = ExitReason::MaxTurnsReached {
            max_turns: 50,
            turns_used: 50,
        };
        assert!(reason.is_error());
        assert!(reason.description().contains("50/50"));
    }

    #[test]
    fn exit_reason_cost_limit_exceeded() {
        let reason = ExitReason::CostLimitExceeded {
            limit_usd: 5.0,
            actual_usd: 6.5,
        };
        assert!(reason.is_error());
        assert!(reason.description().contains("$6.50"));
    }

    #[test]
    fn exit_reason_timeout() {
        let reason = ExitReason::Timeout {
            timeout_seconds: 300,
        };
        assert!(reason.is_error());
        assert!(reason.is_recoverable());
        assert!(reason.description().contains("300s"));
    }

    #[test]
    fn exit_reason_user_interrupt() {
        let reason = ExitReason::UserInterrupt;
        assert!(reason.is_normal());
        assert!(reason.is_recoverable());
    }

    #[test]
    fn exit_reason_session_expired() {
        let reason = ExitReason::SessionExpired;
        assert!(reason.is_error());
        assert!(!reason.is_recoverable());
    }

    #[test]
    fn exit_reason_variant_count() {
        assert_eq!(ExitReason::variant_count(), 15);
    }

    #[test]
    fn exit_reason_serialization_roundtrip() {
        let reasons = vec![
            ExitReason::Completed,
            ExitReason::Cancelled,
            ExitReason::ContextOverflow,
            ExitReason::ModelError {
                message: "test".to_owned(),
            },
            ExitReason::RateLimit {
                retry_after_seconds: None,
            },
            ExitReason::PermissionDenied {
                operation: "bash".to_owned(),
            },
            ExitReason::NetworkError {
                message: "timeout".to_owned(),
            },
            ExitReason::InternalError {
                message: "bug".to_owned(),
            },
            ExitReason::SessionExpired,
            ExitReason::UserInterrupt,
            ExitReason::HookAbort {
                hook_name: "hook".to_owned(),
                detail: None,
            },
            ExitReason::ToolFailure {
                tool_name: "bash".to_owned(),
                message: "fail".to_owned(),
            },
            ExitReason::MaxTurnsReached {
                max_turns: 10,
                turns_used: 10,
            },
            ExitReason::CostLimitExceeded {
                limit_usd: 1.0,
                actual_usd: 2.0,
            },
            ExitReason::Timeout {
                timeout_seconds: 60,
            },
        ];
        assert_eq!(reasons.len(), 15);
        for reason in reasons {
            let json = serde_json::to_string(&reason).expect("serialize should succeed");
            let parsed: ExitReason =
                serde_json::from_str(&json).expect("deserialize should succeed");
            assert_eq!(reason.label(), parsed.label());
        }
    }

    #[test]
    fn exit_record_creation() {
        let record = ExitRecord::new(ExitReason::Completed);
        assert_eq!(record.reason, ExitReason::Completed);
        assert!(record.session_id.is_none());
        assert!(record.context.is_none());
    }

    #[test]
    fn exit_record_with_session() {
        let record = ExitRecord::with_session(ExitReason::Cancelled, "sess-123");
        assert_eq!(record.reason, ExitReason::Cancelled);
        assert_eq!(record.session_id.as_deref(), Some("sess-123"));
    }

    #[test]
    fn exit_record_with_context() {
        let record =
            ExitRecord::new(ExitReason::ContextOverflow).with_context("200k tokens exceeded");
        assert_eq!(record.context.as_deref(), Some("200k tokens exceeded"));
    }

    #[test]
    fn exit_reason_tracker_track() {
        let mut tracker = ExitReasonTracker::new();
        assert!(tracker.is_empty());

        tracker.track(ExitReason::Completed);
        assert_eq!(tracker.count(), 1);
        assert_eq!(tracker.last_reason(), Some(&ExitReason::Completed));
    }

    #[test]
    fn exit_reason_tracker_multiple() {
        let mut tracker = ExitReasonTracker::new();
        tracker.track(ExitReason::RateLimit {
            retry_after_seconds: None,
        });
        tracker.track(ExitReason::ModelError {
            message: "fail".to_owned(),
        });
        tracker.track(ExitReason::Completed);

        assert_eq!(tracker.count(), 3);
        assert_eq!(tracker.error_count(), 2);
        assert_eq!(tracker.recoverable_count(), 1);
    }

    #[test]
    fn exit_reason_tracker_count_by_label() {
        let mut tracker = ExitReasonTracker::new();
        tracker.track(ExitReason::RateLimit {
            retry_after_seconds: None,
        });
        tracker.track(ExitReason::RateLimit {
            retry_after_seconds: Some(30),
        });
        tracker.track(ExitReason::Completed);

        assert_eq!(tracker.count_by_label("rate_limit"), 2);
        assert_eq!(tracker.count_by_label("completed"), 1);
        assert_eq!(tracker.count_by_label("cancelled"), 0);
    }

    #[test]
    fn exit_reason_tracker_has_reason() {
        let mut tracker = ExitReasonTracker::new();
        tracker.track(ExitReason::ContextOverflow);

        assert!(tracker.has_reason(|r| matches!(r, ExitReason::ContextOverflow)));
        assert!(!tracker.has_reason(|r| matches!(r, ExitReason::Completed)));
    }

    #[test]
    fn exit_reason_tracker_clear() {
        let mut tracker = ExitReasonTracker::new();
        tracker.track(ExitReason::Completed);
        tracker.clear();
        assert!(tracker.is_empty());
        assert!(tracker.last_reason().is_none());
    }

    #[test]
    fn exit_reason_tracker_filter() {
        let mut tracker = ExitReasonTracker::new();
        tracker.track(ExitReason::Completed);
        tracker.track(ExitReason::ModelError {
            message: "fail".to_owned(),
        });
        tracker.track(ExitReason::Cancelled);

        let errors = tracker.filter(|r| r.reason.is_error());
        assert_eq!(errors.len(), 1);
    }

    #[test]
    fn exit_reason_tracker_with_session() {
        let mut tracker = ExitReasonTracker::new();
        tracker.track_with_session(ExitReason::Completed, "sess-1");
        let record = tracker.last_record().expect("should have a record");
        assert_eq!(record.session_id.as_deref(), Some("sess-1"));
    }

    #[test]
    fn exit_record_serialization() {
        let record = ExitRecord::new(ExitReason::Completed).with_context("test context");
        let json = serde_json::to_string(&record).expect("serialize should succeed");
        let parsed: ExitRecord = serde_json::from_str(&json).expect("deserialize should succeed");
        assert_eq!(parsed.reason, ExitReason::Completed);
        assert_eq!(parsed.context.as_deref(), Some("test context"));
    }
}
