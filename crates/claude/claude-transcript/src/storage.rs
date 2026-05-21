use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::Result;
use tokio::fs::{self, OpenOptions};
use tokio::io::AsyncWriteExt;

use crate::entry::TranscriptEntry;

/// Async JSONL transcript storage.
#[derive(Debug, Clone)]
pub struct TranscriptStorage {
    path: PathBuf,
}

impl TranscriptStorage {
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Append one record as a single JSONL line.
    ///
    /// # Errors
    /// Returns an error if the parent directory cannot be created or the line
    /// cannot be written.
    pub async fn append(&self, entry: &TranscriptEntry) -> Result<()> {
        self.ensure_parent_dir().await?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .await?;
        let mut encoded = serde_json::to_vec(entry)?;
        encoded.push(b'\n');
        file.write_all(&encoded).await?;
        file.flush().await?;
        Ok(())
    }

    /// Read every record from the JSONL file.
    ///
    /// Missing files are treated as an empty transcript.
    ///
    /// # Errors
    /// Returns an error if the file cannot be read or any line cannot be
    /// decoded as `TranscriptEntry`.
    pub async fn read_all(&self) -> Result<Vec<TranscriptEntry>> {
        let contents = match fs::read_to_string(&self.path).await {
            Ok(contents) => contents,
            Err(err) if err.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => return Err(err.into()),
        };

        contents
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(serde_json::from_str)
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    /// Read a half-open range `[start, end)` from the transcript.
    ///
    /// # Errors
    /// Returns an error if reading or decoding the JSONL file fails.
    pub async fn read_range(&self, start: usize, end: usize) -> Result<Vec<TranscriptEntry>> {
        if start >= end {
            return Ok(Vec::new());
        }

        Ok(self
            .read_all()
            .await?
            .into_iter()
            .skip(start)
            .take(end - start)
            .collect())
    }

    /// Remove all records after `index`, keeping `0..=index`.
    ///
    /// Missing files are treated as a no-op.
    ///
    /// # Errors
    /// Returns an error if the file cannot be rewritten.
    pub async fn truncate_after(&self, index: usize) -> Result<()> {
        let mut entries = self.read_all().await?;
        if entries.is_empty() || index >= entries.len().saturating_sub(1) {
            return Ok(());
        }

        entries.truncate(index + 1);
        self.write_entries(&entries).await
    }

    async fn ensure_parent_dir(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).await?;
        }
        Ok(())
    }

    async fn write_entries(&self, entries: &[TranscriptEntry]) -> Result<()> {
        self.ensure_parent_dir().await?;
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&self.path)
            .await?;

        for entry in entries {
            let mut encoded = serde_json::to_vec(entry)?;
            encoded.push(b'\n');
            file.write_all(&encoded).await?;
        }

        file.flush().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use claude_core::ConversationEntry;
    use serde_json::json;
    use tempfile::tempdir;
    use uuid::Uuid;

    use super::TranscriptStorage;
    use crate::boundary::{CompactBoundary, CompactTrigger};
    use crate::entry::TranscriptEntry;

    #[tokio::test]
    async fn storage_appends_and_reads_all_entries() {
        let dir = tempdir().expect("temp dir");
        let storage = TranscriptStorage::new(dir.path().join("transcript.jsonl"));
        let session_id = Uuid::new_v4();

        storage
            .append(&TranscriptEntry::conversation(
                session_id,
                Utc::now(),
                ConversationEntry::user("hello"),
            ))
            .await
            .expect("append conversation");
        storage
            .append(&TranscriptEntry::named_event(
                session_id,
                Utc::now(),
                "status",
                Some(json!({ "ok": true })),
            ))
            .await
            .expect("append event");
        storage
            .append(&TranscriptEntry::compact_boundary(
                session_id,
                Utc::now(),
                CompactBoundary::new(CompactTrigger::Auto, 512),
            ))
            .await
            .expect("append boundary");

        let entries = storage.read_all().await.expect("read entries");
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].event_type(), "conversation");
        assert_eq!(entries[1].event_type(), "status");
        assert_eq!(entries[2].event_type(), "compact_boundary");
    }

    #[tokio::test]
    async fn storage_reads_half_open_ranges() {
        let dir = tempdir().expect("temp dir");
        let storage = TranscriptStorage::new(dir.path().join("transcript.jsonl"));
        let session_id = Uuid::new_v4();

        for text in ["one", "two", "three", "four"] {
            storage
                .append(&TranscriptEntry::conversation(
                    session_id,
                    Utc::now(),
                    ConversationEntry::assistant(text),
                ))
                .await
                .expect("append entry");
        }

        let slice = storage.read_range(1, 3).await.expect("read range");
        assert_eq!(slice.len(), 2);
        assert_eq!(
            slice[0].as_conversation().expect("conversation").text,
            "two"
        );
        assert_eq!(
            slice[1].as_conversation().expect("conversation").text,
            "three"
        );
    }

    #[tokio::test]
    async fn storage_truncates_tail_after_index() {
        let dir = tempdir().expect("temp dir");
        let storage = TranscriptStorage::new(dir.path().join("transcript.jsonl"));
        let session_id = Uuid::new_v4();

        for text in ["alpha", "beta", "gamma"] {
            storage
                .append(&TranscriptEntry::conversation(
                    session_id,
                    Utc::now(),
                    ConversationEntry::assistant(text),
                ))
                .await
                .expect("append entry");
        }

        storage.truncate_after(1).await.expect("truncate");
        let entries = storage.read_all().await.expect("read truncated");
        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries[1].as_conversation().expect("conversation").text,
            "beta"
        );
    }

    #[tokio::test]
    async fn storage_handles_missing_files_as_empty() {
        let dir = tempdir().expect("temp dir");
        let storage = TranscriptStorage::new(dir.path().join("missing").join("transcript.jsonl"));

        assert!(storage.read_all().await.expect("read empty").is_empty());
        assert!(
            storage
                .read_range(0, 5)
                .await
                .expect("read empty range")
                .is_empty()
        );
        storage.truncate_after(3).await.expect("truncate missing");
    }
}
