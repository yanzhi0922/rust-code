//! Session restore support.
//!
//! Provides functionality for finding and restoring previous sessions,
//! enabling users to resume conversations from where they left off.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context;
use chrono::{DateTime, Utc};
use claude_core::ConversationEntry;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default profile directory name.
const PROFILE_DIR_NAME: &str = ".remote-code-rust";

/// Sessions subdirectory name.
const SESSIONS_SUBDIR: &str = "sessions";

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Information about a session that can be restored.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRestoreInfo {
    /// Unique session identifier.
    pub session_id: String,
    /// Working directory the session was started in.
    pub cwd: String,
    /// Model used in the session.
    pub model: String,
    /// Timestamp when the session was created.
    pub timestamp: DateTime<Utc>,
    /// Number of messages in the session.
    pub message_count: usize,
}

impl SessionRestoreInfo {
    /// Create a new session restore info.
    pub fn new(
        session_id: String,
        cwd: String,
        model: String,
        timestamp: DateTime<Utc>,
        message_count: usize,
    ) -> Self {
        Self {
            session_id,
            cwd,
            model,
            timestamp,
            message_count,
        }
    }

    /// Format the session info for display.
    pub fn display_summary(&self) -> String {
        let time = self.timestamp.format("%Y-%m-%d %H:%M");
        format!(
            "[{}] {} ({} messages, model: {})",
            time, self.session_id, self.message_count, self.model
        )
    }
}

// ---------------------------------------------------------------------------
// Internal: StoredEvent (mirrors claude_core::StoredEvent for lightweight parsing)
// ---------------------------------------------------------------------------

/// Lightweight event representation for scanning transcripts without
/// depending on the full rc-core StoredEvent type.
#[derive(Debug, Deserialize)]
struct ScannedEvent {
    #[serde(default)]
    timestamp: Option<DateTime<Utc>>,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(rename = "type", default)]
    #[allow(dead_code)]
    event_type: Option<String>,
    #[serde(default)]
    conversation: Option<SerdeConversationEntry>,
    /// Payload may contain session metadata like cwd, model, etc.
    #[serde(default)]
    payload: Option<serde_json::Value>,
}

/// Lightweight conversation entry for transcript scanning.
#[derive(Debug, Deserialize)]
struct SerdeConversationEntry {
    #[serde(default)]
    role: Option<String>,
}

// ---------------------------------------------------------------------------
// Internal: session metadata extracted from transcript
// ---------------------------------------------------------------------------

/// Metadata extracted from scanning a session transcript file.
#[derive(Debug)]
struct SessionMetadata {
    session_id: String,
    timestamp: DateTime<Utc>,
    cwd: String,
    model: String,
    message_count: usize,
}

// ---------------------------------------------------------------------------
// Session directory resolution
// ---------------------------------------------------------------------------

/// Resolve the default sessions directory.
///
/// Checks the following in order:
/// 1. `REMOTE_CODE_RUST_HOME` environment variable
/// 2. `~/.remote-code-rust/sessions`
fn resolve_sessions_dir() -> Option<PathBuf> {
    if let Ok(custom) = std::env::var("REMOTE_CODE_RUST_HOME") {
        let path = PathBuf::from(custom).join(SESSIONS_SUBDIR);
        if path.exists() {
            return Some(path);
        }
    }

    // Fall back to home directory.
    let home = dirs_home()?;
    let path = home.join(PROFILE_DIR_NAME).join(SESSIONS_SUBDIR);
    if path.exists() { Some(path) } else { None }
}

/// Try to find the user's home directory.
fn dirs_home() -> Option<PathBuf> {
    // Try HOME (Unix/macOS), then USERPROFILE (Windows), then HOMEDRIVE+HOMEPATH.
    if let Ok(home) = std::env::var("HOME") {
        let p = PathBuf::from(home);
        if p.exists() {
            return Some(p);
        }
    }
    if let Ok(home) = std::env::var("USERPROFILE") {
        let p = PathBuf::from(home);
        if p.exists() {
            return Some(p);
        }
    }
    let drive = std::env::var("HOMEDRIVE").unwrap_or_else(|_| "C:".to_string());
    let path_str = std::env::var("HOMEPATH").unwrap_or_else(|_| "\\Users\\Default".to_string());
    let p = PathBuf::from(format!("{drive}{path_str}"));
    if p.exists() { Some(p) } else { None }
}

