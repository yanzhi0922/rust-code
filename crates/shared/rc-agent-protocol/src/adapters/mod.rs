//! Agent adapter implementations.
//!
//! This module contains concrete implementations of the [`AgentAdapter`](crate::AgentAdapter)
//! trait for each supported Agent type:
//!
//! - **In-process adapters** (`InProcessAdapter`, `RemoteClaudeAdapter`, etc.) share
//!   the same callback-based implementation — primarily used in integration tests.
//!
//! Production adapters live in their own crates:
//! - **Remote Claude** → `ClaudeInProcessAdapter` from `rc-claude-adapter`
//! - **Remote Codex** → `CodexInProcessAdapter` from `rc-codex-adapter`
//! - **Remote Roo** → `RooInProcessAdapter` from `rc-roo-adapter`

mod in_process;
pub mod remote_claude;
pub mod remote_codex;
pub mod remote_roo;

pub use in_process::InProcessAdapter;
pub use remote_claude::RemoteClaudeAdapter;
pub use remote_codex::RemoteCodexAdapter;
pub use remote_roo::RemoteRooAdapter;
