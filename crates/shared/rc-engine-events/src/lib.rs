//! Shared event contracts for runtime-compatible transport and the Phase 1 engine layer.

pub mod stream;
pub mod types;

pub use stream::EventStream;
pub use types::{
    AgentId, CompactionResult, ContentBlockDelta, ContentBlockType, DaemonPresenceState,
    EngineEvent, EngineStateSnapshot, MessageRole, RuntimeEventCreateRequest, RuntimeEventDetail,
    SessionId, ToolError, ToolProgress, ToolResult, Usage,
};
