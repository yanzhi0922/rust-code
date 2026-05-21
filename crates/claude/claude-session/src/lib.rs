//! Session persistence with SQLite metadata and NDJSON transcripts.
//!
//! [`SessionStore`] manages session lifecycle: creation, conversation append,
//! event storage, and export. Each session is backed by a SQLite row for
//! metadata and an NDJSON file for the full event transcript.

pub mod conversation;
pub mod plan_state;
pub mod replay;
pub mod resume_state;
pub mod runtime_context;
pub mod session_memory;
pub mod transcript;

use parking_lot::Mutex;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Utc};
use claude_config::AppPaths;
use claude_core::{ConversationEntry, StoredEvent};
use claude_transcript::TranscriptEntry;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::plan_state::PlanModeState;
use crate::resume_state::ResumeState;
use crate::transcript::SessionTranscript;

/// Summary metadata for a single session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    /// Unique session identifier.
    pub session_id: Uuid,
    /// Optional parent session for lineage tracking.
    pub parent_session_id: Option<Uuid>,
    /// Human-readable session title.
    pub title: String,
    /// Working directory at session creation time.
    pub cwd: PathBuf,
    /// Provider name used for this session.
    pub provider_name: String,
    /// Model identifier, if known.
    pub model: Option<String>,
    /// When the session was created.
    pub created_at: DateTime<Utc>,
    /// When the session was last updated.
    pub updated_at: DateTime<Utc>,
    /// Path to the NDJSON transcript file.
    pub transcript_path: PathBuf,
    /// Whether the session is archived and hidden from active views.
    pub archived: bool,
}

/// Token usage aggregated across a session.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionUsageSummary {
    /// Total input tokens consumed.
    pub input_tokens: u64,
    /// Total output tokens consumed.
    pub output_tokens: u64,
}

/// Statistical summary of a session's contents.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionStats {
    /// Total number of stored events.
    pub total_events: usize,
    /// Number of conversation entries.
    pub conversation_entries: usize,
    /// Message count broken down by role.
    pub messages_by_role: BTreeMap<String, usize>,
    /// Number of tool calls made.
    pub tool_call_count: usize,
    /// Number of error events.
    pub error_count: usize,
    /// Stop reason from the last provider response.
    pub last_stop_reason: Option<String>,
    /// Token usage summary.
    pub usage: SessionUsageSummary,
}

/// A complete bundle of session data for export or inspection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionBundle {
    /// Session metadata.
    pub summary: SessionSummary,
    /// Session statistics.
    pub stats: SessionStats,
    /// Full conversation history.
    pub conversation: Vec<ConversationEntry>,
    /// All stored events.
    pub events: Vec<StoredEvent>,
}

/// Persistent session store backed by SQLite and NDJSON files.
///
/// Uses a single persistent SQLite connection with WAL journal mode for
/// improved write performance and concurrent read safety during long-running
/// sessions.
pub struct SessionStore {
    paths: AppPaths,
    conn: Mutex<Connection>,
}

