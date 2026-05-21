//! Workload Context for API request routing.
//!
//! Provides types and utilities for tagging API requests with workload
//! information, enabling the API to route requests to appropriate backends.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

// ---------------------------------------------------------------------------
// WorkloadType enum
// ---------------------------------------------------------------------------

/// The type of workload for request routing.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum WorkloadType {
    /// Default workload for standard queries.
    #[default]
    Default,
    /// Code review workload.
    CodeReview,
    /// Conversation compaction workload.
    Compact,
    /// Agent workload for sub-agent queries.
    Agent,
}

impl WorkloadType {
    /// Return the wire representation for the API.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::CodeReview => "code_review",
            Self::Compact => "compact",
            Self::Agent => "agent",
        }
    }

    /// Parse a workload type from its wire representation.
    pub fn from_str_opt(s: &str) -> Option<Self> {
        match s {
            "default" => Some(Self::Default),
            "code_review" => Some(Self::CodeReview),
            "compact" => Some(Self::Compact),
            "agent" => Some(Self::Agent),
            _ => None,
        }
    }
}

impl std::fmt::Display for WorkloadType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ---------------------------------------------------------------------------
// WorkloadContext
// ---------------------------------------------------------------------------

/// Context for workload-based request routing.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkloadContext {
    /// The workload type.
    pub workload_type: WorkloadType,
    /// Optional priority level (0 = normal, higher = more important).
    #[serde(default)]
    pub priority: u32,
    /// Optional routing hint for the backend.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routing_hint: Option<String>,
    /// Whether this is a long-running workload.
    #[serde(default)]
    pub long_running: bool,
}

impl Default for WorkloadContext {
    fn default() -> Self {
        Self {
            workload_type: WorkloadType::Default,
            priority: 0,
            routing_hint: None,
            long_running: false,
        }
    }
}

impl WorkloadContext {
    /// Create a new workload context with the given type.
    #[must_use]
    pub fn new(workload_type: WorkloadType) -> Self {
        Self {
            workload_type,
            ..Self::default()
        }
    }

    /// Set the priority level.
    #[must_use]
    pub fn with_priority(mut self, priority: u32) -> Self {
        self.priority = priority;
        self
    }

    /// Set the routing hint.
    #[must_use]
    pub fn with_routing_hint(mut self, hint: String) -> Self {
        self.routing_hint = Some(hint);
        self
    }

    /// Mark as long-running.
    #[must_use]
    pub fn with_long_running(mut self) -> Self {
        self.long_running = true;
        self
    }
}

// ---------------------------------------------------------------------------
// Header generation
// ---------------------------------------------------------------------------

/// HTTP header name for workload routing.
pub const WORKLOAD_HEADER: &str = "x-cc-workload";

/// Generate the workload routing header value.
///
/// The header value encodes the workload type and optional parameters.
///
/// # Arguments
///
/// * `ctx` — The workload context.
///
/// # Returns
///
/// The header value string.
#[must_use]
pub fn workload_header(ctx: &WorkloadContext) -> String {
    let mut parts = vec![format!("type={}", ctx.workload_type.as_str())];

    if ctx.priority > 0 {
        parts.push(format!("priority={}", ctx.priority));
    }
    if let Some(ref hint) = ctx.routing_hint {
        parts.push(format!("routing_hint={hint}"));
    }
    if ctx.long_running {
        parts.push("long_running=true".to_string());
    }

    parts.join(";")
}

/// Parse a workload header value back into a context.
///
/// # Arguments
///
/// * `header` — The header value string.
///
/// # Returns
///
/// The parsed `WorkloadContext`, or `None` if invalid.
pub fn parse_workload_header(header: &str) -> Option<WorkloadContext> {
    let parts: Vec<&str> = header.split(';').collect();
    let mut workload_type = None;
    let mut priority = 0u32;
    let mut routing_hint = None;
    let mut long_running = false;

    for part in parts {
        let part = part.trim();
        if let Some((key, value)) = part.split_once('=') {
            match key {
                "type" => workload_type = WorkloadType::from_str_opt(value),
                "priority" => priority = value.parse::<u32>().unwrap_or(0),
                "routing_hint" => routing_hint = Some(value.to_string()),
                "long_running" => long_running = value == "true",
                _ => {}
            }
        }
    }

    workload_type.map(|wt| WorkloadContext {
        workload_type: wt,
        priority,
        routing_hint,
        long_running,
    })
}

