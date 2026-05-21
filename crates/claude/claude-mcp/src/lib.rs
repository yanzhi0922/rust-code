//! MCP (Model Context Protocol) client with stdio transport.
//!
//! Discovers, loads, and communicates with MCP servers over JSON-RPC via
//! stdio. Supports tool listing, tool invocation, and capability negotiation
//! per the MCP specification.
//!
//! # Module structure
//!
//! - [`auth_cache`]    — Authentication state cache (file + TTL)
//! - [`batch`]         — Batched state update queue
//! - [`channel`]       — Channel permissions (allowlist + notifications)
//! - [`config`]        — Configuration loading, parsing, and saving
//! - [`connection`]    — Connection state machine (connected/failed/pending/etc.)
//! - [`discovery`]     — Tool/resource discovery cache
//! - [`elicitation`]   — Elicitation request handling
//! - [`env_expansion`] — Environment variable expansion in config values
//! - [`error`]         — Error types for config and runtime failures
//! - [`headers`]       — Dynamic header resolution (headersHelper scripts)
//! - [`jsonrpc`]       — JSON-RPC protocol types (internal)
//! - [`lifecycle`]     — Lifecycle events and hooks
//! - [`manager`]       — Connection manager (orchestrates all connections)
//! - [`normalization`] — Name normalization utilities
//! - [`oauth`]         — MCP OAuth authentication (PKCE + token management)
//! - [`proxy`]         — Claude.ai proxy server support
//! - [`reconnect`]     — Exponential backoff reconnect scheduler
//! - [`registry`]      — Official MCP server registry
//! - [`resources`]     — Server resource types
//! - [`scope`]         — Configuration scope (local/user/project/etc.)
//! - [`serialization`] — CLI state serialization types
//! - [`session`]       — Stdio MCP session management
//! - [`transport`]     — Transport types (stdio/HTTP/WebSocket/etc.)
//! - [`types`]         — Core MCP type definitions
//! - [`validation`]    — MCP configuration validation utilities

// ── Public modules ──────────────────────────────────────────────────────────

pub mod auth_cache;
pub mod batch;
pub mod channel;
pub mod config;
pub mod connection;
pub mod connection_pool;
pub mod discovery;
pub mod elicitation;
pub mod env_expansion;
pub mod error;
pub mod headers;
pub mod lifecycle;
pub mod manager;
pub mod normalization;
pub mod oauth;
pub mod proxy;
pub mod reconnect;
pub mod registry;
pub mod resources;
pub mod scope;
pub mod serialization;
pub mod session;
pub mod tool_policy;
pub mod transport;
pub mod types;
pub mod validation;

// jsonrpc is internal (pub(crate))
pub(crate) mod jsonrpc;

// ── Re-exports for backward compatibility ───────────────────────────────────

// Constants
pub use session::{
    DEFAULT_MCP_CONFIG_FILE, DEFAULT_MCP_PROTOCOL_VERSION, DEFAULT_PROJECT_MCP_CONFIG_FILE,
    DEFAULT_REQUEST_TIMEOUT_SECS, DEFAULT_STARTUP_TIMEOUT_SECS,
};

// Config types
pub use config::{DiscoveredMcpConfig, McpCapabilityMatrix, McpConfig, McpServerConfig};

// Transport types
pub use transport::{McpTransport, McpTransportConfig};

// Core types
pub use types::{
    MCP_TOOL_RESULT_MAX_CHARS, MCP_TOOL_RESULT_TRUNCATION_NOTICE, McpClientInfo, McpPeerInfo,
    McpPromptArgument, McpPromptDescriptor, McpPromptGetResponse, McpPromptGetResult,
    McpPromptMessage, McpServerInspection, McpToolCallContent, McpToolCallResponse,
    McpToolCallResult, McpToolDescriptor, truncate_tool_call_result, truncate_tool_result_content,
};

// Error types
pub use error::{
    MCP_SESSION_EXPIRED_CODE, McpConfigError, McpRuntimeError, is_session_expired_error,
};

// Session functions and persistent client
pub use session::{
    McpClient, call_tool, discover_mcp_configs, get_prompt, inspect_server, list_prompts,
    list_resources, load_discovered_mcp_configs, read_resource, resolve_stdio_command,
};

// Resource types
pub use jsonrpc::McpResourceContent;

// Connection types
pub use connection::McpServerConnection;

// Manager (high-level API)
pub use manager::McpConnectionManager;

// Lifecycle
pub use lifecycle::{
    DisconnectReason, McpConnectionLifecycle, McpConnectionState, McpLifecycleEvent,
    McpLifecycleHook, McpListChangedSurface, StateTransitionError,
};

// Connection pool
pub use connection_pool::{
    BatchOperationResult, HealthCheckResult, McpConnectionPool, PoolConfig, PoolConnectionStats,
    PoolEntry,
};

// Reconnect
pub use reconnect::{
    CircuitBreakerReconnect, CircuitState, ExponentialBackoffReconnect, ReconnectAction,
    ReconnectScheduler, ReconnectState, ReconnectStrategy,
};

// Auth cache
pub use auth_cache::McpAuthCache;

// Discovery
pub use discovery::{McpDiscovery, McpDiscoveryResult};

// Batch
pub use batch::{
    BatchOperationResults, BatchResourceFetch, BatchResourceResult, BatchResourceResults,
    BatchToolCall, BatchToolCallResult, BatchUpdate, BatchedUpdateQueue, McpBatchOperation,
    McpBatchResourceFetch,
};

// Headers
pub use headers::McpHeadersResolver;

// Serialization
pub use serialization::McpCliState;

// OAuth
pub use oauth::{
    AuthorizationServerMetadata, McpOAuthFlow, OAuthTokenStore, OAuthTokens, PkceParams,
    mcp_oauth_server_key,
};

// Elicitation
pub use elicitation::{
    AutoCancelElicitationHandler, AutoDeclineElicitationHandler, CallbackElicitationHandler,
    ElicitationHandler, ElicitationParams, ElicitationRequestEvent, ElicitationResult,
    ElicitationType, ElicitationWaitingState, QueuedElicitationHandler, TimeoutElicitationHandler,
};

// Channel
pub use channel::{
    ChannelAllowlist, ChannelMessage, ChannelPermissionDecision, ChannelPermissionManager,
};

// Registry
pub use registry::OfficialMcpRegistry;

// Proxy
pub use proxy::{ClaudeAiProxyConfig, ClaudeAiProxyFetch, ProxyRequest};

// Validation
pub use validation::{
    DuplicateEntry, McpConfigValidator, SecurityLevel, SecurityWarning, ValidationWarning,
    ValidationWarningKind,
};

// Tool policy
pub use tool_policy::McpToolPolicy;

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests;