impl SessionStore {
    /// Open (or create) the session store at the given application paths.
    ///
    /// Enables WAL journal mode, `synchronous=NORMAL`, and a 5-second busy
    /// timeout for robustness during long-running sessions.
    ///
    /// # Errors
    /// Returns an error if the database cannot be opened or the schema cannot be initialised.
    pub fn open(paths: AppPaths) -> Result<Self> {
        paths.ensure_exists()?;
        let conn = Connection::open(&paths.state_db_path)
            .with_context(|| format!("failed to open {}", paths.state_db_path.display()))?;
        // Enable WAL mode for better write performance and concurrent reads.
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA busy_timeout=5000;",
        )
        .context("failed to set SQLite pragmas")?;
        let store = Self {
            paths,
            conn: Mutex::new(conn),
        };
        store.init_schema()?;
        Ok(store)
    }

    /// Return a reference to the application paths.
    #[must_use]
    pub fn paths(&self) -> &AppPaths {
        &self.paths
    }

    /// Ensure a session exists with the given metadata, creating it if needed.
    ///
    /// # Errors
    /// Returns an error if the transcript file or database row cannot be created.
    pub fn ensure_session(
        &self,
        session_id: Uuid,
        cwd: &Path,
        provider_name: &str,
        model: Option<&str>,
        title_hint: Option<&str>,
    ) -> Result<PathBuf> {
        self.ensure_session_with_parent(session_id, cwd, provider_name, model, title_hint, None)
    }

    /// Ensure a session exists with explicit parent lineage metadata.
    ///
    /// # Errors
    /// Returns an error if the transcript file or database row cannot be created.
    pub fn ensure_session_with_parent(
        &self,
        session_id: Uuid,
        cwd: &Path,
        provider_name: &str,
        model: Option<&str>,
        title_hint: Option<&str>,
        parent_session_id: Option<Uuid>,
    ) -> Result<PathBuf> {
        let transcript_path = self.session_transcript_path(session_id);
        if let Some(parent) = transcript_path.parent() {
            fs::create_dir_all(parent)?;
        }
        if !transcript_path.exists() {
            File::create(&transcript_path)?;
        }
        let now = Utc::now();
        let existing = self.try_get_session_summary(session_id)?;
        let title = match existing.as_ref() {
            Some(summary) if !is_default_title(&summary.title, session_id) => summary.title.clone(),
            Some(summary) => {
                normalize_title_hint(title_hint).unwrap_or_else(|| summary.title.clone())
            }
            None => {
                normalize_title_hint(title_hint).unwrap_or_else(|| format!("session-{session_id}"))
            }
        };
        let created_at = existing.as_ref().map_or(now, |summary| summary.created_at);
        let parent_session_id = parent_session_id
            .or_else(|| {
                existing
                    .as_ref()
                    .and_then(|summary| summary.parent_session_id)
            })
            .map(|value| value.to_string());
        self.conn()?.execute(
            "INSERT INTO sessions (
                session_id, title, cwd, provider_name, model, created_at, updated_at, transcript_path, archived, parent_session_id
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, COALESCE((SELECT archived FROM sessions WHERE session_id = ?1), 0), ?9)
            ON CONFLICT(session_id) DO UPDATE SET
                title = excluded.title,
                cwd = excluded.cwd,
                provider_name = excluded.provider_name,
                model = excluded.model,
                updated_at = excluded.updated_at,
                transcript_path = excluded.transcript_path,
                parent_session_id = COALESCE(excluded.parent_session_id, sessions.parent_session_id)",
            params![
                session_id.to_string(),
                title,
                cwd.display().to_string(),
                provider_name,
                model,
                created_at.to_rfc3339(),
                now.to_rfc3339(),
                transcript_path.display().to_string(),
                parent_session_id,
            ],
        )?;
        Ok(transcript_path)
    }

    /// Append a conversation entry to the session transcript.
    ///
    /// # Errors
    /// Returns an error if the event cannot be written to the transcript file.
    pub fn append_conversation_entry(
        &self,
        session_id: Uuid,
        conversation: &ConversationEntry,
    ) -> Result<()> {
        let event = StoredEvent {
            timestamp: Utc::now(),
            session_id,
            event_type: "conversation".to_owned(),
            conversation: Some(conversation.clone()),
            payload: None,
        };
        self.append_event(&event)?;
        self.touch(session_id)?;
        Ok(())
    }

    /// Append a named event with a JSON payload to the session transcript.
    ///
    /// # Errors
    /// Returns an error if the event cannot be written to the transcript file.
    pub fn append_named_event(
        &self,
        session_id: Uuid,
        event_type: impl Into<String>,
        payload: Value,
    ) -> Result<()> {
        let event = StoredEvent {
            timestamp: Utc::now(),
            session_id,
            event_type: event_type.into(),
            conversation: None,
            payload: Some(payload),
        };
        self.append_event(&event)?;
        self.touch(session_id)?;
        Ok(())
    }

    /// Append a typed transcript entry via the legacy `StoredEvent` storage shape.
    ///
    /// # Errors
    /// Returns an error if the projected event cannot be persisted.
    pub fn append_transcript_entry(&self, entry: &TranscriptEntry) -> Result<()> {
        let event = StoredEvent::from(entry);
        self.append_event(&event)?;
        self.touch(event.session_id)?;
        Ok(())
    }

    /// List all sessions ordered by last-updated time (newest first).
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub fn list_sessions(&self) -> Result<Vec<SessionSummary>> {
        self.list_sessions_by_archived(None)
    }

    /// List only active, non-archived sessions ordered by last-updated time.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub fn list_active_sessions(&self) -> Result<Vec<SessionSummary>> {
        self.list_sessions_by_archived(Some(false))
    }

    /// List only archived sessions ordered by last-updated time.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub fn list_archived_sessions(&self) -> Result<Vec<SessionSummary>> {
        self.list_sessions_by_archived(Some(true))
    }

    /// Return the most recently updated active session, if one exists.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub fn latest_active_session(&self) -> Result<Option<SessionSummary>> {
        Ok(self.list_active_sessions()?.into_iter().next())
    }

    /// Mark a session as archived or restored.
    ///
    /// # Errors
    /// Returns an error if the session does not exist or the database update fails.
    pub fn set_archived(&self, session_id: Uuid, archived: bool) -> Result<()> {
        let changed = self.conn()?.execute(
            "UPDATE sessions SET archived = ?2 WHERE session_id = ?1",
            params![session_id.to_string(), archived],
        )?;
        if changed == 0 {
            return Err(anyhow!("session {session_id} does not exist"));
        }
        Ok(())
    }

    fn list_sessions_by_archived(&self, archived: Option<bool>) -> Result<Vec<SessionSummary>> {
        let conn = self.conn()?;
        let sql = if archived.is_some() {
            "SELECT session_id, parent_session_id, title, cwd, provider_name, model, created_at, updated_at, transcript_path, archived
             FROM sessions WHERE archived = ?1 ORDER BY updated_at DESC"
        } else {
            "SELECT session_id, parent_session_id, title, cwd, provider_name, model, created_at, updated_at, transcript_path, archived
             FROM sessions ORDER BY updated_at DESC"
        };
        let mut statement = conn.prepare(sql)?;
        let raw_rows = if let Some(archived) = archived {
            statement
                .query_map([archived], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, String>(7)?,
                        row.get::<_, String>(8)?,
                        row.get::<_, bool>(9)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?
        } else {
            statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, String>(7)?,
                        row.get::<_, String>(8)?,
                        row.get::<_, bool>(9)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        raw_rows.into_iter().map(raw_row_to_summary).collect()
    }

    /// Get the summary for a specific session.
    ///
    /// # Errors
    /// Returns an error if the session does not exist.
    pub fn get_session_summary(&self, session_id: Uuid) -> Result<SessionSummary> {
        self.try_get_session_summary(session_id)?
            .ok_or_else(|| anyhow!("session {session_id} does not exist"))
    }

    /// Load all stored events for a session from its NDJSON transcript.
    ///
    /// # Errors
    /// Returns an error if the transcript file cannot be read or parsed.
    pub fn load_events(&self, session_id: Uuid) -> Result<Vec<StoredEvent>> {
        let transcript_path = self.session_transcript_path(session_id);
        let file = File::open(&transcript_path)
            .with_context(|| format!("failed to open {}", transcript_path.display()))?;
        let reader = BufReader::new(file);
        let mut events = Vec::new();
        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            events.push(serde_json::from_str(&line)?);
        }
        Ok(events)
    }

    /// Load a transcript facade for semantic read access to the NDJSON event log.
    ///
    /// # Errors
    /// Returns an error if the transcript file cannot be read or parsed.
    pub fn load_transcript(&self, session_id: Uuid) -> Result<SessionTranscript> {
        Ok(SessionTranscript::new(
            session_id,
            self.load_events(session_id)?,
        ))
    }

    /// Load a typed transcript view projected from the legacy stored-event log.
    ///
    /// # Errors
    /// Returns an error if any stored event cannot be converted into a
    /// `TranscriptEntry`.
    pub fn load_transcript_v2(&self, session_id: Uuid) -> Result<Vec<TranscriptEntry>> {
        self.load_events(session_id)?
            .into_iter()
            .map(TranscriptEntry::try_from)
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    /// Create a Phase 1 transcript storage handle for the session's transcript path.
    #[must_use]
    pub fn transcript_storage(&self, session_id: Uuid) -> claude_transcript::TranscriptStorage {
        claude_transcript::TranscriptStorage::new(self.session_transcript_path(session_id))
    }

    /// Load only the conversation entries for a session.
    ///
    /// # Errors
    /// Returns an error if the transcript file cannot be read or parsed.
    pub fn load_conversation(&self, session_id: Uuid) -> Result<Vec<ConversationEntry>> {
        Ok(self.load_transcript(session_id)?.conversation_entries())
    }

    /// Load tool names carried forward from compact-boundary metadata.
    ///
    /// # Errors
    /// Returns an error if the transcript file cannot be read or parsed.
    pub fn load_carried_discovered_tool_names(&self, session_id: Uuid) -> Result<BTreeSet<String>> {
        Ok(self
            .load_transcript(session_id)?
            .carried_discovered_tool_names())
    }

    /// Load a complete session bundle (summary, stats, conversation, events).
    ///
    /// # Errors
    /// Returns an error if the session does not exist or data cannot be read.
    pub fn load_session_bundle(&self, session_id: Uuid) -> Result<SessionBundle> {
        let summary = self.get_session_summary(session_id)?;
        let transcript = self.load_transcript(session_id)?;
        let events = transcript.events().to_vec();
        let conversation = transcript.conversation_entries();
        let stats = build_session_stats(&transcript);
        Ok(SessionBundle {
            summary,
            stats,
            conversation,
            events,
        })
    }

    /// Fork an existing session into a new target session, preserving transcript
    /// ordering, timestamps, and summary lineage.
    ///
    /// # Errors
    /// Returns an error if the source session does not exist, contains no
    /// conversation entries, or the target session already has transcript data.
    pub fn fork_session_from_source(
        &self,
        source_session_id: Uuid,
        target_session_id: Uuid,
        title_hint: Option<&str>,
    ) -> Result<SessionSummary> {
        if source_session_id == target_session_id {
            return Err(anyhow!("cannot fork session into itself"));
        }

        let source_summary = self.get_session_summary(source_session_id)?;
        let source_events = self.load_events(source_session_id)?;
        if !source_events
            .iter()
            .any(|event| event.conversation.is_some())
        {
            return Err(anyhow!("no conversation to branch"));
        }

        let target_transcript_path = self.session_transcript_path(target_session_id);
        if target_transcript_path.exists() && fs::metadata(&target_transcript_path)?.len() > 0 {
            return Err(anyhow!(
                "target session {target_session_id} already has transcript state"
            ));
        }

        self.ensure_session_with_parent(
            target_session_id,
            &source_summary.cwd,
            &source_summary.provider_name,
            source_summary.model.as_deref(),
            title_hint.or(Some(source_summary.title.as_str())),
            Some(source_session_id),
        )?;

        for mut event in source_events {
            event.session_id = target_session_id;
            self.append_event(&event)?;
        }
        self.touch(target_session_id)?;
        self.get_session_summary(target_session_id)
    }

    /// Persist a resume-state snapshot for interrupted-session recovery.
    ///
    /// # Errors
    /// Returns an error if the state cannot be serialized or written.
    pub fn save_resume_state(&self, session_id: Uuid, state: &ResumeState) -> Result<()> {
        self.append_named_event(session_id, "resume_state", serde_json::to_value(state)?)
    }

    /// Clear any persisted resume-state snapshot for a session.
    ///
    /// # Errors
    /// Returns an error if the cleared state cannot be written.
    pub fn clear_resume_state(&self, session_id: Uuid) -> Result<()> {
        self.save_resume_state(session_id, &ResumeState::empty())
    }

    /// Load the latest resume-state snapshot for a session.
    ///
    /// # Errors
    /// Returns an error if the transcript cannot be read or the snapshot is invalid.
    pub fn load_resume_state(&self, session_id: Uuid) -> Result<Option<ResumeState>> {
        if !self.session_transcript_path(session_id).exists() {
            return Ok(None);
        }
        self.load_transcript(session_id)?.latest_resume_state()
    }

    /// Persist a plan-mode state snapshot for resume / re-entry flows.
    ///
    /// # Errors
    /// Returns an error if the state cannot be serialized or written.
    pub fn save_plan_mode_state(&self, session_id: Uuid, state: &PlanModeState) -> Result<()> {
        self.append_named_event(session_id, "plan_mode_state", serde_json::to_value(state)?)
    }

    /// Load the latest plan-mode state snapshot for a session.
    ///
    /// # Errors
    /// Returns an error if the transcript cannot be read or the snapshot is invalid.
    pub fn load_plan_mode_state(&self, session_id: Uuid) -> Result<Option<PlanModeState>> {
        if !self.session_transcript_path(session_id).exists() {
            return Ok(None);
        }
        self.load_transcript(session_id)?.latest_plan_mode_state()
    }

    /// Export the session transcript to an NDJSON file.
    ///
    /// # Errors
    /// Returns an error if the session does not exist or the file copy fails.
    pub fn export_session(
        &self,
        session_id: Uuid,
        output_path: Option<PathBuf>,
    ) -> Result<PathBuf> {
        let source = self.session_transcript_path(session_id);
        if !source.exists() {
            return Err(anyhow!("session {session_id} does not exist"));
        }
        let destination = output_path.unwrap_or_else(|| {
            self.paths
                .artifacts_dir
                .join(format!("session-{session_id}.ndjson"))
        });
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(&source, &destination)?;
        Ok(destination)
    }

    /// Export the session bundle as a single JSON file.
    ///
    /// # Errors
    /// Returns an error if the session cannot be loaded or the file cannot be written.
    pub fn export_session_bundle_json(
        &self,
        session_id: Uuid,
        output_path: Option<PathBuf>,
    ) -> Result<PathBuf> {
        let bundle = self.load_session_bundle(session_id)?;
        let destination = output_path.unwrap_or_else(|| {
            self.paths
                .artifacts_dir
                .join(format!("session-{session_id}.json"))
        });
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        let contents = serde_json::to_vec_pretty(&bundle)?;
        fs::write(&destination, contents)?;
        Ok(destination)
    }

    #[must_use]
    pub fn session_transcript_path(&self, session_id: Uuid) -> PathBuf {
        self.paths.sessions_dir.join(format!("{session_id}.ndjson"))
    }

    fn append_event(&self, event: &StoredEvent) -> Result<()> {
        let transcript_path = self.session_transcript_path(event.session_id);
        if let Some(parent) = transcript_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&transcript_path)?;
        serde_json::to_writer(&mut file, event)?;
        file.write_all(b"\n")?;
        Ok(())
    }

    fn touch(&self, session_id: Uuid) -> Result<()> {
        self.conn()?.execute(
            "UPDATE sessions SET updated_at = ?2 WHERE session_id = ?1",
            params![session_id.to_string(), Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    fn init_schema(&self) -> Result<()> {
        let conn = self.conn()?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS sessions (
                session_id TEXT PRIMARY KEY,
                parent_session_id TEXT,
                title TEXT NOT NULL,
                cwd TEXT NOT NULL,
                provider_name TEXT NOT NULL,
                model TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                transcript_path TEXT NOT NULL,
                archived INTEGER NOT NULL DEFAULT 0
            );",
        )?;
        let has_archived = {
            let mut statement = conn.prepare("PRAGMA table_info(sessions)")?;
            let rows = statement.query_map([], |row| row.get::<_, String>(1))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
                .into_iter()
                .any(|column| column == "archived")
        };
        if !has_archived {
            conn.execute(
                "ALTER TABLE sessions ADD COLUMN archived INTEGER NOT NULL DEFAULT 0",
                [],
            )?;
        }
        let has_parent_session_id = {
            let mut statement = conn.prepare("PRAGMA table_info(sessions)")?;
            let rows = statement.query_map([], |row| row.get::<_, String>(1))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
                .into_iter()
                .any(|column| column == "parent_session_id")
        };
        if !has_parent_session_id {
            conn.execute("ALTER TABLE sessions ADD COLUMN parent_session_id TEXT", [])?;
        }
        Ok(())
    }

    /// Obtain a locked reference to the persistent SQLite connection.
    ///
    /// Uses `unwrap_or_else` to recover from a poisoned mutex rather than
    /// panicking, ensuring resilience during long-running sessions.
    fn conn(&self) -> Result<parking_lot::MutexGuard<'_, Connection>> {
        Ok(self.conn.lock())
    }

    fn try_get_session_summary(&self, session_id: Uuid) -> Result<Option<SessionSummary>> {
        let conn = self.conn()?;
        let mut statement = conn.prepare(
            "SELECT session_id, parent_session_id, title, cwd, provider_name, model, created_at, updated_at, transcript_path, archived
             FROM sessions WHERE session_id = ?1 LIMIT 1",
        )?;
        let row = statement.query_row([session_id.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, bool>(9)?,
            ))
        });

        match row {
            Ok(raw) => raw_row_to_summary(raw).map(Some),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }
}

