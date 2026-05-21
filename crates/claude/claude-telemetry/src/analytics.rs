//! Analytics event system for tracking usage patterns and feature adoption.
//!
//! Provides structured event recording with pluggable sinks (console, buffered)
//! and a simple feature-flag system for controlling analytics behaviour.

use parking_lot::Mutex;
use std::sync::Arc;

use anyhow::Result;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Analytics Event
// ---------------------------------------------------------------------------

/// Top-level analytics event types emitted by the runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AnalyticsEvent {
    /// A new session has started.
    SessionStart {
        /// Unique session identifier.
        session_id: String,
    },
    /// A tool was invoked.
    ToolUse {
        /// Name of the tool that was used.
        tool_name: String,
        /// Whether the tool execution succeeded.
        success: bool,
    },
    /// A query / conversation turn completed.
    QueryComplete {
        /// Number of tokens consumed by the query.
        tokens_used: u64,
        /// Wall-clock duration of the query in milliseconds.
        duration_ms: u64,
    },
    /// An error occurred.
    Error {
        /// Error category (e.g. "provider", "tool", "network").
        category: String,
        /// Short error message.
        message: String,
    },
    /// A context compaction occurred.
    Compact {
        /// Strategy used for compaction.
        strategy: String,
        /// Approximate tokens saved.
        tokens_saved: u64,
    },
    /// A permission decision was made.
    PermissionDecision {
        /// The tool or action that was evaluated.
        action: String,
        /// The decision outcome (e.g. "allow", "deny", "ask").
        decision: String,
    },
}

impl AnalyticsEvent {
    /// Returns a human-readable label for the event variant.
    #[must_use]
    pub fn event_label(&self) -> &'static str {
        match self {
            Self::SessionStart { .. } => "session_start",
            Self::ToolUse { .. } => "tool_use",
            Self::QueryComplete { .. } => "query_complete",
            Self::Error { .. } => "error",
            Self::Compact { .. } => "compact",
            Self::PermissionDecision { .. } => "permission_decision",
        }
    }
}

// ---------------------------------------------------------------------------
// Event Metadata
// ---------------------------------------------------------------------------

/// Metadata attached to every analytics event for enrichment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventMetadata {
    /// Unique session identifier.
    pub session_id: String,
    /// Model name (e.g. "claude-sonnet-4-20250514").
    pub model: String,
    /// Provider name (e.g. "anthropic", "openai").
    pub provider: String,
    /// Platform identifier (e.g. "windows", "macos", "linux").
    pub platform: String,
    /// Duration of the associated operation in milliseconds, if applicable.
    pub duration_ms: Option<u64>,
}

impl EventMetadata {
    /// Create metadata with the given session ID and sensible defaults.
    #[must_use]
    pub fn new(session_id: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            model: String::new(),
            provider: String::new(),
            platform: default_platform(),
            duration_ms: None,
        }
    }

    /// Set the model name.
    #[must_use]
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// Set the provider name.
    #[must_use]
    pub fn with_provider(mut self, provider: impl Into<String>) -> Self {
        self.provider = provider.into();
        self
    }

    /// Set the duration.
    #[must_use]
    pub fn with_duration(mut self, ms: u64) -> Self {
        self.duration_ms = Some(ms);
        self
    }
}

/// Detect the current platform string.
fn default_platform() -> String {
    if cfg!(target_os = "windows") {
        "windows".to_string()
    } else if cfg!(target_os = "macos") {
        "macos".to_string()
    } else if cfg!(target_os = "linux") {
        "linux".to_string()
    } else {
        "unknown".to_string()
    }
}

// ---------------------------------------------------------------------------
// AnalyticsSink trait
// ---------------------------------------------------------------------------

/// Trait for analytics event consumers (sinks).
pub trait AnalyticsSink: Send + Sync {
    /// Send a single event with optional metadata.
    fn send_event(&self, event: &AnalyticsEvent, metadata: &EventMetadata) -> Result<()>;

    /// Flush any buffered events.
    fn flush(&self) -> Result<()>;
}

// ---------------------------------------------------------------------------
// ConsoleSink
// ---------------------------------------------------------------------------

/// Analytics sink that writes events to the console (stdout).
#[derive(Debug, Default)]
pub struct ConsoleSink {
    /// Whether to include metadata in output.
    pub verbose: bool,
}

