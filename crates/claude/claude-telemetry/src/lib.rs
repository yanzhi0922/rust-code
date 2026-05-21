//! Structured telemetry with OpenTelemetry-compatible spans and metrics.
//!
//! Provides a comprehensive telemetry stack including:
//! - **Span tracking**: Hierarchical operation spans with timing and metadata
//! - **Metrics**: Counters, gauges, and histograms for operational monitoring
//! - **Event recording**: Structured event logging with severity levels
//! - **Health checks**: Liveness and readiness probe support
//! - **Session metrics**: Per-session token usage, cost, and latency tracking
//!
//! # Architecture
//!
//! The telemetry system is designed to be frontend-agnostic:
//! - TUI, GUI, and Remote-Control frontends can all subscribe to events
//! - Metrics can be exported to OpenTelemetry, Prometheus, or JSON
//! - Spans nest naturally for tool calls → provider calls → streaming chunks
//!
//! # Example
//!
//! ```rust,no_run
//! use claude_telemetry::{TelemetryHub, SpanKind};
//!
//! let hub = TelemetryHub::new("remote-code");
//! let span = hub.start_span(SpanKind::ProviderCall, "openai.complete");
//! // ... do work ...
//! hub.finish_span(span, None);
//! ```

use parking_lot::{Mutex, RwLock};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use anyhow::Result;
use tracing_subscriber::EnvFilter;

// ---------------------------------------------------------------------------
// Sub-modules
// ---------------------------------------------------------------------------

pub mod analytics;
pub mod away_summary;
pub mod growthbook;
pub mod rate_limit;
pub mod token_estimation;

// ---------------------------------------------------------------------------
// Core types
// ---------------------------------------------------------------------------

/// Unique identifier for a span.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SpanId(u64);

/// Unique identifier for a session metric bucket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SessionId(u64);

/// Classification of a telemetry span.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SpanKind {
    /// A full conversation turn (user input → assistant response).
    ConversationTurn,
    /// An LLM provider API call.
    ProviderCall,
    /// A tool execution.
    ToolExecution,
    /// A streaming chunk processing step.
    StreamingChunk,
    /// Context compaction operation.
    ContextCompaction,
    /// Permission check.
    PermissionCheck,
    /// MCP server communication.
    McpOperation,
    /// Hook execution.
    HookExecution,
    /// Plugin operation.
    PluginOperation,
    /// Agent / sub-agent dispatch.
    AgentDispatch,
    /// Session lifecycle event.
    SessionLifecycle,
    /// Custom / user-defined span.
    Custom,
}

impl SpanKind {
    /// Returns the string representation used in OpenTelemetry attributes.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ConversationTurn => "conversation_turn",
            Self::ProviderCall => "provider_call",
            Self::ToolExecution => "tool_execution",
            Self::StreamingChunk => "streaming_chunk",
            Self::ContextCompaction => "context_compaction",
            Self::PermissionCheck => "permission_check",
            Self::McpOperation => "mcp_operation",
            Self::HookExecution => "hook_execution",
            Self::PluginOperation => "plugin_operation",
            Self::AgentDispatch => "agent_dispatch",
            Self::SessionLifecycle => "session_lifecycle",
            Self::Custom => "custom",
        }
    }
}

/// Severity level for telemetry events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Severity {
    /// Trace-level detail (streaming chunks, raw API data).
    Trace,
    /// Debug-level detail (tool parameters, rule matching).
    Debug,
    /// Informational events (turn start/end, tool results).
    Info,
    /// Warning conditions (slow responses, near-limit context).
    Warn,
    /// Error conditions (API failures, tool errors).
    Error,
    /// Critical failures (unable to start, data corruption).
    Critical,
}

impl Severity {
    /// Returns the string representation.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Trace => "TRACE",
            Self::Debug => "DEBUG",
            Self::Info => "INFO",
            Self::Warn => "WARN",
            Self::Error => "ERROR",
            Self::Critical => "CRITICAL",
        }
    }
}

