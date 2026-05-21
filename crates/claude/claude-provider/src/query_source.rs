//! Query Source marking for API requests.
//!
//! Provides types and utilities for tagging queries with their origin,
//! enabling the API to route and handle requests differently based on source.

use claude_core::{AgentId, SessionId};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

// ---------------------------------------------------------------------------
// QuerySource enum
// ---------------------------------------------------------------------------

/// The origin of a query sent to the API.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum QuerySource {
    /// Direct user input from the CLI or UI.
    User,
    /// Main interactive REPL thread.
    ReplMainThread,
    /// SDK / headless entrypoint.
    Sdk,
    /// Generated during conversation compaction.
    Compact,
    /// Restored from session memory.
    SessionMemory,
    /// Issued by a sub-agent or forked agent.
    Agent,
    /// Issued by extract-memories background fork.
    ExtractMemories,
    /// Issued by the auto-dream background consolidation agent.
    AutoDream,
    /// Issued by the advisor system.
    Advisor,
    /// Issued by a background task.
    BackgroundTask,
    /// Issued by a hook-triggered agent.
    HookAgent,
    /// Issued by a hook-triggered prompt evaluation.
    HookPrompt,
    /// Issued by the verification agent.
    VerificationAgent,
    /// Issued by a side-question (follow-up) query.
    SideQuestion,
    /// Issued by the auto-mode security classifier.
    AutoMode,
}

impl QuerySource {
    /// Return the wire representation for the API header.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::ReplMainThread => "repl_main_thread",
            Self::Sdk => "sdk",
            Self::Compact => "compact",
            Self::SessionMemory => "session_memory",
            Self::Agent => "agent",
            Self::ExtractMemories => "extract_memories",
            Self::AutoDream => "auto_dream",
            Self::Advisor => "advisor",
            Self::BackgroundTask => "background_task",
            Self::HookAgent => "hook_agent",
            Self::HookPrompt => "hook_prompt",
            Self::VerificationAgent => "verification_agent",
            Self::SideQuestion => "side_question",
            Self::AutoMode => "auto_mode",
        }
    }

    /// All known query source values.
    #[must_use]
    pub fn all_values() -> &'static [QuerySource] {
        &[
            QuerySource::User,
            QuerySource::ReplMainThread,
            QuerySource::Sdk,
            QuerySource::Compact,
            QuerySource::SessionMemory,
            QuerySource::Agent,
            QuerySource::ExtractMemories,
            QuerySource::AutoDream,
            QuerySource::Advisor,
            QuerySource::BackgroundTask,
            QuerySource::HookAgent,
            QuerySource::HookPrompt,
            QuerySource::VerificationAgent,
            QuerySource::SideQuestion,
            QuerySource::AutoMode,
        ]
    }
}

impl std::fmt::Display for QuerySource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ---------------------------------------------------------------------------
// QuerySourceContext
// ---------------------------------------------------------------------------

/// Additional context about the query source.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QuerySourceContext {
    /// The query source.
    pub source: QuerySource,
    /// Optional session ID associated with this query.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Optional agent ID if the source is an agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    /// Optional parent query ID for nested queries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_query_id: Option<String>,
}

impl QuerySourceContext {
    /// Create a new query source context with the given source.
    #[must_use]
    pub fn new(source: QuerySource) -> Self {
        Self {
            source,
            session_id: None,
            agent_id: None,
            parent_query_id: None,
        }
    }

    /// Set the session ID.
    #[must_use]
    pub fn with_session_id(mut self, session_id: String) -> Self {
        self.session_id = Some(session_id);
        self
    }

    /// Set the agent ID.
    #[must_use]
    pub fn with_agent_id(mut self, agent_id: String) -> Self {
        self.agent_id = Some(agent_id);
        self
    }

    /// Set the parent query ID.
    #[must_use]
    pub fn with_parent_query_id(mut self, parent_query_id: String) -> Self {
        self.parent_query_id = Some(parent_query_id);
        self
    }
}