fn parse_timestamp(value: &str) -> Result<DateTime<Utc>> {
    Ok(DateTime::parse_from_rfc3339(value)?.with_timezone(&Utc))
}

fn normalize_title_hint(title_hint: Option<&str>) -> Option<String> {
    title_hint
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.chars().take(80).collect::<String>())
}

fn is_default_title(title: &str, session_id: Uuid) -> bool {
    title == format!("session-{session_id}")
}

type SessionSummaryRow = (
    String,
    Option<String>,
    String,
    String,
    String,
    Option<String>,
    String,
    String,
    String,
    bool,
);

fn raw_row_to_summary(raw: SessionSummaryRow) -> Result<SessionSummary> {
    let (
        session_id,
        parent_session_id,
        title,
        cwd,
        provider_name,
        model,
        created_at,
        updated_at,
        transcript_path,
        archived,
    ) = raw;
    Ok(SessionSummary {
        session_id: Uuid::parse_str(&session_id)?,
        parent_session_id: parent_session_id
            .as_deref()
            .map(Uuid::parse_str)
            .transpose()?,
        title,
        cwd: PathBuf::from(cwd),
        provider_name,
        model,
        created_at: parse_timestamp(&created_at)?,
        updated_at: parse_timestamp(&updated_at)?,
        transcript_path: PathBuf::from(transcript_path),
        archived,
    })
}

