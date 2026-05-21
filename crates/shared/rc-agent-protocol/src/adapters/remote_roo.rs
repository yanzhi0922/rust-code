//! Remote Roo in-process adapter.
//!
//! [`RemoteRooAdapter`] wraps the existing rc-* crates as an in-process Agent.
//! All shared logic lives in [`InProcessAdapter`](super::in_process::InProcessAdapter).

use super::in_process::InProcessAdapter;

/// Remote Roo in-process adapter (type alias for [`InProcessAdapter`]).
pub type RemoteRooAdapter = InProcessAdapter;