/// Minimal request context needed by provider request construction.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderRequestContext {
    pub query_source: QuerySource,
    pub session_id: SessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<AgentId>,
    /// Optional per-request model override used for fallback/retry paths.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_override: Option<String>,
    /// Optional per-request output token limit override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    /// Optional user-facing effort level to forward as `output_config.effort`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// Whether this request should opt into provider fast mode.
    #[serde(default)]
    pub fast_mode: bool,
    /// Optional API-side token budget forwarded as `output_config.task_budget`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_budget: Option<ProviderTaskBudget>,
}

/// API-side task budget sent with Anthropic `output_config.task_budget`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderTaskBudget {
    pub total: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remaining: Option<u64>,
}

impl ProviderRequestContext {
    #[must_use]
    pub fn new(query_source: QuerySource, session_id: SessionId) -> Self {
        Self {
            query_source,
            session_id,
            agent_id: None,
            model_override: None,
            max_output_tokens: None,
            effort: None,
            fast_mode: false,
            task_budget: None,
        }
    }

    #[must_use]
    pub fn with_agent_id(mut self, agent_id: AgentId) -> Self {
        self.agent_id = Some(agent_id);
        self
    }

    #[must_use]
    pub fn with_model_override(mut self, model: Option<String>) -> Self {
        self.model_override = model.and_then(|value| {
            let trimmed = value.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_owned())
        });
        self
    }

    #[must_use]
    pub fn with_max_output_tokens(mut self, max_output_tokens: Option<u32>) -> Self {
        self.max_output_tokens = max_output_tokens.filter(|value| *value > 0);
        self
    }

    #[must_use]
    pub fn with_effort(mut self, effort: Option<String>) -> Self {
        self.effort = effort.and_then(|value| {
            let normalized = value.trim().to_ascii_lowercase();
            matches!(normalized.as_str(), "low" | "medium" | "high" | "max").then_some(normalized)
        });
        self
    }

    #[must_use]
    pub fn with_fast_mode(mut self, fast_mode: bool) -> Self {
        self.fast_mode = fast_mode;
        self
    }

    #[must_use]
    pub fn with_task_budget(mut self, task_budget: Option<ProviderTaskBudget>) -> Self {
        self.task_budget = task_budget.filter(|budget| budget.total > 0);
        self
    }
}

// ---------------------------------------------------------------------------
// Header generation
// ---------------------------------------------------------------------------

/// HTTP header name for query source.
pub const QUERY_SOURCE_HEADER: &str = "x-query-source";

/// Generate the query source header value.
///
/// The header value encodes the source and optional context as a
/// semicolon-separated key=value string.
///
/// # Arguments
///
/// * `ctx` — The query source context.
///
/// # Returns
///
/// The header value string.
#[must_use]
pub fn query_source_header(ctx: &QuerySourceContext) -> String {
    let mut parts = vec![format!("source={}", ctx.source.as_str())];

    if let Some(ref sid) = ctx.session_id {
        parts.push(format!("session_id={sid}"));
    }
    if let Some(ref aid) = ctx.agent_id {
        parts.push(format!("agent_id={aid}"));
    }
    if let Some(ref pid) = ctx.parent_query_id {
        parts.push(format!("parent_query_id={pid}"));
    }

    parts.join(";")
}

/// Parse a query source header value back into a context.
///
/// # Arguments
///
/// * `header` — The header value string.
///
/// # Returns
///
/// The parsed `QuerySourceContext`, or `None` if invalid.
pub fn parse_query_source_header(header: &str) -> Option<QuerySourceContext> {
    let parts: Vec<&str> = header.split(';').collect();
    let mut source = None;
    let mut session_id = None;
    let mut agent_id = None;
    let mut parent_query_id = None;

    for part in parts {
        let part = part.trim();
        if let Some((key, value)) = part.split_once('=') {
            match key {
                "source" => {
                    source = match value {
                        "user" => Some(QuerySource::User),
                        "repl_main_thread" => Some(QuerySource::ReplMainThread),
                        "sdk" => Some(QuerySource::Sdk),
                        "compact" => Some(QuerySource::Compact),
                        "session_memory" => Some(QuerySource::SessionMemory),
                        "agent" => Some(QuerySource::Agent),
                        "extract_memories" => Some(QuerySource::ExtractMemories),
                        "auto_dream" => Some(QuerySource::AutoDream),
                        "advisor" => Some(QuerySource::Advisor),
                        "background_task" => Some(QuerySource::BackgroundTask),
                        "hook_agent" => Some(QuerySource::HookAgent),
                        "hook_prompt" => Some(QuerySource::HookPrompt),
                        "verification_agent" => Some(QuerySource::VerificationAgent),
                        "side_question" => Some(QuerySource::SideQuestion),
                        "auto_mode" => Some(QuerySource::AutoMode),
                        _ => None,
                    };
                }
                "session_id" => session_id = Some(value.to_string()),
                "agent_id" => agent_id = Some(value.to_string()),
                "parent_query_id" => parent_query_id = Some(value.to_string()),
                _ => {}
            }
        }
    }

    source.map(|s| QuerySourceContext {
        source: s,
        session_id,
        agent_id,
        parent_query_id,
    })
}