fn build_session_stats(transcript: &SessionTranscript) -> SessionStats {
    let events = transcript.events();
    let conversation = transcript.conversation_entries();
    let mut messages_by_role = BTreeMap::new();
    let mut tool_call_count = 0usize;
    let mut error_count = 0usize;
    for entry in &conversation {
        let role = match entry.role {
            claude_core::ConversationRole::System => "system",
            claude_core::ConversationRole::User => "user",
            claude_core::ConversationRole::Assistant => "assistant",
            claude_core::ConversationRole::Tool => "tool",
        };
        *messages_by_role.entry(role.to_owned()).or_insert(0) += 1;
        tool_call_count += entry.tool_calls.len();
        if entry.is_error {
            error_count += 1;
        }
    }

    error_count += transcript.named_event_error_count();
    let usage = transcript.accumulated_usage();
    let last_stop_reason = transcript.last_stop_reason();

    SessionStats {
        total_events: events.len(),
        conversation_entries: conversation.len(),
        messages_by_role,
        tool_call_count,
        error_count,
        last_stop_reason,
        usage,
    }
}

#[cfg(test)]
mod tests {
    use super::SessionStore;
    use crate::resume_state::{PendingToolCall, ResumeState};
    use claude_config::AppPaths;
    use claude_core::ConversationEntry;
    use claude_transcript::{CompactBoundary, CompactTrigger, TranscriptEntry};
    use serde_json::json;
    use tempfile::tempdir;
    use uuid::Uuid;