/// Convert a workload context to a JSON value for API body parameters.
///
/// # Arguments
///
/// * `ctx` — The workload context.
///
/// # Returns
///
/// A JSON object with the workload metadata.
#[must_use]
pub fn workload_to_json(ctx: &WorkloadContext) -> Value {
    let mut obj = json!({
        "workload_type": ctx.workload_type.as_str(),
    });
    if ctx.priority > 0 {
        obj["priority"] = json!(ctx.priority);
    }
    if let Some(ref hint) = ctx.routing_hint {
        obj["routing_hint"] = json!(hint);
    }
    if ctx.long_running {
        obj["long_running"] = json!(true);
    }
    obj
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- WorkloadType ---

    #[test]
    fn workload_type_as_str() {
        assert_eq!(WorkloadType::Default.as_str(), "default");
        assert_eq!(WorkloadType::CodeReview.as_str(), "code_review");
        assert_eq!(WorkloadType::Compact.as_str(), "compact");
        assert_eq!(WorkloadType::Agent.as_str(), "agent");
    }

    #[test]
    fn workload_type_display() {
        assert_eq!(WorkloadType::Default.to_string(), "default");
    }

    #[test]
    fn workload_type_default() {
        assert_eq!(WorkloadType::default(), WorkloadType::Default);
    }

    #[test]
    fn workload_type_from_str_opt() {
        assert_eq!(
            WorkloadType::from_str_opt("default"),
            Some(WorkloadType::Default)
        );
        assert_eq!(
            WorkloadType::from_str_opt("code_review"),
            Some(WorkloadType::CodeReview)
        );
        assert_eq!(WorkloadType::from_str_opt("unknown"), None);
    }

    #[test]
    fn workload_type_serialization_roundtrip() {
        let wt = WorkloadType::Agent;
        let json = serde_json::to_string(&wt).expect("serialize");
        let deserialized: WorkloadType = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(wt, deserialized);
    }

    // --- WorkloadContext ---

    #[test]
    fn workload_context_default() {
        let ctx = WorkloadContext::default();
        assert_eq!(ctx.workload_type, WorkloadType::Default);
        assert_eq!(ctx.priority, 0);
        assert!(ctx.routing_hint.is_none());
        assert!(!ctx.long_running);
    }

    #[test]
    fn workload_context_new() {
        let ctx = WorkloadContext::new(WorkloadType::CodeReview);
        assert_eq!(ctx.workload_type, WorkloadType::CodeReview);
    }

    #[test]
    fn workload_context_builder() {
        let ctx = WorkloadContext::new(WorkloadType::Agent)
            .with_priority(5)
            .with_routing_hint("gpu_pool".to_string())
            .with_long_running();
        assert_eq!(ctx.priority, 5);
        assert_eq!(ctx.routing_hint.as_ref().expect("hint"), "gpu_pool");
        assert!(ctx.long_running);
    }

    #[test]
    fn workload_context_serialization_roundtrip() {
        let ctx = WorkloadContext::new(WorkloadType::Compact).with_priority(3);
        let json = serde_json::to_string(&ctx).expect("serialize");
        let deserialized: WorkloadContext = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(ctx, deserialized);
    }

    // --- workload_header ---

    #[test]
    fn workload_header_simple() {
        let ctx = WorkloadContext::new(WorkloadType::Default);
        let header = workload_header(&ctx);
        assert_eq!(header, "type=default");
    }

    #[test]
    fn workload_header_full() {
        let ctx = WorkloadContext::new(WorkloadType::Agent)
            .with_priority(10)
            .with_routing_hint("special".to_string())
            .with_long_running();
        let header = workload_header(&ctx);
        assert!(header.contains("type=agent"));
        assert!(header.contains("priority=10"));
        assert!(header.contains("routing_hint=special"));
        assert!(header.contains("long_running=true"));
    }

    #[test]
    fn workload_header_no_priority() {
        let ctx = WorkloadContext::new(WorkloadType::Compact);
        let header = workload_header(&ctx);
        assert!(!header.contains("priority"));
    }

    // --- parse_workload_header ---

    #[test]
    fn parse_header_simple() {
        let ctx = parse_workload_header("type=default").expect("should parse");
        assert_eq!(ctx.workload_type, WorkloadType::Default);
    }

    #[test]
    fn parse_header_full() {
        let header = "type=agent;priority=5;routing_hint=gpu;long_running=true";
        let ctx = parse_workload_header(header).expect("should parse");
        assert_eq!(ctx.workload_type, WorkloadType::Agent);
        assert_eq!(ctx.priority, 5);
        assert_eq!(ctx.routing_hint.as_ref().expect("hint"), "gpu");
        assert!(ctx.long_running);
    }

    #[test]
    fn parse_header_unknown_type() {
        assert!(parse_workload_header("type=unknown").is_none());
    }

    #[test]
    fn parse_header_empty() {
        assert!(parse_workload_header("").is_none());
    }

    #[test]
    fn parse_header_no_type() {
        assert!(parse_workload_header("priority=5").is_none());
    }

    #[test]
    fn header_roundtrip() {
        let ctx = WorkloadContext::new(WorkloadType::CodeReview)
            .with_priority(7)
            .with_long_running();
        let header = workload_header(&ctx);
        let parsed = parse_workload_header(&header).expect("should parse");
        assert_eq!(parsed.workload_type, WorkloadType::CodeReview);
        assert_eq!(parsed.priority, 7);
        assert!(parsed.long_running);
    }

    // --- workload_to_json ---

    #[test]
    fn workload_to_json_simple() {
        let ctx = WorkloadContext::new(WorkloadType::Default);
        let json = workload_to_json(&ctx);
        assert_eq!(json["workload_type"], "default");
        assert!(json.get("priority").is_none());
    }

    #[test]
    fn workload_to_json_full() {
        let ctx = WorkloadContext::new(WorkloadType::Agent)
            .with_priority(3)
            .with_routing_hint("fast".to_string())
            .with_long_running();
        let json = workload_to_json(&ctx);
        assert_eq!(json["workload_type"], "agent");
        assert_eq!(json["priority"], 3);
        assert_eq!(json["routing_hint"], "fast");
        assert_eq!(json["long_running"], true);
    }
}
