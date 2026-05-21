//! Transcript entry model and JSONL-backed async storage.

pub mod boundary;
pub mod entry;
pub mod storage;

pub use boundary::{CompactBoundary, CompactTrigger, PreservedSegment};
pub use entry::{
    TranscriptEntry, TranscriptEntryConversionError, TranscriptEntryKind, TranscriptRecordMeta,
};
pub use storage::TranscriptStorage;