    #[test]
    fn store_can_round_trip_sessions() {
        let tempdir = match tempdir() {
            Ok(dir) => dir,
            Err(error) => panic!("failed to create tempdir: {error}"),
        };
        let paths = AppPaths::discover(Some(tempdir.path().join(".remote-code-rust")));
        assert!(paths.is_ok());
        let store = SessionStore::open(paths.unwrap_or_else(|error| panic!("{error}")));
        assert!(store.is_ok());
        let store = store.unwrap_or_else(|error| panic!("{error}"));

        let session_id = Uuid::new_v4();
        let ensured = store.ensure_session(
            session_id,
            tempdir.path(),
            "mock",
            Some("mock-model"),
            Some("hello world"),
        );
        assert!(ensured.is_ok());
        let appended =
            store.append_conversation_entry(session_id, &ConversationEntry::user("ship it"));
        assert!(appended.is_ok());
        let list = store.list_sessions();
        assert!(list.is_ok());
        let list = list.unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(list.len(), 1);

        let loaded = store.load_conversation(session_id);
        assert!(loaded.is_ok());
        let loaded = loaded.unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(loaded.len(), 1);

        let appended = store.append_named_event(
            session_id,
            "result",
            json!({
                "is_error": false,
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 2, "output_tokens": 3}
            }),
        );
        assert!(appended.is_ok());