impl ConsoleSink {
    /// Create a new console sink.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Enable verbose output including metadata.
    #[must_use]
    pub fn with_verbose(mut self) -> Self {
        self.verbose = true;
        self
    }

    /// Format an event for console output.
    #[must_use]
    pub fn format_event(&self, event: &AnalyticsEvent, metadata: &EventMetadata) -> String {
        if self.verbose {
            format!(
                "[analytics] {} session={} model={} provider={} platform={}",
                event.event_label(),
                metadata.session_id,
                metadata.model,
                metadata.provider,
                metadata.platform,
            )
        } else {
            format!("[analytics] {}", event.event_label())
        }
    }
}

impl AnalyticsSink for ConsoleSink {
    fn send_event(&self, event: &AnalyticsEvent, metadata: &EventMetadata) -> Result<()> {
        let line = self.format_event(event, metadata);
        println!("{line}");
        Ok(())
    }

    fn flush(&self) -> Result<()> {
        // Console is unbuffered — nothing to flush.
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// BufferedSink
// ---------------------------------------------------------------------------

/// Analytics sink that buffers events and flushes them in batches.
pub struct BufferedSink {
    inner: Arc<dyn AnalyticsSink>,
    buffer: Mutex<Vec<(AnalyticsEvent, EventMetadata)>>,
    max_buffer_size: usize,
}

impl std::fmt::Debug for BufferedSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BufferedSink")
            .field("max_buffer_size", &self.max_buffer_size)
            .field("buffered_count", &self.buffered_count())
            .finish_non_exhaustive()
    }
}

impl BufferedSink {
    /// Create a new buffered sink wrapping `inner` with the given max buffer size.
    pub fn new(inner: Arc<dyn AnalyticsSink>, max_buffer_size: usize) -> Self {
        Self {
            inner,
            buffer: Mutex::new(Vec::new()),
            max_buffer_size,
        }
    }

    /// Returns the number of events currently buffered.
    #[must_use]
    pub fn buffered_count(&self) -> usize {
        self.buffer.lock().len()
    }

    /// Returns the configured max buffer size.
    #[must_use]
    pub fn max_buffer_size(&self) -> usize {
        self.max_buffer_size
    }
}

impl AnalyticsSink for BufferedSink {
    fn send_event(&self, event: &AnalyticsEvent, metadata: &EventMetadata) -> Result<()> {
        let mut buf = self.buffer.lock();
        buf.push((event.clone(), metadata.clone()));
        if buf.len() >= self.max_buffer_size {
            self.flush_locked(&mut buf)?;
        }
        Ok(())
    }

    fn flush(&self) -> Result<()> {
        let mut buf = self.buffer.lock();
        self.flush_locked(&mut buf)
    }
}

