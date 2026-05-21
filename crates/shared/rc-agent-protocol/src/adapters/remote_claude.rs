//! Remote Claude in-process adapter.
//!
//! [`RemoteClaudeAdapter`] wraps the existing rc-* crates as an in-process Agent.
//! All shared logic lives in [`InProcessAdapter`](super::in_process::InProcessAdapter).

use super::in_process::InProcessAdapter;

/// Remote Claude in-process adapter (type alias for [`InProcessAdapter`]).
pub type RemoteClaudeAdapter = InProcessAdapter;