        let bundle = store.load_session_bundle(session_id);
        assert!(bundle.is_ok());
        let bundle = bundle.unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(bundle.stats.total_events, 2);
        assert_eq!(bundle.stats.usage.output_tokens, 3);

        let export = store.export_session_bundle_json(session_id, None);
        assert!(export.is_ok());
        let export = export.unwrap_or_else(|error| panic!("{error}"));
        assert!(export.exists());
    }

    #[test]
    fn resume_state_round_trips() {
        let tempdir = match tempdir() {
            Ok(dir) => dir,
            Err(error) => panic!("failed to create tempdir: {error}"),
        };
        let paths = AppPaths::discover(Some(tempdir.path().join(".remote-code-rust")));
        let store = SessionStore::open(paths.unwrap_or_else(|error| panic!("{error}")))
            .unwrap_or_else(|error| panic!("{error}"));

        let session_id = Uuid::new_v4();
        store
            .ensure_session(session_id, tempdir.path(), "mock", None, None)
            .unwrap_or_else(|error| panic!("{error}"));
        let state = ResumeState::from_pending_calls(vec![PendingToolCall {
            id: "tool-1".to_owned(),
            name: "bash_command".to_owned(),
            input: json!({"command": "pwd"}),
        }]);
        store
            .save_resume_state(session_id, &state)
            .unwrap_or_else(|error| panic!("{error}"));

        let loaded = store
            .load_resume_state(session_id)
            .unwrap_or_else(|error| panic!("{error}"))
            .unwrap_or_else(|| panic!("missing resume state"));
        assert_eq!(loaded.pending_tool_calls.len(), 1);
        assert_eq!(loaded.pending_tool_calls[0].name, "bash_command");

        store
            .clear_resume_state(session_id)
            .unwrap_or_else(|error| panic!("{error}"));
        let cleared = store
            .load_resume_state(session_id)
            .unwrap_or_else(|error| panic!("{error}"))
            .unwrap_or_else(|| panic!("missing cleared state"));
        assert!(cleared.pending_tool_calls.is_empty());
    }

    #[test]
    fn child_session_preserves_parent_lineage() {
        let tempdir = tempdir().unwrap_or_else(|error| panic!("failed to create tempdir: {error}"));
        let paths = AppPaths::discover(Some(tempdir.path().join(".remote-code-rust")))
            .unwrap_or_else(|error| panic!("{error}"));
        let store = SessionStore::open(paths).unwrap_or_else(|error| panic!("{error}"));

        let parent_session_id = Uuid::new_v4();
        let child_session_id = Uuid::new_v4();
        store
            .ensure_session(
                parent_session_id,
                tempdir.path(),
                "mock",
                None,
                Some("parent"),
            )
            .unwrap_or_else(|error| panic!("{error}"));
        store
            .ensure_session_with_parent(
                child_session_id,
                tempdir.path(),
                "mock",
                None,
                Some("child"),
                Some(parent_session_id),
            )
            .unwrap_or_else(|error| panic!("{error}"));

        let summary = store
            .get_session_summary(child_session_id)
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(summary.parent_session_id, Some(parent_session_id));

        store
            .ensure_session(
                child_session_id,
                tempdir.path(),
                "mock",
                None,
                Some("child-again"),
            )
            .unwrap_or_else(|error| panic!("{error}"));
        let summary = store
            .get_session_summary(child_session_id)
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(summary.parent_session_id, Some(parent_session_id));
    }

    #[test]
    fn fork_session_from_source_copies_transcript_and_preserves_lineage() {
        let tempdir = tempdir().unwrap_or_else(|error| panic!("failed to create tempdir: {error}"));
        let paths = AppPaths::discover(Some(tempdir.path().join(".remote-code-rust")))
            .unwrap_or_else(|error| panic!("{error}"));
        let store = SessionStore::open(paths).unwrap_or_else(|error| panic!("{error}"));

        let source_session_id = Uuid::new_v4();
        let target_session_id = Uuid::new_v4();
        store
            .ensure_session(
                source_session_id,
                tempdir.path(),
                "mock",
                Some("mock-model"),
                Some("source"),
            )
            .unwrap_or_else(|error| panic!("{error}"));
        store
            .append_conversation_entry(source_session_id, &ConversationEntry::system("system"))
            .unwrap_or_else(|error| panic!("{error}"));
        store
            .append_conversation_entry(source_session_id, &ConversationEntry::user("hello"))
            .unwrap_or_else(|error| panic!("{error}"));
        store
            .append_named_event(source_session_id, "result", json!({"ok": true}))
            .unwrap_or_else(|error| panic!("{error}"));

        let target_summary = store
            .fork_session_from_source(
                source_session_id,
                target_session_id,
                Some("source (Branch)"),
            )
            .unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(target_summary.session_id, target_session_id);
        assert_eq!(target_summary.parent_session_id, Some(source_session_id));
        assert_eq!(target_summary.title, "source (Branch)");

        let source_events = store
            .load_events(source_session_id)
            .unwrap_or_else(|error| panic!("{error}"));
        let target_events = store
            .load_events(target_session_id)
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(target_events.len(), source_events.len());
        assert!(
            target_events
                .iter()
                .all(|event| event.session_id == target_session_id)
        );
        let source_conversation = store
            .load_conversation(source_session_id)
            .unwrap_or_else(|error| panic!("{error}"));
        let target_conversation = store
            .load_conversation(target_session_id)
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(target_conversation.len(), source_conversation.len());
        for (target, source) in target_conversation.iter().zip(source_conversation.iter()) {
            assert_eq!(target.role, source.role);
            assert_eq!(target.text, source.text);
            assert_eq!(target.tool_call_id, source.tool_call_id);
            assert_eq!(target.name, source.name);
            assert_eq!(target.is_error, source.is_error);
        }
    }

    #[test]
    fn fork_session_from_source_rejects_transcripts_without_conversation_entries() {
        let tempdir = tempdir().unwrap_or_else(|error| panic!("failed to create tempdir: {error}"));
        let paths = AppPaths::discover(Some(tempdir.path().join(".remote-code-rust")))
            .unwrap_or_else(|error| panic!("{error}"));
        let store = SessionStore::open(paths).unwrap_or_else(|error| panic!("{error}"));

        let source_session_id = Uuid::new_v4();
        store
            .ensure_session(
                source_session_id,
                tempdir.path(),
                "mock",
                None,
                Some("source"),
            )
            .unwrap_or_else(|error| panic!("{error}"));
        store
            .append_named_event(source_session_id, "result", json!({"ok": true}))
            .unwrap_or_else(|error| panic!("{error}"));

        let error = store
            .fork_session_from_source(source_session_id, Uuid::new_v4(), None)
            .expect_err("fork should reject transcripts without conversation");
        assert!(error.to_string().contains("no conversation to branch"));
    }

    #[test]
    fn transcript_v2_projection_round_trips_existing_events() {
        let tempdir = tempdir().unwrap_or_else(|error| panic!("failed to create tempdir: {error}"));
        let paths = AppPaths::discover(Some(tempdir.path().join(".remote-code-rust")))
            .unwrap_or_else(|error| panic!("{error}"));
        let store = SessionStore::open(paths).unwrap_or_else(|error| panic!("{error}"));

        let session_id = Uuid::new_v4();
        store
            .ensure_session(
                session_id,
                tempdir.path(),
                "mock",
                Some("mock-model"),
                Some("compat"),
            )
            .unwrap_or_else(|error| panic!("{error}"));
        store
            .append_conversation_entry(session_id, &ConversationEntry::user("hello"))
            .unwrap_or_else(|error| panic!("{error}"));
        store
            .append_named_event(session_id, "tool_result", json!({"ok": true}))
            .unwrap_or_else(|error| panic!("{error}"));

        let entries = store
            .load_transcript_v2(session_id)
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].event_type(), "conversation");
        assert_eq!(entries[1].event_type(), "tool_result");
    }

    #[test]
    fn session_store_accepts_typed_transcript_entries() {
        let tempdir = tempdir().unwrap_or_else(|error| panic!("failed to create tempdir: {error}"));
        let paths = AppPaths::discover(Some(tempdir.path().join(".remote-code-rust")))
            .unwrap_or_else(|error| panic!("{error}"));
        let store = SessionStore::open(paths).unwrap_or_else(|error| panic!("{error}"));

        let session_id = Uuid::new_v4();
        store
            .ensure_session(
                session_id,
                tempdir.path(),
                "mock",
                Some("mock-model"),
                Some("compat"),
            )
            .unwrap_or_else(|error| panic!("{error}"));
        store
            .append_transcript_entry(&TranscriptEntry::compact_boundary_now(
                session_id,
                CompactBoundary::new(CompactTrigger::Auto, 2048),
            ))
            .unwrap_or_else(|error| panic!("{error}"));

        let entries = store
            .load_transcript_v2(session_id)
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(entries.len(), 1);
        assert!(entries[0].as_compact_boundary().is_some());
    }
}