impl BufferedSink {
    /// Flush the buffer while the lock is already held.
    fn flush_locked(&self, buf: &mut Vec<(AnalyticsEvent, EventMetadata)>) -> Result<()> {
        for (event, metadata) in buf.drain(..) {
            self.inner.send_event(&event, &metadata)?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Feature Flags
// ---------------------------------------------------------------------------

/// Simple feature flag identifiers for analytics control.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeatureFlag {
    /// Enable analytics event collection.
    AnalyticsEnabled,
    /// Enable detailed tool-usage tracking.
    ToolUsageTracking,
    /// Enable performance metrics collection.
    PerformanceMetrics,
    /// Enable error reporting.
    ErrorReporting,
    /// Enable compact-related analytics.
    CompactAnalytics,
    /// Enable permission-decision tracking.
    PermissionTracking,
}

/// Check whether a feature flag is enabled.
///
/// Uses a simple environment-variable-based check:
/// `REMOTE_CODE_FEATURE_<FLAG>=true|1|yes`.
#[must_use]
pub fn is_feature_enabled(flag: FeatureFlag) -> bool {
    let key = format!(
        "REMOTE_CODE_FEATURE_{}",
        match flag {
            FeatureFlag::AnalyticsEnabled => "ANALYTICS_ENABLED",
            FeatureFlag::ToolUsageTracking => "TOOL_USAGE_TRACKING",
            FeatureFlag::PerformanceMetrics => "PERFORMANCE_METRICS",
            FeatureFlag::ErrorReporting => "ERROR_REPORTING",
            FeatureFlag::CompactAnalytics => "COMPACT_ANALYTICS",
            FeatureFlag::PermissionTracking => "PERMISSION_TRACKING",
        }
    );
    std::env::var(&key)
        .map(|v| matches!(v.to_lowercase().as_str(), "true" | "1" | "yes"))
        .unwrap_or(true) // default: enabled
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- AnalyticsEvent -------------------------------------------------------

    #[test]
    fn session_start_event_label() {
        let e = AnalyticsEvent::SessionStart {
            session_id: "abc".into(),
        };
        assert_eq!(e.event_label(), "session_start");
    }

    #[test]
    fn tool_use_event_label() {
        let e = AnalyticsEvent::ToolUse {
            tool_name: "bash".into(),
            success: true,
        };
        assert_eq!(e.event_label(), "tool_use");
    }

    #[test]
    fn query_complete_event_label() {
        let e = AnalyticsEvent::QueryComplete {
            tokens_used: 100,
            duration_ms: 500,
        };
        assert_eq!(e.event_label(), "query_complete");
    }

    #[test]
    fn error_event_label() {
        let e = AnalyticsEvent::Error {
            category: "network".into(),
            message: "timeout".into(),
        };
        assert_eq!(e.event_label(), "error");
    }

    #[test]
    fn compact_event_label() {
        let e = AnalyticsEvent::Compact {
            strategy: "full".into(),
            tokens_saved: 5000,
        };
        assert_eq!(e.event_label(), "compact");
    }

    #[test]
    fn permission_decision_event_label() {
        let e = AnalyticsEvent::PermissionDecision {
            action: "bash".into(),
            decision: "allow".into(),
        };
        assert_eq!(e.event_label(), "permission_decision");
    }

    #[test]
    fn analytics_event_serialization_roundtrip() {
        let events = vec![
            AnalyticsEvent::SessionStart {
                session_id: "s1".into(),
            },
            AnalyticsEvent::ToolUse {
                tool_name: "read".into(),
                success: false,
            },
            AnalyticsEvent::QueryComplete {
                tokens_used: 42,
                duration_ms: 100,
            },
            AnalyticsEvent::Error {
                category: "provider".into(),
                message: "rate limited".into(),
            },
            AnalyticsEvent::Compact {
                strategy: "micro".into(),
                tokens_saved: 999,
            },
            AnalyticsEvent::PermissionDecision {
                action: "write".into(),
                decision: "deny".into(),
            },
        ];
        for event in &events {
            let json = serde_json::to_string(event).expect("serialize");
            let back: AnalyticsEvent = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(*event, back);
        }
    }

    // -- EventMetadata ---------------------------------------------------------

    #[test]
    fn event_metadata_builder() {
        let meta = EventMetadata::new("session-1")
            .with_model("claude-sonnet-4-20250514")
            .with_provider("anthropic")
            .with_duration(1234);
        assert_eq!(meta.session_id, "session-1");
        assert_eq!(meta.model, "claude-sonnet-4-20250514");
        assert_eq!(meta.provider, "anthropic");
        assert_eq!(meta.duration_ms, Some(1234));
        // platform should be set
        assert!(!meta.platform.is_empty());
    }

    #[test]
    fn event_metadata_serialization() {
        let meta = EventMetadata::new("s");
        let json = serde_json::to_string(&meta).expect("serialize");
        let back: EventMetadata = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(meta.session_id, back.session_id);
    }

    // -- ConsoleSink -----------------------------------------------------------

    #[test]
    fn console_sink_format_non_verbose() {
        let sink = ConsoleSink::new();
        let meta = EventMetadata::new("s1");
        let event = AnalyticsEvent::SessionStart {
            session_id: "s1".into(),
        };
        let formatted = sink.format_event(&event, &meta);
        assert!(formatted.contains("[analytics] session_start"));
    }

    #[test]
    fn console_sink_format_verbose() {
        let sink = ConsoleSink::new().with_verbose();
        let meta = EventMetadata::new("s1")
            .with_model("test-model")
            .with_provider("test-provider");
        let event = AnalyticsEvent::ToolUse {
            tool_name: "bash".into(),
            success: true,
        };
        let formatted = sink.format_event(&event, &meta);
        assert!(formatted.contains("session=s1"));
        assert!(formatted.contains("model=test-model"));
        assert!(formatted.contains("provider=test-provider"));
    }

    #[test]
    fn console_sink_send_and_flush() {
        let sink = ConsoleSink::new();
        let meta = EventMetadata::new("s");
        let event = AnalyticsEvent::SessionStart {
            session_id: "s".into(),
        };
        assert!(sink.send_event(&event, &meta).is_ok());
        assert!(sink.flush().is_ok());
    }

    // -- BufferedSink ----------------------------------------------------------

    #[test]
    fn buffered_sink_buffers_events() {
        let inner = Arc::new(ConsoleSink::new());
        let buffered = BufferedSink::new(inner, 10);
        let meta = EventMetadata::new("s");

        buffered
            .send_event(
                &AnalyticsEvent::SessionStart {
                    session_id: "s".into(),
                },
                &meta,
            )
            .expect("send");
        assert_eq!(buffered.buffered_count(), 1);
    }

    #[test]
    fn buffered_sink_auto_flushes_at_capacity() {
        let inner = Arc::new(ConsoleSink::new());
        let buffered = BufferedSink::new(inner, 2);
        let meta = EventMetadata::new("s");

        buffered
            .send_event(
                &AnalyticsEvent::SessionStart {
                    session_id: "s".into(),
                },
                &meta,
            )
            .expect("send");
        assert_eq!(buffered.buffered_count(), 1);

        buffered
            .send_event(
                &AnalyticsEvent::ToolUse {
                    tool_name: "x".into(),
                    success: true,
                },
                &meta,
            )
            .expect("send");
        // auto-flush at capacity 2
        assert_eq!(buffered.buffered_count(), 0);
    }

    #[test]
    fn buffered_sink_manual_flush() {
        let inner = Arc::new(ConsoleSink::new());
        let buffered = BufferedSink::new(inner, 100);
        let meta = EventMetadata::new("s");

        buffered
            .send_event(
                &AnalyticsEvent::Error {
                    category: "test".into(),
                    message: "err".into(),
                },
                &meta,
            )
            .expect("send");
        assert_eq!(buffered.buffered_count(), 1);

        buffered.flush().expect("flush");
        assert_eq!(buffered.buffered_count(), 0);
    }

    // -- Feature Flags ---------------------------------------------------------

    #[test]
    fn feature_flag_default_enabled() {
        // Without setting env vars, features default to enabled.
        assert!(is_feature_enabled(FeatureFlag::AnalyticsEnabled));
        assert!(is_feature_enabled(FeatureFlag::ToolUsageTracking));
        assert!(is_feature_enabled(FeatureFlag::PerformanceMetrics));
        assert!(is_feature_enabled(FeatureFlag::ErrorReporting));
        assert!(is_feature_enabled(FeatureFlag::CompactAnalytics));
        assert!(is_feature_enabled(FeatureFlag::PermissionTracking));
    }

    #[test]
    fn feature_flag_parsing_logic() {
        // Test the underlying parsing logic without modifying env vars.
        // The default case (no env var set) returns true.
        assert!(matches!(
            "true".to_lowercase().as_str(),
            "true" | "1" | "yes"
        ));
        assert!(matches!("1".to_lowercase().as_str(), "true" | "1" | "yes"));
        assert!(matches!(
            "yes".to_lowercase().as_str(),
            "true" | "1" | "yes"
        ));
        assert!(!matches!(
            "false".to_lowercase().as_str(),
            "true" | "1" | "yes"
        ));
        assert!(!matches!("0".to_lowercase().as_str(), "true" | "1" | "yes"));
    }

    #[test]
    fn feature_flag_all_variants_covered() {
        // Ensure all feature flags can be checked without panic.
        let _ = is_feature_enabled(FeatureFlag::AnalyticsEnabled);
        let _ = is_feature_enabled(FeatureFlag::ToolUsageTracking);
        let _ = is_feature_enabled(FeatureFlag::PerformanceMetrics);
        let _ = is_feature_enabled(FeatureFlag::ErrorReporting);
        let _ = is_feature_enabled(FeatureFlag::CompactAnalytics);
        let _ = is_feature_enabled(FeatureFlag::PermissionTracking);
    }

    // -- default_platform ------------------------------------------------------

    #[test]
    fn default_platform_not_empty() {
        let p = default_platform();
        assert!(!p.is_empty());
        assert!(p == "windows" || p == "macos" || p == "linux" || p == "unknown");
    }
}
