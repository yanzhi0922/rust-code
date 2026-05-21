//! Elicitation request handling for MCP servers.
//!
//! Elicitation is an MCP feature where a server can request information
//! from the user mid-interaction. This module provides types for handling
//! elicitation requests, including automatic decline and queued handlers
//! for external processing. Supports URL and text type elicitation with
//! timeout handling.

use parking_lot::Mutex;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

// ── Elicitation type ─────────────────────────────────────────────────────────

/// The type of elicitation being requested.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ElicitationType {
    /// Request for a text input from the user.
    Text,
    /// Request for the user to visit a URL.
    Url,
}

// ── Elicitation params ──────────────────────────────────────────────────────

/// Parameters sent by an MCP server when requesting elicitation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElicitationParams {
    /// Human-readable message describing what information is needed.
    pub message: String,
    /// Optional JSON Schema describing the expected response format.
    #[serde(default)]
    pub requested_schema: Option<serde_json::Value>,
    /// The type of elicitation (text or URL).
    #[serde(default)]
    pub elicitation_type: Option<ElicitationType>,
    /// Optional URL for URL-type elicitation.
    #[serde(default)]
    pub url: Option<String>,
    /// Timeout for the elicitation request in seconds.
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

impl ElicitationParams {
    /// Create a new text elicitation params.
    #[must_use]
    pub fn text(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            requested_schema: None,
            elicitation_type: Some(ElicitationType::Text),
            url: None,
            timeout_secs: None,
        }
    }

    /// Create a new URL elicitation params.
    #[must_use]
    pub fn url(message: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            requested_schema: None,
            elicitation_type: Some(ElicitationType::Url),
            url: Some(url.into()),
            timeout_secs: None,
        }
    }

    /// Returns `true` if this is a URL-type elicitation.
    #[must_use]
    pub fn is_url_elicitation(&self) -> bool {
        matches!(self.elicitation_type, Some(ElicitationType::Url))
    }

    /// Returns `true` if this is a text-type elicitation.
    #[must_use]
    pub fn is_text_elicitation(&self) -> bool {
        matches!(self.elicitation_type, Some(ElicitationType::Text))
    }

    /// Get the timeout duration, if specified.
    #[must_use]
    pub fn timeout(&self) -> Option<Duration> {
        self.timeout_secs.map(Duration::from_secs)
    }
}

// ── Elicitation result ──────────────────────────────────────────────────────

/// Response to an elicitation request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "action")]
pub enum ElicitationResult {
    /// User accepted and provided the requested information.
    #[serde(rename = "accept")]
    Accept {
        /// The content provided by the user.
        content: serde_json::Value,
    },
    /// User declined to provide information.
    #[serde(rename = "decline")]
    Decline,
    /// User cancelled the elicitation request.
    #[serde(rename = "cancel")]
    Cancel,
}

impl ElicitationResult {
    /// Create an accept result with text content.
    #[must_use]
    pub fn accept_text(text: impl Into<String>) -> Self {
        Self::Accept {
            content: serde_json::Value::String(text.into()),
        }
    }

    /// Create an accept result with a URL.
    #[must_use]
    pub fn accept_url(url: impl Into<String>) -> Self {
        Self::Accept {
            content: serde_json::json!({ "url": url.into() }),
        }
    }

    /// Returns `true` if the result is an accept.
    #[must_use]
    pub fn is_accepted(&self) -> bool {
        matches!(self, Self::Accept { .. })
    }

    /// Returns `true` if the result is a decline.
    #[must_use]
    pub fn is_declined(&self) -> bool {
        matches!(self, Self::Decline)
    }

    /// Returns `true` if the result is a cancel.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancel)
    }
}

// ── Elicitation waiting state ───────────────────────────────────────────────

/// State of an elicitation request's lifecycle.
#[derive(Debug, Clone)]
pub enum ElicitationWaitingState {
    /// Waiting for user response.
    Waiting,
    /// User has responded.
    Completed(Box<ElicitationResult>),
    /// Request has expired without a response.
    Expired,
}

impl ElicitationWaitingState {
    /// Returns `true` if still waiting for a response.
    #[must_use]
    pub fn is_waiting(&self) -> bool {
        matches!(self, Self::Waiting)
    }

