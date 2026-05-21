//! # rc-agent-protocol
//!
//! Unified Agent protocol layer for the multi-agent architecture.
//!
//! This crate defines the common types, events, and traits that all Agent
//! adapters must implement, enabling seamless integration of:
//! - **Remote Claude** (in-process, callback-based)
//! - **Remote Roo** (in-process, callback-based)
//! - **Remote Codex** (in-process, callback-based)
//!
//! All three adapters use the same callback pattern: the caller injects
//! `send_message`, `cancel`, and `resolve_permission` callbacks via the
//! builder pattern, and the adapter delegates to those callbacks at runtime.

pub mod adapter;
pub mod adapters;
pub mod bridge;
pub mod error;
pub mod events;
pub mod health;
pub mod jsonrpc;
pub mod permission;
pub mod restart;
pub mod router;
pub mod shared_str;
pub mod types;
pub mod util;

// Re-export core types at crate root for convenience.
pub use adapter::AgentAdapter;
pub use adapters::{InProcessAdapter, RemoteClaudeAdapter, RemoteCodexAdapter, RemoteRooAdapter};
pub use bridge::unified_event_to_runtime_detail;
pub use error::{AdapterError, AgentProtocolError};
pub use events::{AgentResult, ToolCallInfo, UnifiedAgentEvent, UsageInfo};
pub use permission::{PermissionDecision, PermissionRequest};
pub use router::AgentRouter;
pub use shared_str::SharedStr;
pub use types::{AgentCapability, AgentConfig, AgentInfo, AgentStatus, AgentType};