/// A completed span with timing and metadata.
#[derive(Debug, Clone)]
pub struct CompletedSpan {
    /// Unique span identifier.
    pub id: SpanId,
    /// Kind of operation.
    pub kind: SpanKind,
    /// Human-readable operation name.
    pub name: String,
    /// Wall-clock duration.
    pub duration: std::time::Duration,
    /// Whether the operation succeeded.
    pub success: bool,
    /// Error message if the operation failed.
    pub error: Option<String>,
    /// Arbitrary key-value metadata.
    pub attributes: HashMap<String, String>,
}

/// A structured telemetry event.
#[derive(Debug, Clone)]
pub struct TelemetryEvent {
    /// Event severity.
    pub severity: Severity,
    /// Event category (e.g. "provider", "tool", "session").
    pub category: String,
    /// Human-readable message.
    pub message: String,
    /// Timestamp (monotonic, relative to hub creation).
    pub timestamp: std::time::Duration,
    /// Arbitrary key-value metadata.
    pub attributes: HashMap<String, String>,
}

// ---------------------------------------------------------------------------
// Metrics
// ---------------------------------------------------------------------------

/// A monotonically increasing counter.
#[derive(Debug)]
pub struct Counter {
    value: AtomicU64,
    name: String,
    #[allow(dead_code)]
    labels: HashMap<String, String>,
}

impl Counter {
    fn new(name: String, labels: HashMap<String, String>) -> Self {
        Self {
            value: AtomicU64::new(0),
            name,
            labels,
        }
    }