    /// Returns `true` if the request has completed.
    #[must_use]
    pub fn is_completed(&self) -> bool {
        matches!(self, Self::Completed(_))
    }

    /// Returns `true` if the request has expired.
    #[must_use]
    pub fn is_expired(&self) -> bool {
        matches!(self, Self::Expired)
    }
}

// ── Elicitation request event ───────────────────────────────────────────────

/// Event representing an incoming elicitation request from an MCP server.
#[derive(Debug)]
pub struct ElicitationRequestEvent {
    /// Name of the MCP server making the request.
    pub server_name: String,
    /// Unique request identifier.
    pub request_id: String,
    /// Elicitation parameters.
    pub params: ElicitationParams,
    /// Current waiting state.
    pub waiting_state: ElicitationWaitingState,
    /// When this event was created.
    pub created_at: Instant,
}

impl ElicitationRequestEvent {
    /// Create a new elicitation request event.
    pub fn new(
        server_name: impl Into<String>,
        request_id: impl Into<String>,
        params: ElicitationParams,
    ) -> Self {
        Self {
            server_name: server_name.into(),
            request_id: request_id.into(),
            params,
            waiting_state: ElicitationWaitingState::Waiting,
            created_at: Instant::now(),
        }
    }

    /// Check if this event has timed out.
    #[must_use]
    pub fn is_timed_out(&self) -> bool {
        match self.params.timeout() {
            Some(timeout) => self.created_at.elapsed() >= timeout,
            None => false,
        }
    }

    /// Get the elapsed time since this event was created.
    #[must_use]
    pub fn elapsed(&self) -> Duration {
        self.created_at.elapsed()
    }
}

// ── Elicitation handler trait ───────────────────────────────────────────────

/// Trait for handling elicitation requests from MCP servers.
///
/// Implementations decide how to respond when a server asks for
/// user input during a tool call or other interaction.
pub trait ElicitationHandler: Send + Sync {
    /// Handle an elicitation request and return the result.
    fn handle_elicitation(&self, event: ElicitationRequestEvent) -> ElicitationResult;
}

// ── Auto-decline handler ────────────────────────────────────────────────────

/// Default elicitation handler that automatically declines all requests.
///
/// Useful as a safe default when no user interaction is available.
#[derive(Debug, Clone, Default)]
pub struct AutoDeclineElicitationHandler;

impl AutoDeclineElicitationHandler {
    /// Create a new auto-decline handler.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl ElicitationHandler for AutoDeclineElicitationHandler {
    fn handle_elicitation(&self, _event: ElicitationRequestEvent) -> ElicitationResult {
        ElicitationResult::Decline
    }
}

// ── Auto-cancel handler ─────────────────────────────────────────────────────

/// Elicitation handler that automatically cancels all requests.
#[derive(Debug, Clone, Default)]
pub struct AutoCancelElicitationHandler;

impl AutoCancelElicitationHandler {
    /// Create a new auto-cancel handler.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl ElicitationHandler for AutoCancelElicitationHandler {
    fn handle_elicitation(&self, _event: ElicitationRequestEvent) -> ElicitationResult {
        ElicitationResult::Cancel
    }
}

// ── Timeout-aware handler ───────────────────────────────────────────────────

/// Elicitation handler that checks for timeouts before delegating.
///
/// If the elicitation event has a timeout and it has been exceeded,
/// returns `Cancel` instead of delegating to the inner handler.
pub struct TimeoutElicitationHandler<H: ElicitationHandler> {
    inner: H,
}

impl<H: ElicitationHandler> TimeoutElicitationHandler<H> {
    /// Create a new timeout-aware handler wrapping the given handler.
    #[must_use]
    pub fn new(inner: H) -> Self {
        Self { inner }
    }
}

impl<H: ElicitationHandler> ElicitationHandler for TimeoutElicitationHandler<H> {
    fn handle_elicitation(&self, event: ElicitationRequestEvent) -> ElicitationResult {
        if event.is_timed_out() {
            ElicitationResult::Cancel
        } else {
            self.inner.handle_elicitation(event)
        }
    }
}

// ── Queued handler ──────────────────────────────────────────────────────────