/// Convert a query source context to a JSON value for API body parameters.
///
/// # Arguments
///
/// * `ctx` — The query source context.
///
/// # Returns
///
/// A JSON object with the source metadata.
#[must_use]
pub fn query_source_to_json(ctx: &QuerySourceContext) -> Value {
    let mut obj = json!({
        "source": ctx.source.as_str(),
    });
    if let Some(ref sid) = ctx.session_id {
        obj["session_id"] = json!(sid);
    }
    if let Some(ref aid) = ctx.agent_id {
        obj["agent_id"] = json!(aid);
    }
    if let Some(ref pid) = ctx.parent_query_id {
        obj["parent_query_id"] = json!(pid);
    }
    obj
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- QuerySource ---

    #[test]
    fn query_source_as_str() {
        assert_eq!(QuerySource::User.as_str(), "user");
        assert_eq!(QuerySource::ReplMainThread.as_str(), "repl_main_thread");
        assert_eq!(QuerySource::Sdk.as_str(), "sdk");
        assert_eq!(QuerySource::Compact.as_str(), "compact");
        assert_eq!(QuerySource::SessionMemory.as_str(), "session_memory");
        assert_eq!(QuerySource::Agent.as_str(), "agent");
        assert_eq!(QuerySource::ExtractMemories.as_str(), "extract_memories");
        assert_eq!(QuerySource::Advisor.as_str(), "advisor");
        assert_eq!(QuerySource::BackgroundTask.as_str(), "background_task");
    }

    #[test]
    fn query_source_display() {
        assert_eq!(QuerySource::User.to_string(), "user");
        assert_eq!(QuerySource::ReplMainThread.to_string(), "repl_main_thread");
        assert_eq!(QuerySource::Agent.to_string(), "agent");
    }

    #[test]
    fn query_source_all_values() {
        let values = QuerySource::all_values();
        assert_eq!(values.len(), 15);
        assert!(values.contains(&QuerySource::ReplMainThread));
        assert!(values.contains(&QuerySource::Sdk));
        assert!(values.contains(&QuerySource::ExtractMemories));
    }

    #[test]
    fn query_source_serialization_roundtrip() {
        for source in QuerySource::all_values() {
            let json = serde_json::to_string(source).expect("serialize");
            let deserialized: QuerySource = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(*source, deserialized);
        }
    }

    // --- QuerySourceContext ---

    #[test]
    fn query_source_context_new() {
        let ctx = QuerySourceContext::new(QuerySource::User);
        assert_eq!(ctx.source, QuerySource::User);
        assert!(ctx.session_id.is_none());
        assert!(ctx.agent_id.is_none());
        assert!(ctx.parent_query_id.is_none());
    }

    #[test]
    fn query_source_context_builder() {
        let ctx = QuerySourceContext::new(QuerySource::Agent)
            .with_session_id("sess_123".to_string())
            .with_agent_id("agent_456".to_string())
            .with_parent_query_id("pq_789".to_string());
        assert_eq!(ctx.session_id.as_ref().expect("session_id"), "sess_123");
        assert_eq!(ctx.agent_id.as_ref().expect("agent_id"), "agent_456");
        assert_eq!(
            ctx.parent_query_id.as_ref().expect("parent_query_id"),
            "pq_789"
        );
    }

    #[test]
    fn query_source_context_serialization_roundtrip() {
        let ctx = QuerySourceContext::new(QuerySource::Compact).with_session_id("s1".to_string());
        let json = serde_json::to_string(&ctx).expect("serialize");
        let deserialized: QuerySourceContext = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(ctx, deserialized);
    }

    // --- query_source_header ---

    #[test]
    fn query_source_header_simple() {
        let ctx = QuerySourceContext::new(QuerySource::User);
        let header = query_source_header(&ctx);
        assert_eq!(header, "source=user");
    }

    #[test]
    fn query_source_header_with_session() {
        let ctx =
            QuerySourceContext::new(QuerySource::Agent).with_session_id("sess_123".to_string());
        let header = query_source_header(&ctx);
        assert!(header.contains("source=agent"));
        assert!(header.contains("session_id=sess_123"));
    }

    #[test]
    fn query_source_header_full() {
        let ctx = QuerySourceContext::new(QuerySource::Agent)
            .with_session_id("s1".to_string())
            .with_agent_id("a1".to_string())
            .with_parent_query_id("p1".to_string());
        let header = query_source_header(&ctx);
        assert!(header.contains("source=agent"));
        assert!(header.contains("session_id=s1"));
        assert!(header.contains("agent_id=a1"));
        assert!(header.contains("parent_query_id=p1"));
    }

    // --- parse_query_source_header ---

    #[test]
    fn parse_header_simple() {
        let ctx = parse_query_source_header("source=user").expect("should parse");
        assert_eq!(ctx.source, QuerySource::User);
    }

    #[test]
    fn parse_header_with_context() {
        let header = "source=agent;session_id=s1;agent_id=a1";
        let ctx = parse_query_source_header(header).expect("should parse");
        assert_eq!(ctx.source, QuerySource::Agent);
        assert_eq!(ctx.session_id.as_ref().expect("session_id"), "s1");
        assert_eq!(ctx.agent_id.as_ref().expect("agent_id"), "a1");
    }

    #[test]
    fn parse_header_accepts_repl_and_sdk_sources() {
        let repl =
            parse_query_source_header("source=repl_main_thread;session_id=s1").expect("repl");
        assert_eq!(repl.source, QuerySource::ReplMainThread);
        assert_eq!(repl.session_id.as_deref(), Some("s1"));

        let sdk = parse_query_source_header("source=sdk").expect("sdk");
        assert_eq!(sdk.source, QuerySource::Sdk);
    }

    #[test]
    fn parse_header_unknown_source() {
        assert!(parse_query_source_header("source=unknown").is_none());
    }

    #[test]
    fn parse_header_empty() {
        assert!(parse_query_source_header("").is_none());
    }

    #[test]
    fn parse_header_no_source() {
        assert!(parse_query_source_header("session_id=s1").is_none());
    }

    #[test]
    fn header_roundtrip() {
        let ctx = QuerySourceContext::new(QuerySource::Advisor).with_session_id("s2".to_string());
        let header = query_source_header(&ctx);
        let parsed = parse_query_source_header(&header).expect("should parse");
        assert_eq!(parsed.source, QuerySource::Advisor);
        assert_eq!(parsed.session_id.as_ref().expect("session_id"), "s2");
    }

    // --- query_source_to_json ---

    #[test]
    fn query_source_to_json_simple() {
        let ctx = QuerySourceContext::new(QuerySource::User);
        let json = query_source_to_json(&ctx);
        assert_eq!(json["source"], "user");
        assert!(json.get("session_id").is_none());
    }

    #[test]
    fn query_source_to_json_full() {
        let ctx = QuerySourceContext::new(QuerySource::Agent)
            .with_session_id("s1".to_string())
            .with_agent_id("a1".to_string())
            .with_parent_query_id("p1".to_string());
        let json = query_source_to_json(&ctx);
        assert_eq!(json["source"], "agent");
        assert_eq!(json["session_id"], "s1");
        assert_eq!(json["agent_id"], "a1");
        assert_eq!(json["parent_query_id"], "p1");
    }
}