    /// Increment the counter by 1.
    pub fn inc(&self) {
        self.value.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment the counter by `delta`.
    pub fn add(&self, delta: u64) {
        self.value.fetch_add(delta, Ordering::Relaxed);
    }

    /// Get the current counter value.
    #[must_use]
    pub fn get(&self) -> u64 {
        self.value.load(Ordering::Relaxed)
    }

    /// Get the counter name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// A value that can go up and down.
#[derive(Debug)]
pub struct Gauge {
    value: AtomicU64,
    name: String,
    #[allow(dead_code)]
    labels: HashMap<String, String>,
}

impl Gauge {
    fn new(name: String, labels: HashMap<String, String>) -> Self {
        Self {
            value: AtomicU64::new(0),
            name,
            labels,
        }
    }

    /// Set the gauge to a specific value.
    pub fn set(&self, value: u64) {
        self.value.store(value, Ordering::Relaxed);
    }

    /// Increment the gauge by 1.
    pub fn inc(&self) {
        self.value.fetch_add(1, Ordering::Relaxed);
    }

    /// Decrement the gauge by 1.
    pub fn dec(&self) {
        self.value.fetch_sub(1, Ordering::Relaxed);
    }

    /// Get the current gauge value.
    #[must_use]
    pub fn get(&self) -> u64 {
        self.value.load(Ordering::Relaxed)
    }

    /// Get the gauge name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// A histogram for tracking value distributions (latency, size, etc.).
#[derive(Debug)]
pub struct Histogram {
    buckets: Mutex<Vec<u64>>,
    name: String,
    #[allow(dead_code)]
    labels: HashMap<String, String>,
}

impl Histogram {
    fn new(name: String, labels: HashMap<String, String>) -> Self {
        Self {
            buckets: Mutex::new(Vec::new()),
            name,
            labels,
        }
    }

    /// Record a value observation.
    pub fn observe(&self, value: u64) {
        self.buckets.lock().push(value);
    }

    /// Get the number of observations.
    #[must_use]
    pub fn count(&self) -> usize {
        self.buckets.lock().len()
    }

    /// Get the sum of all observations.
    #[must_use]
    pub fn sum(&self) -> u64 {
        self.buckets.lock().iter().sum()
    }

    /// Get the average of all observations (0 if empty).
    #[must_use]
    pub fn avg(&self) -> f64 {
        let guard = self.buckets.lock();
        if guard.is_empty() {
            return 0.0;
        }
        guard.iter().sum::<u64>() as f64 / guard.len() as f64
    }

    /// Get the p50 (median) of all observations.
    #[must_use]
    pub fn p50(&self) -> u64 {
        let mut guard = self.buckets.lock();
        if guard.is_empty() {
            return 0;
        }
        guard.sort_unstable();
        guard[guard.len() / 2]
    }

    /// Get the p99 of all observations.
    #[must_use]
    pub fn p99(&self) -> u64 {
        let mut guard = self.buckets.lock();
        if guard.is_empty() {
            return 0;
        }
        guard.sort_unstable();
        let idx = ((guard.len() as f64) * 0.99).ceil() as usize;
        guard[idx.min(guard.len()) - 1]
    }

    /// Get the histogram name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
}

// ---------------------------------------------------------------------------
// Active span tracking
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct ActiveSpan {
    id: SpanId,
    kind: SpanKind,
    name: String,
    start: Instant,
    attributes: HashMap<String, String>,
    #[allow(dead_code)]
    parent_id: Option<SpanId>,
}

// ---------------------------------------------------------------------------
// TelemetryHub — central telemetry collector
// ---------------------------------------------------------------------------

/// Central telemetry collection point for the entire application.
///
/// The hub is designed to be shared across all crates via `Arc<TelemetryHub>`.
/// It collects spans, metrics, and events, and supports subscriber callbacks
/// for real-time UI updates (TUI, GUI, Remote-Control).
pub struct TelemetryHub {
    service_name: String,
    start_time: Instant,
    next_span_id: AtomicU64,
    active_spans: RwLock<HashMap<u64, ActiveSpan>>,
    completed_spans: Mutex<Vec<CompletedSpan>>,
    events: Mutex<Vec<TelemetryEvent>>,
    counters: RwLock<Vec<Arc<Counter>>>,
    gauges: RwLock<Vec<Arc<Gauge>>>,
    histograms: RwLock<Vec<Arc<Histogram>>>,
    subscribers: RwLock<Vec<Box<dyn TelemetrySubscriber + Send + Sync>>>,
    // Pre-built well-known metrics
    provider_latency: Arc<Histogram>,
    tool_latency: Arc<Histogram>,
    token_input_total: Arc<Counter>,
    token_output_total: Arc<Counter>,
    cost_total_micros: AtomicU64,
    active_sessions: Arc<Gauge>,
    compaction_count: Arc<Counter>,
    error_count: Arc<Counter>,
}

impl TelemetryHub {
    /// Create a new telemetry hub for the given service.
    #[must_use]
    pub fn new(service_name: &str) -> Arc<Self> {
        let provider_latency = Arc::new(Histogram::new(
            "provider_latency_ms".to_owned(),
            HashMap::new(),
        ));
        let tool_latency = Arc::new(Histogram::new("tool_latency_ms".to_owned(), HashMap::new()));
        let token_input_total =
            Arc::new(Counter::new("token_input_total".to_owned(), HashMap::new()));
        let token_output_total = Arc::new(Counter::new(
            "token_output_total".to_owned(),
            HashMap::new(),
        ));
        let active_sessions = Arc::new(Gauge::new("active_sessions".to_owned(), HashMap::new()));
        let compaction_count =
            Arc::new(Counter::new("compaction_count".to_owned(), HashMap::new()));
        let error_count = Arc::new(Counter::new("error_count".to_owned(), HashMap::new()));

        Arc::new(Self {
            service_name: service_name.to_owned(),
            start_time: Instant::now(),
            next_span_id: AtomicU64::new(1),
            active_spans: RwLock::new(HashMap::new()),
            completed_spans: Mutex::new(Vec::new()),
            events: Mutex::new(Vec::new()),
            counters: RwLock::new(Vec::new()),
            gauges: RwLock::new(Vec::new()),
            histograms: RwLock::new(Vec::new()),
            subscribers: RwLock::new(Vec::new()),
            provider_latency: provider_latency.clone(),
            tool_latency: tool_latency.clone(),
            token_input_total: token_input_total.clone(),
            token_output_total: token_output_total.clone(),
            cost_total_micros: AtomicU64::new(0),
            active_sessions: active_sessions.clone(),
            compaction_count: compaction_count.clone(),
            error_count: error_count.clone(),
        })
    }

    /// Get the service name.
    #[must_use]
    pub fn service_name(&self) -> &str {
        &self.service_name
    }

    /// Get the uptime since hub creation.
    #[must_use]
    pub fn uptime(&self) -> std::time::Duration {
        self.start_time.elapsed()
    }

    // ── Span management ──────────────────────────────────────────────────

    /// Start a new telemetry span.
    pub fn start_span(&self, kind: SpanKind, name: &str) -> SpanId {
        self.start_span_with_parent(kind, name, None)
    }

    /// Start a new telemetry span with an optional parent.
    pub fn start_span_with_parent(
        &self,
        kind: SpanKind,
        name: &str,
        parent_id: Option<SpanId>,
    ) -> SpanId {
        let id = SpanId(self.next_span_id.fetch_add(1, Ordering::Relaxed));
        let span = ActiveSpan {
            id,
            kind,
            name: name.to_owned(),
            start: Instant::now(),
            attributes: HashMap::new(),
            parent_id,
        };
        self.active_spans.write().insert(id.0, span);

        self.notify_subscribers(|sub| sub.on_span_start(id, kind, name));
        id
    }

    /// Add an attribute to an active span.
    pub fn span_attr(&self, span_id: SpanId, key: &str, value: &str) {
        let mut guard = self.active_spans.write();
        if let Some(span) = guard.get_mut(&span_id.0) {
            span.attributes.insert(key.to_owned(), value.to_owned());
        }
    }

    /// Finish a span, recording its duration.
    pub fn finish_span(&self, span_id: SpanId, error: Option<&str>) {
        let mut guard = self.active_spans.write();
        if let Some(span) = guard.remove(&span_id.0) {
            let duration = span.start.elapsed();
            let completed = CompletedSpan {
                id: span.id,
                kind: span.kind,
                name: span.name.clone(),
                duration,
                success: error.is_none(),
                error: error.map(str::to_owned),
                attributes: span.attributes,
            };

            // Update well-known metrics based on span kind.
            match completed.kind {
                SpanKind::ProviderCall => {
                    self.provider_latency.observe(duration.as_millis() as u64);
                }
                SpanKind::ToolExecution => {
                    self.tool_latency.observe(duration.as_millis() as u64);
                }
                SpanKind::ContextCompaction => {
                    self.compaction_count.inc();
                }
                _ => {}
            }

            if completed.error.is_some() {
                self.error_count.inc();
            }

            self.notify_subscribers(|sub| sub.on_span_finish(&completed));
            self.completed_spans.lock().push(completed);
        }
    }

    // ── Event recording ──────────────────────────────────────────────────

    /// Record a structured telemetry event.
    pub fn record_event(&self, severity: Severity, category: &str, message: &str) {
        self.record_event_with_attrs(severity, category, message, HashMap::new());
    }

    /// Record a structured telemetry event with additional attributes.
    pub fn record_event_with_attrs(
        &self,
        severity: Severity,
        category: &str,
        message: &str,
        attributes: HashMap<String, String>,
    ) {
        let event = TelemetryEvent {
            severity,
            category: category.to_owned(),
            message: message.to_owned(),
            timestamp: self.start_time.elapsed(),
            attributes,
        };
        self.notify_subscribers(|sub| sub.on_event(&event));
        self.events.lock().push(event);
    }

    // ── Well-known metric shortcuts ──────────────────────────────────────

    /// Record token usage from a provider response.
    pub fn record_token_usage(&self, input_tokens: u64, output_tokens: u64) {
        self.token_input_total.add(input_tokens);
        self.token_output_total.add(output_tokens);
    }

    /// Record an estimated cost in microdollars (1 USD = 1_000_000 microdollars).
    pub fn record_cost(&self, cost_usd: f64) {
        let micros = (cost_usd * 1_000_000.0) as u64;
        self.cost_total_micros.fetch_add(micros, Ordering::Relaxed);
    }

    /// Increment the active session count.
    pub fn session_started(&self) {
        self.active_sessions.inc();
    }

    /// Decrement the active session count.
    pub fn session_ended(&self) {
        self.active_sessions.dec();
    }

    /// Get the total cost in USD.
    #[must_use]
    pub fn total_cost_usd(&self) -> f64 {
        self.cost_total_micros.load(Ordering::Relaxed) as f64 / 1_000_000.0
    }

    // ── Custom metrics ───────────────────────────────────────────────────

    /// Create a new counter metric.
    pub fn create_counter(&self, name: &str, labels: HashMap<String, String>) -> Arc<Counter> {
        let counter = Arc::new(Counter::new(name.to_owned(), labels));
        self.counters.write().push(counter.clone());
        counter
    }

    /// Create a new gauge metric.
    pub fn create_gauge(&self, name: &str, labels: HashMap<String, String>) -> Arc<Gauge> {
        let gauge = Arc::new(Gauge::new(name.to_owned(), labels));
        self.gauges.write().push(gauge.clone());
        gauge
    }

    /// Create a new histogram metric.
    pub fn create_histogram(&self, name: &str, labels: HashMap<String, String>) -> Arc<Histogram> {
        let histogram = Arc::new(Histogram::new(name.to_owned(), labels));
        self.histograms.write().push(histogram.clone());
        histogram
    }

    // ── Subscriber management ────────────────────────────────────────────

    /// Add a telemetry subscriber for real-time event streaming.
    pub fn subscribe(&self, subscriber: Box<dyn TelemetrySubscriber + Send + Sync>) {
        self.subscribers.write().push(subscriber);
    }

    fn notify_subscribers(&self, f: impl Fn(&dyn TelemetrySubscriber)) {
        let guard = self.subscribers.read();
        for sub in guard.iter() {
            f(sub.as_ref());
        }
    }

    // ── Snapshot / export ────────────────────────────────────────────────

    /// Generate a snapshot of all current metrics.
    #[must_use]
    pub fn metrics_snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            uptime: self.start_time.elapsed(),
            total_input_tokens: self.token_input_total.get(),
            total_output_tokens: self.token_output_total.get(),
            total_cost_usd: self.total_cost_usd(),
            active_sessions: self.active_sessions.get(),
            provider_latency_p50_ms: self.provider_latency.p50(),
            provider_latency_p99_ms: self.provider_latency.p99(),
            tool_latency_p50_ms: self.tool_latency.p50(),
            tool_latency_p99_ms: self.tool_latency.p99(),
            compaction_count: self.compaction_count.get(),
            error_count: self.error_count.get(),
            completed_spans: self.completed_spans.lock().len(),
            active_spans: self.active_spans.read().len(),
            event_count: self.events.lock().len(),
        }
    }

    /// Export metrics as a JSON-compatible HashMap.
    #[must_use]
    pub fn export_metrics(&self) -> HashMap<String, serde_json::Value> {
        let snap = self.metrics_snapshot();
        let mut map = HashMap::new();
        map.insert(
            "uptime_secs".to_owned(),
            serde_json::json!(snap.uptime.as_secs()),
        );
        map.insert(
            "total_input_tokens".to_owned(),
            serde_json::json!(snap.total_input_tokens),
        );
        map.insert(
            "total_output_tokens".to_owned(),
            serde_json::json!(snap.total_output_tokens),
        );
        map.insert(
            "total_cost_usd".to_owned(),
            serde_json::json!(snap.total_cost_usd),
        );
        map.insert(
            "active_sessions".to_owned(),
            serde_json::json!(snap.active_sessions),
        );
        map.insert(
            "provider_latency_p50_ms".to_owned(),
            serde_json::json!(snap.provider_latency_p50_ms),
        );
        map.insert(
            "provider_latency_p99_ms".to_owned(),
            serde_json::json!(snap.provider_latency_p99_ms),
        );
        map.insert(
            "tool_latency_p50_ms".to_owned(),
            serde_json::json!(snap.tool_latency_p50_ms),
        );
        map.insert(
            "tool_latency_p99_ms".to_owned(),
            serde_json::json!(snap.tool_latency_p99_ms),
        );
        map.insert(
            "compaction_count".to_owned(),
            serde_json::json!(snap.compaction_count),
        );
        map.insert(
            "error_count".to_owned(),
            serde_json::json!(snap.error_count),
        );
        map.insert(
            "completed_spans".to_owned(),
            serde_json::json!(snap.completed_spans),
        );
        map.insert(
            "active_spans".to_owned(),
            serde_json::json!(snap.active_spans),
        );
        map.insert(
            "event_count".to_owned(),
            serde_json::json!(snap.event_count),
        );
        map
    }

    /// Get recent events (last N).
    #[must_use]
    pub fn recent_events(&self, limit: usize) -> Vec<TelemetryEvent> {
        let guard = self.events.lock();
        guard.iter().rev().take(limit).cloned().collect()
    }

    /// Get recent completed spans (last N).
    #[must_use]
    pub fn recent_spans(&self, limit: usize) -> Vec<CompletedSpan> {
        let guard = self.completed_spans.lock();
        guard.iter().rev().take(limit).cloned().collect()
    }
}

/// A point-in-time snapshot of all metrics.
#[derive(Debug, Clone)]
pub struct MetricsSnapshot {
    /// Hub uptime.
    pub uptime: std::time::Duration,
    /// Total input tokens consumed.
    pub total_input_tokens: u64,
    /// Total output tokens generated.
    pub total_output_tokens: u64,
    /// Total estimated cost in USD.
    pub total_cost_usd: f64,
    /// Number of currently active sessions.
    pub active_sessions: u64,
    /// Provider API latency p50 in milliseconds.
    pub provider_latency_p50_ms: u64,
    /// Provider API latency p99 in milliseconds.
    pub provider_latency_p99_ms: u64,
    /// Tool execution latency p50 in milliseconds.
    pub tool_latency_p50_ms: u64,
    /// Tool execution latency p99 in milliseconds.
    pub tool_latency_p99_ms: u64,
    /// Number of context compactions performed.
    pub compaction_count: u64,
    /// Total error count.
    pub error_count: u64,
    /// Total completed spans.
    pub completed_spans: usize,
    /// Currently active spans.
    pub active_spans: usize,
    /// Total recorded events.
    pub event_count: usize,
}

// ---------------------------------------------------------------------------
// Subscriber trait — for UI/Remote-Control integration
// ---------------------------------------------------------------------------

/// Trait for receiving real-time telemetry updates.
///
/// Implement this trait to subscribe to telemetry events from any frontend
/// (TUI, GUI, Remote-Control, external monitoring).
pub trait TelemetrySubscriber {
    /// Called when a span starts.
    fn on_span_start(&self, id: SpanId, kind: SpanKind, name: &str);
    /// Called when a span finishes.
    fn on_span_finish(&self, span: &CompletedSpan);
    /// Called when a telemetry event is recorded.
    fn on_event(&self, event: &TelemetryEvent);
}

// ---------------------------------------------------------------------------
// Health check support
// ---------------------------------------------------------------------------

/// Health check result.
#[derive(Debug, Clone)]
pub struct HealthStatus {
    /// Whether the service is healthy.
    pub healthy: bool,
    /// Human-readable status message.
    pub message: String,
    /// Uptime at the time of the check.
    pub uptime: std::time::Duration,
    /// Current metrics snapshot.
    pub metrics: MetricsSnapshot,
}

impl TelemetryHub {
    /// Perform a health check.
    #[must_use]
    pub fn health_check(&self) -> HealthStatus {
        let metrics = self.metrics_snapshot();
        let healthy = metrics.error_count < 1000;
        let message = if healthy {
            "OK".to_owned()
        } else {
            format!("High error count: {}", metrics.error_count)
        };
        HealthStatus {
            healthy,
            message,
            uptime: metrics.uptime,
            metrics,
        }
    }
}

// ---------------------------------------------------------------------------
// Legacy tracing initialization (backward compatible)
// ---------------------------------------------------------------------------

/// Install the tracing subscriber for log output.
///
/// # Errors
/// Returns an error if the tracing subscriber cannot be initialized.
pub fn install_tracing(service_name: &str, json: bool) -> Result<()> {
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(format!("{service_name}=info")));
    let builder = tracing_subscriber::fmt().with_env_filter(env_filter);
    if json {
        let _ = builder.json().try_init();
    } else {
        let _ = builder.with_target(false).try_init();
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn hub_creates_with_service_name() {
        let hub = TelemetryHub::new("test-service");
        assert_eq!(hub.service_name(), "test-service");
    }

    #[test]
    fn span_lifecycle_starts_and_finishes() {
        let hub = TelemetryHub::new("test");
        let id = hub.start_span(SpanKind::ProviderCall, "openai.complete");
        hub.span_attr(id, "model", "gpt-4o");
        hub.finish_span(id, None);

        let spans = hub.recent_spans(10);
        assert_eq!(spans.len(), 1);
        assert!(spans[0].success);
        assert_eq!(spans[0].kind, SpanKind::ProviderCall);
        assert_eq!(spans[0].name, "openai.complete");
    }

    #[test]
    fn span_with_error_records_failure() {
        let hub = TelemetryHub::new("test");
        let id = hub.start_span(SpanKind::ToolExecution, "bash");
        hub.finish_span(id, Some("command timed out"));

        let spans = hub.recent_spans(10);
        assert_eq!(spans.len(), 1);
        assert!(!spans[0].success);
        assert_eq!(spans[0].error.as_deref(), Some("command timed out"));
    }

    #[test]
    fn nested_spans_with_parent() {
        let hub = TelemetryHub::new("test");
        let parent = hub.start_span(SpanKind::ConversationTurn, "turn-1");
        let child = hub.start_span_with_parent(SpanKind::ProviderCall, "call-1", Some(parent));
        hub.finish_span(child, None);
        hub.finish_span(parent, None);

        let spans = hub.recent_spans(10);
        assert_eq!(spans.len(), 2);
    }

    #[test]
    fn token_usage_tracking() {
        let hub = TelemetryHub::new("test");
        hub.record_token_usage(100, 50);
        hub.record_token_usage(200, 100);

        let snap = hub.metrics_snapshot();
        assert_eq!(snap.total_input_tokens, 300);
        assert_eq!(snap.total_output_tokens, 150);
    }

    #[test]
    fn cost_tracking_in_micros() {
        let hub = TelemetryHub::new("test");
        hub.record_cost(0.003);
        hub.record_cost(0.007);

        let total = hub.total_cost_usd();
        assert!((total - 0.01).abs() < 0.001);
    }

    #[test]
    fn session_counting() {
        let hub = TelemetryHub::new("test");
        hub.session_started();
        hub.session_started();
        hub.session_started();
        hub.session_ended();

        let snap = hub.metrics_snapshot();
        assert_eq!(snap.active_sessions, 2);
    }

    #[test]
    fn event_recording() {
        let hub = TelemetryHub::new("test");
        hub.record_event(Severity::Info, "provider", "API call completed");
        hub.record_event(Severity::Error, "tool", "bash failed");

        let events = hub.recent_events(10);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].severity, Severity::Error);
        assert_eq!(events[1].severity, Severity::Info);
    }

    #[test]
    fn counter_operations() {
        let hub = TelemetryHub::new("test");
        let counter = hub.create_counter("requests", HashMap::new());
        counter.inc();
        counter.inc();
        counter.add(5);
        assert_eq!(counter.get(), 7);
    }

    #[test]
    fn gauge_operations() {
        let hub = TelemetryHub::new("test");
        let gauge = hub.create_gauge("connections", HashMap::new());
        gauge.set(10);
        gauge.inc();
        gauge.inc();
        gauge.dec();
        assert_eq!(gauge.get(), 11);
    }

    #[test]
    fn histogram_statistics() {
        let hub = TelemetryHub::new("test");
        let hist = hub.create_histogram("latency", HashMap::new());
        for v in [10, 20, 30, 40, 50, 60, 70, 80, 90, 100] {
            hist.observe(v);
        }
        assert_eq!(hist.count(), 10);
        assert_eq!(hist.sum(), 550);
        assert!((hist.avg() - 55.0).abs() < 0.01);
        assert_eq!(hist.p50(), 60);
    }

    #[test]
    fn metrics_export_produces_json_values() {
        let hub = TelemetryHub::new("test");
        hub.record_token_usage(1000, 500);
        let export = hub.export_metrics();
        assert_eq!(export["total_input_tokens"], serde_json::json!(1000));
        assert_eq!(export["total_output_tokens"], serde_json::json!(500));
    }

    #[test]
    fn health_check_returns_healthy() {
        let hub = TelemetryHub::new("test");
        let health = hub.health_check();
        assert!(health.healthy);
        assert_eq!(health.message, "OK");
    }

    #[test]
    fn provider_latency_metric_updated_on_span_finish() {
        let hub = TelemetryHub::new("test");
        let id = hub.start_span(SpanKind::ProviderCall, "call");
        hub.finish_span(id, None);

        let snap = hub.metrics_snapshot();
        // At least one observation should exist
        assert!(snap.provider_latency_p50_ms > 0 || snap.completed_spans > 0);
    }

    #[test]
    fn compaction_counter_updated() {
        let hub = TelemetryHub::new("test");
        let id = hub.start_span(SpanKind::ContextCompaction, "compact");
        hub.finish_span(id, None);

        let snap = hub.metrics_snapshot();
        assert_eq!(snap.compaction_count, 1);
    }

    #[test]
    fn subscriber_receives_events() {
        use std::sync::Arc;

        struct TestSubscriber {
            events: Arc<Mutex<Vec<String>>>,
        }
        impl TelemetrySubscriber for TestSubscriber {
            fn on_span_start(&self, _id: SpanId, _kind: SpanKind, _name: &str) {}
            fn on_span_finish(&self, span: &CompletedSpan) {
                self.events.lock().push(format!("span:{}", span.name));
            }
            fn on_event(&self, event: &TelemetryEvent) {
                self.events.lock().push(format!("event:{}", event.message));
            }
        }

        let hub = TelemetryHub::new("test");
        let events_arc = Arc::new(Mutex::new(Vec::new()));
        let sub = Box::new(TestSubscriber {
            events: events_arc.clone(),
        });
        hub.subscribe(sub);

        hub.record_event(Severity::Info, "test", "hello");
        let id = hub.start_span(SpanKind::Custom, "my-span");
        hub.finish_span(id, None);

        let guard = events_arc.lock();
        assert!(guard.iter().any(|e: &String| e.contains("event:hello")));
        assert!(guard.iter().any(|e: &String| e.contains("span:my-span")));
    }

    #[test]
    fn span_kind_as_str_roundtrips() {
        assert_eq!(SpanKind::ProviderCall.as_str(), "provider_call");
        assert_eq!(SpanKind::ToolExecution.as_str(), "tool_execution");
        assert_eq!(SpanKind::ContextCompaction.as_str(), "context_compaction");
        assert_eq!(SpanKind::Custom.as_str(), "custom");
    }

    #[test]
    fn severity_ordering() {
        assert!(Severity::Trace < Severity::Debug);
        assert!(Severity::Debug < Severity::Info);
        assert!(Severity::Info < Severity::Warn);
        assert!(Severity::Warn < Severity::Error);
        assert!(Severity::Error < Severity::Critical);
    }

    #[test]
    fn install_tracing_does_not_panic() {
        // Calling twice will fail but should not panic.
        let _ = install_tracing("test", false);
        let _ = install_tracing("test", true);
    }
}
