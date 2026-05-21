//! Remote Codex in-process adapter.
//!
//! [`RemoteCodexAdapter`] wraps the existing rc-* crates as an in-process Agent.
//! All shared logic lives in [`InProcessAdapter`](super::in_process::InProcessAdapter).

use super::in_process::InProcessAdapter;

/// Remote Codex in-process adapter (type alias for [`InProcessAdapter`]).
pub type RemoteCodexAdapter = InProcessAdapter;