// ---------------------------------------------------------------------------
// Transcript scanning
// ---------------------------------------------------------------------------

/// Scan a single NDJSON transcript file and extract session metadata.
///
/// Returns `None` if the file cannot be parsed or contains no valid events.
fn scan_transcript(path: &Path) -> Option<SessionMetadata> {
    let content = fs::read_to_string(path).ok()?;

    let mut session_id = None;
    let mut timestamp: Option<DateTime<Utc>> = None;
    let mut cwd = String::new();
    let mut model = String::new();
    let mut message_count = 0usize;

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let event: ScannedEvent = match serde_json::from_str(line) {
            Ok(e) => e,
            Err(_) => continue, // Skip malformed lines
        };

        // Extract session_id from the first valid event.
        if session_id.is_none() {
            session_id = event.session_id;
        }

        // Use the first event's timestamp as the session creation time.
        if timestamp.is_none() {
            timestamp = event.timestamp;
        }

        // Count conversation entries (user/assistant/tool messages).
        if let Some(conv) = &event.conversation {
            let role = conv.role.as_deref().unwrap_or("");
            if matches!(role, "user" | "assistant" | "tool") {
                message_count += 1;
            }
        }

        // Extract metadata from payload if available.
        if let Some(payload) = &event.payload {
            if cwd.is_empty()
                && let Some(c) = payload.get("cwd").and_then(|v| v.as_str())
            {
                cwd = c.to_string();
            }
            if model.is_empty()
                && let Some(m) = payload.get("model").and_then(|v| v.as_str())
            {
                model = m.to_string();
            }
        }
    }

    // Try to infer session_id from the filename if not found in events.
    if session_id.is_none() {
        session_id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string());
    }

    Some(SessionMetadata {
        session_id: session_id.unwrap_or_default(),
        timestamp: timestamp.unwrap_or(DateTime::UNIX_EPOCH),
        cwd,
        model,
        message_count,
    })
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Find sessions that can be restored.
///
/// Scans the session storage directory for valid session transcript files,
/// extracts metadata, sorts by timestamp (most recent first), and returns
/// up to `limit` results.
///
/// # Errors
/// Returns an error if the session directory cannot be accessed.
pub fn find_restorable_sessions(limit: usize) -> anyhow::Result<Vec<SessionRestoreInfo>> {
    let sessions_dir = match resolve_sessions_dir() {
        Some(dir) => dir,
        None => return Ok(vec![]),
    };
    find_restorable_sessions_in(&sessions_dir, limit)
}

/// Internal: find restorable sessions in a specific directory.
fn find_restorable_sessions_in(
    sessions_dir: &Path,
    limit: usize,
) -> anyhow::Result<Vec<SessionRestoreInfo>> {
    let entries = match fs::read_dir(sessions_dir) {
        Ok(entries) => entries,
        Err(e) => {
            // Session directory doesn't exist or isn't readable — not an error,
            // just means no sessions to restore.
            if e.kind() == std::io::ErrorKind::NotFound {
                return Ok(vec![]);
            }
            return Err(e).with_context(|| {
                format!(
                    "failed to read sessions directory: {}",
                    sessions_dir.display()
                )
            });
        }
    };

    let mut sessions: Vec<SessionRestoreInfo> = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();

        // Only process .ndjson transcript files.
        if path.extension().and_then(|e| e.to_str()) != Some("ndjson") {
            continue;
        }

        if let Some(meta) = scan_transcript(&path) {
            if meta.session_id.is_empty() || meta.message_count == 0 {
                // Skip empty or invalid sessions.
                continue;
            }

            sessions.push(SessionRestoreInfo {
                session_id: meta.session_id,
                cwd: meta.cwd,
                model: meta.model,
                timestamp: meta.timestamp,
                message_count: meta.message_count,
            });
        }
    }

    // Sort by timestamp, most recent first.
    sessions.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));

    // Apply limit.
    sessions.truncate(limit);

    Ok(sessions)
}

/// Restore a session by its ID, returning the conversation entries.
///
/// Locates the session transcript file by ID, parses the JSONL transcript,
/// and returns the conversation entries in order.
///
/// # Errors
/// Returns an error if the session file cannot be found or parsed.
pub fn restore_session(session_id: &str) -> anyhow::Result<Vec<ConversationEntry>> {
    let sessions_dir = match resolve_sessions_dir() {
        Some(dir) => dir,
        None => return Err(anyhow::anyhow!("session directory not found")),
    };
    restore_session_in(&sessions_dir, session_id)
}