/// Elicitation handler that queues requests for external processing.
///
/// Collects all incoming elicitation events in an internal queue so
/// that an external consumer (e.g., a UI) can process them at its
/// own pace.
#[derive(Debug, Default)]
pub struct QueuedElicitationHandler {
    pending: Arc<Mutex<Vec<ElicitationRequestEvent>>>,
}

impl QueuedElicitationHandler {
    /// Create a new queued handler.
    #[must_use]
    pub fn new() -> Self {
        Self {
            pending: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Drain all pending elicitation events.
    ///
    /// Returns all queued events and clears the internal buffer.
    pub fn drain_pending(&self) -> Vec<ElicitationRequestEvent> {
        let mut guard = self.pending.lock();
        std::mem::take(&mut *guard)
    }

    /// Get the number of pending events.
    #[must_use]
    pub fn pending_count(&self) -> usize {
        self.pending.lock().len()
    }
}

impl ElicitationHandler for QueuedElicitationHandler {
    fn handle_elicitation(&self, event: ElicitationRequestEvent) -> ElicitationResult {
        let mut guard = self.pending.lock();
        guard.push(event);
        // Return decline by default; the queued events can be processed
        // asynchronously and the response can be updated later.
        ElicitationResult::Decline
    }
}

// ── Callback handler ────────────────────────────────────────────────────────

/// Elicitation handler that delegates to a closure.
///
/// Allows inline handling of elicitation requests without defining
/// a full struct implementation.
pub struct CallbackElicitationHandler<F>
where
    F: Fn(ElicitationRequestEvent) -> ElicitationResult + Send + Sync,
{
    callback: F,
}

impl<F> CallbackElicitationHandler<F>
where
    F: Fn(ElicitationRequestEvent) -> ElicitationResult + Send + Sync,
{
    /// Create a new callback handler.
    #[must_use]
    pub fn new(callback: F) -> Self {
        Self { callback }
    }
}

impl<F> ElicitationHandler for CallbackElicitationHandler<F>
where
    F: Fn(ElicitationRequestEvent) -> ElicitationResult + Send + Sync,
{
    fn handle_elicitation(&self, event: ElicitationRequestEvent) -> ElicitationResult {
        (self.callback)(event)
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_event(message: &str) -> ElicitationRequestEvent {
        ElicitationRequestEvent {
            server_name: "test-server".to_owned(),
            request_id: "req-1".to_owned(),
            params: ElicitationParams {
                message: message.to_owned(),
                requested_schema: Some(json!({"type": "string"})),
                elicitation_type: Some(ElicitationType::Text),
                url: None,
                timeout_secs: None,
            },
            waiting_state: ElicitationWaitingState::Waiting,
            created_at: Instant::now(),
        }
    }

    #[test]
    fn auto_decline_handler_returns_decline() {
        let handler = AutoDeclineElicitationHandler::new();
        let event = make_event("Enter your API key");
        let result = handler.handle_elicitation(event);
        assert_eq!(result, ElicitationResult::Decline);
    }

    #[test]
    fn queued_handler_collects_events() {
        let handler = QueuedElicitationHandler::new();
        let event1 = make_event("Question 1");
        let event2 = ElicitationRequestEvent {
            server_name: "other-server".to_owned(),
            request_id: "req-2".to_owned(),
            params: ElicitationParams {
                message: "Question 2".to_owned(),
                requested_schema: None,
                elicitation_type: None,
                url: None,
                timeout_secs: None,
            },
            waiting_state: ElicitationWaitingState::Waiting,
            created_at: Instant::now(),
        };

        let _ = handler.handle_elicitation(event1);
        let _ = handler.handle_elicitation(event2);

        assert_eq!(handler.pending_count(), 2);
        let drained = handler.drain_pending();
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0].server_name, "test-server");
        assert_eq!(drained[1].server_name, "other-server");
        assert!(handler.pending_count() == 0);
    }

    #[test]
    fn callback_handler_delegates() {
        let handler = CallbackElicitationHandler::new(|_event| ElicitationResult::Accept {
            content: json!("user-response"),
        });
        let event = make_event("Enter value");
        let result = handler.handle_elicitation(event);
        assert_eq!(
            result,
            ElicitationResult::Accept {
                content: json!("user-response")
            }
        );
    }

    #[test]
    fn elicitation_result_serde_roundtrip() {
        let accept = ElicitationResult::Accept {
            content: json!({"key": "value"}),
        };
        let json_str = serde_json::to_string(&accept).expect("serialize");
        let back: ElicitationResult = serde_json::from_str(&json_str).expect("deserialize");
        assert_eq!(back, accept);

        let decline = ElicitationResult::Decline;
        let json_str = serde_json::to_string(&decline).expect("serialize");
        let back: ElicitationResult = serde_json::from_str(&json_str).expect("deserialize");
        assert_eq!(back, decline);

        let cancel = ElicitationResult::Cancel;
        let json_str = serde_json::to_string(&cancel).expect("serialize");
        let back: ElicitationResult = serde_json::from_str(&json_str).expect("deserialize");
        assert_eq!(back, cancel);
    }

    #[test]
    fn elicitation_params_serde_roundtrip() {
        let params = ElicitationParams {
            message: "Please enter your name".to_owned(),
            requested_schema: Some(json!({"type": "string", "maxLength": 100})),
            elicitation_type: Some(ElicitationType::Text),
            url: None,
            timeout_secs: Some(30),
        };
        let json_str = serde_json::to_string(&params).expect("serialize");
        let back: ElicitationParams = serde_json::from_str(&json_str).expect("deserialize");
        assert_eq!(back.message, "Please enter your name");
        assert!(back.requested_schema.is_some());
        assert_eq!(back.elicitation_type, Some(ElicitationType::Text));
        assert_eq!(back.timeout_secs, Some(30));
    }

    #[test]
    fn elicitation_params_deserializes_without_schema() {
        let json_str = r#"{"message":"hello"}"#;
        let params: ElicitationParams = serde_json::from_str(json_str).expect("deserialize");
        assert_eq!(params.message, "hello");
        assert!(params.requested_schema.is_none());
        assert!(params.elicitation_type.is_none());
        assert!(params.url.is_none());
    }

    #[test]
    fn waiting_state_queries() {
        let waiting = ElicitationWaitingState::Waiting;
        assert!(waiting.is_waiting());
        assert!(!waiting.is_completed());
        assert!(!waiting.is_expired());

        let completed = ElicitationWaitingState::Completed(Box::new(ElicitationResult::Decline));
        assert!(!completed.is_waiting());
        assert!(completed.is_completed());
        assert!(!completed.is_expired());

        let expired = ElicitationWaitingState::Expired;
        assert!(!expired.is_waiting());
        assert!(!expired.is_completed());
        assert!(expired.is_expired());
    }

    #[test]
    fn elicitation_result_action_tag() {
        let accept = ElicitationResult::Accept {
            content: json!("yes"),
        };
        let json_val = serde_json::to_value(&accept).expect("serialize");
        assert_eq!(json_val["action"], "accept");

        let decline = ElicitationResult::Decline;
        let json_val = serde_json::to_value(&decline).expect("serialize");
        assert_eq!(json_val["action"], "decline");

        let cancel = ElicitationResult::Cancel;
        let json_val = serde_json::to_value(&cancel).expect("serialize");
        assert_eq!(json_val["action"], "cancel");
    }

    // ── Enhanced tests ────────────────────────────────────────────────────

    #[test]
    fn elicitation_type_text_params() {
        let params = ElicitationParams::text("Enter your name");
        assert!(params.is_text_elicitation());
        assert!(!params.is_url_elicitation());
        assert!(params.url.is_none());
    }

    #[test]
    fn elicitation_type_url_params() {
        let params = ElicitationParams::url("Visit this page", "https://example.com/auth");
        assert!(params.is_url_elicitation());
        assert!(!params.is_text_elicitation());
        assert_eq!(params.url.as_deref(), Some("https://example.com/auth"));
    }

    #[test]
    fn elicitation_params_timeout() {
        let params = ElicitationParams {
            message: "test".to_owned(),
            requested_schema: None,
            elicitation_type: None,
            url: None,
            timeout_secs: Some(60),
        };
        assert_eq!(params.timeout(), Some(Duration::from_secs(60)));
    }

    #[test]
    fn elicitation_params_no_timeout() {
        let params = ElicitationParams::text("test");
        assert!(params.timeout().is_none());
    }

    #[test]
    fn elicitation_result_accept_text() {
        let result = ElicitationResult::accept_text("hello world");
        assert!(result.is_accepted());
        assert!(!result.is_declined());
        assert!(!result.is_cancelled());
        match result {
            ElicitationResult::Accept { content } => {
                assert_eq!(content, json!("hello world"));
            }
            _ => panic!("expected Accept"),
        }
    }

    #[test]
    fn elicitation_result_accept_url() {
        let result = ElicitationResult::accept_url("https://example.com/callback");
        assert!(result.is_accepted());
        match result {
            ElicitationResult::Accept { content } => {
                assert_eq!(content["url"], json!("https://example.com/callback"));
            }
            _ => panic!("expected Accept"),
        }
    }

    #[test]
    fn elicitation_result_queries() {
        assert!(ElicitationResult::Decline.is_declined());
        assert!(ElicitationResult::Cancel.is_cancelled());
        assert!(!ElicitationResult::Decline.is_cancelled());
        assert!(!ElicitationResult::Cancel.is_declined());
    }

    #[test]
    fn auto_cancel_handler() {
        let handler = AutoCancelElicitationHandler::new();
        let event = make_event("Should cancel");
        let result = handler.handle_elicitation(event);
        assert_eq!(result, ElicitationResult::Cancel);
    }

    #[test]
    fn timeout_handler_delegates_when_not_timed_out() {
        let inner = CallbackElicitationHandler::new(|_| ElicitationResult::accept_text("response"));
        let handler = TimeoutElicitationHandler::new(inner);
        let event = ElicitationRequestEvent::new(
            "srv",
            "req-1",
            ElicitationParams {
                message: "test".to_owned(),
                requested_schema: None,
                elicitation_type: None,
                url: None,
                timeout_secs: Some(3600), // 1 hour, won't time out
            },
        );
        let result = handler.handle_elicitation(event);
        assert!(result.is_accepted());
    }

    #[test]
    fn timeout_handler_cancels_when_timed_out() {
        let inner =
            CallbackElicitationHandler::new(|_| ElicitationResult::accept_text("should not reach"));
        let handler = TimeoutElicitationHandler::new(inner);
        let mut event = ElicitationRequestEvent::new(
            "srv",
            "req-1",
            ElicitationParams {
                message: "test".to_owned(),
                requested_schema: None,
                elicitation_type: None,
                url: None,
                timeout_secs: Some(0), // instant timeout
            },
        );
        // Force the created_at into the past
        event.created_at = Instant::now() - Duration::from_secs(1);
        let result = handler.handle_elicitation(event);
        assert!(result.is_cancelled());
    }

    #[test]
    fn event_is_timed_out_without_timeout() {
        let event = ElicitationRequestEvent::new("srv", "req-1", ElicitationParams::text("test"));
        assert!(!event.is_timed_out());
    }

    #[test]
    fn event_elapsed() {
        let event = ElicitationRequestEvent::new("srv", "req-1", ElicitationParams::text("test"));
        // Should be near-zero
        let elapsed = event.elapsed();
        assert!(elapsed < Duration::from_secs(1));
    }

    #[test]
    fn elicitation_type_serde_roundtrip() {
        let text = ElicitationType::Text;
        let json_str = serde_json::to_string(&text).expect("serialize");
        assert_eq!(json_str, "\"text\"");
        let back: ElicitationType = serde_json::from_str(&json_str).expect("deserialize");
        assert_eq!(back, ElicitationType::Text);

        let url = ElicitationType::Url;
        let json_str = serde_json::to_string(&url).expect("serialize");
        assert_eq!(json_str, "\"url\"");
    }

    #[test]
    fn event_new_constructor() {
        let event = ElicitationRequestEvent::new(
            "my-server",
            "req-42",
            ElicitationParams::url("Visit", "https://example.com"),
        );
        assert_eq!(event.server_name, "my-server");
        assert_eq!(event.request_id, "req-42");
        assert!(event.params.is_url_elicitation());
        assert!(event.waiting_state.is_waiting());
    }
}