/// Internal: restore a session from a specific directory.
fn restore_session_in(
    sessions_dir: &Path,
    session_id: &str,
) -> anyhow::Result<Vec<ConversationEntry>> {
    let transcript_path = sessions_dir.join(format!("{session_id}.ndjson"));
    if !transcript_path.exists() {
        return Err(anyhow::anyhow!(
            "session {} not found at {}",
            session_id,
            transcript_path.display()
        ));
    }

    let content = fs::read_to_string(&transcript_path).with_context(|| {
        format!(
            "failed to read session transcript: {}",
            transcript_path.display()
        )
    })?;

    let mut entries: Vec<ConversationEntry> = Vec::new();

    for line in content.lines() {
        let line: &str = line.trim();
        if line.is_empty() {
            continue;
        }

        // Parse as a StoredEvent to extract the conversation entry.
        let event: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        // Only process "conversation" events.
        let event_type = event.get("event_type").and_then(|v| v.as_str());
        if event_type != Some("conversation") {
            continue;
        }

        if let Some(conv_value) = event.get("conversation") {
            match serde_json::from_value::<ConversationEntry>(conv_value.clone()) {
                Ok(entry) => entries.push(entry),
                Err(e) => {
                    // Log but don't fail on individual parse errors.
                    tracing::debug!("skipping malformed conversation entry: {e}");
                }
            }
        }
    }

    Ok(entries)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_session(id: &str, count: usize) -> SessionRestoreInfo {
        SessionRestoreInfo::new(
            id.to_string(),
            "/tmp/project".to_string(),
            "gpt-4".to_string(),
            Utc::now(),
            count,
        )
    }

    #[test]
    fn session_restore_info_new() {
        let info = make_test_session("abc-123", 5);
        assert_eq!(info.session_id, "abc-123");
        assert_eq!(info.cwd, "/tmp/project");
        assert_eq!(info.model, "gpt-4");
        assert_eq!(info.message_count, 5);
    }

    #[test]
    fn session_restore_info_display() {
        let info = make_test_session("abc-123", 3);
        let display = info.display_summary();
        assert!(display.contains("abc-123"));
        assert!(display.contains("3 messages"));
        assert!(display.contains("gpt-4"));
    }

    #[test]
    fn find_restorable_sessions_no_dir() {
        // When no session directory exists, should return empty vec, not error.
        let result = find_restorable_sessions(10);
        // This may or may not find sessions depending on the test environment,
        // but it should not panic.
        assert!(result.is_ok());
    }

    #[test]
    fn restore_session_nonexistent() {
        let result = restore_session("nonexistent-session-id-12345");
        assert!(result.is_err());
    }

    #[test]
    fn scan_transcript_valid() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("test-session.ndjson");

        let ndjson = r#"{"timestamp":"2026-01-15T10:30:00Z","session_id":"sess-001","event_type":"init","payload":{"cwd":"/tmp/project","model":"claude-3"}}
{"timestamp":"2026-01-15T10:30:01Z","session_id":"sess-001","event_type":"conversation","conversation":{"role":"user","text":"hello"}}
{"timestamp":"2026-01-15T10:30:02Z","session_id":"sess-001","event_type":"conversation","conversation":{"role":"assistant","text":"world"}}
{"timestamp":"2026-01-15T10:30:03Z","session_id":"sess-001","event_type":"conversation","conversation":{"role":"tool","text":"result","tool_call_id":"tc-1"}}"#;

        fs::write(&path, ndjson).expect("write");

        let meta = scan_transcript(&path).expect("scan");
        assert_eq!(meta.session_id, "sess-001");
        assert_eq!(meta.cwd, "/tmp/project");
        assert_eq!(meta.model, "claude-3");
        assert_eq!(meta.message_count, 3); // user + assistant + tool
    }

    #[test]
    fn scan_transcript_empty_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("empty.ndjson");
        fs::write(&path, "").expect("write");

        let meta = scan_transcript(&path).expect("scan");
        assert_eq!(meta.message_count, 0);
    }

    #[test]
    fn scan_transcript_infers_id_from_filename() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("abc-def-123.ndjson");

        let ndjson = r#"{"timestamp":"2026-01-15T10:30:00Z","event_type":"conversation","conversation":{"role":"user","text":"hi"}}"#;
        fs::write(&path, ndjson).expect("write");

        let meta = scan_transcript(&path).expect("scan");
        assert_eq!(meta.session_id, "abc-def-123");
    }

    #[test]
    fn scan_transcript_skips_malformed_lines() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("mixed.ndjson");

        let ndjson = "not json at all\n{\"timestamp\":\"2026-01-15T10:30:00Z\",\"session_id\":\"s1\",\"event_type\":\"conversation\",\"conversation\":{\"role\":\"user\",\"text\":\"hello\"}}\nalso not json";
        fs::write(&path, ndjson).expect("write");

        let meta = scan_transcript(&path).expect("scan");
        assert_eq!(meta.session_id, "s1");
        assert_eq!(meta.message_count, 1);
    }

    #[test]
    fn find_restorable_sessions_from_temp_dir() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sessions_dir = dir.path().join("sessions");
        fs::create_dir_all(&sessions_dir).expect("mkdir");

        // Write two transcript files.
        let ndjson1 = r#"{"timestamp":"2026-01-15T10:30:00Z","session_id":"sess-newer","event_type":"init","payload":{"cwd":"/proj1","model":"gpt-4"}}
{"timestamp":"2026-01-15T10:30:01Z","session_id":"sess-newer","event_type":"conversation","conversation":{"role":"user","text":"hello"}}"#;

        let ndjson2 = r#"{"timestamp":"2026-01-10T08:00:00Z","session_id":"sess-older","event_type":"init","payload":{"cwd":"/proj2","model":"claude-3"}}
{"timestamp":"2026-01-10T08:00:01Z","session_id":"sess-older","event_type":"conversation","conversation":{"role":"user","text":"hi"}}
{"timestamp":"2026-01-10T08:00:02Z","session_id":"sess-older","event_type":"conversation","conversation":{"role":"assistant","text":"there"}}"#;

        fs::write(sessions_dir.join("sess-newer.ndjson"), ndjson1).expect("write");
        fs::write(sessions_dir.join("sess-older.ndjson"), ndjson2).expect("write");

        let result = find_restorable_sessions_in(&sessions_dir, 10).expect("find");

        assert_eq!(result.len(), 2);
        // Most recent first.
        assert_eq!(result[0].session_id, "sess-newer");
        assert_eq!(result[1].session_id, "sess-older");
        assert_eq!(result[0].message_count, 1);
        assert_eq!(result[1].message_count, 2);
    }

    #[test]
    fn find_restorable_sessions_respects_limit() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sessions_dir = dir.path().join("sessions");
        fs::create_dir_all(&sessions_dir).expect("mkdir");

        for i in 0..5u32 {
            let ts = format!("2026-01-15T10:{:02}:00Z", 30 + i);
            let ndjson = format!(
                r#"{{"timestamp":"{ts}","session_id":"sess-{i}","event_type":"init","payload":{{"cwd":"/tmp","model":"test"}}, "conversation":{{"role":"user","text":"msg"}}}}"#
            );
            fs::write(sessions_dir.join(format!("sess-{i}.ndjson")), ndjson).expect("write");
        }

        let result = find_restorable_sessions_in(&sessions_dir, 3).expect("find");
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn restore_session_reads_conversation_entries() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sessions_dir = dir.path().join("sessions");
        fs::create_dir_all(&sessions_dir).expect("mkdir");

        let ndjson = r#"{"timestamp":"2026-01-15T10:30:00Z","session_id":"sess-restore","event_type":"init","payload":{}}
{"timestamp":"2026-01-15T10:30:01Z","session_id":"sess-restore","event_type":"conversation","conversation":{"uuid":"00000000-0000-0000-0000-000000000001","role":"user","text":"hello"}}
{"timestamp":"2026-01-15T10:30:02Z","session_id":"sess-restore","event_type":"conversation","conversation":{"uuid":"00000000-0000-0000-0000-000000000002","role":"assistant","text":"world"}}"#;

        fs::write(sessions_dir.join("sess-restore.ndjson"), ndjson).expect("write");

        let entries = restore_session_in(&sessions_dir, "sess-restore").expect("restore");

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].text, "hello");
        assert_eq!(entries[1].text, "world");
    }
}
