use claude_config::AppPaths;
use claude_core::ConversationEntry;
use claude_session::SessionStore;
use serde_json::json;
use tempfile::TempDir;
use uuid::Uuid;

fn open_test_store() -> (TempDir, SessionStore) {
    let tempdir = tempfile::tempdir().expect("failed to create tempdir");
    let paths = AppPaths::discover(Some(tempdir.path().join(".remote-code-rust")))
        .expect("failed to discover paths");
    let store = SessionStore::open(paths).expect("failed to open store");
    (tempdir, store)
}

fn fresh_tempdir() -> TempDir {
    tempfile::tempdir().expect("failed to create tempdir")
}

#[test]
fn ensure_session_creates_transcript_file() {
    let (_guard, store) = open_test_store();
    let work = fresh_tempdir();
    let session_id = Uuid::new_v4();
    let result = store.ensure_session(session_id, work.path(), "mock", None, Some("test"));
    assert!(result.is_ok());
    let transcript = store.session_transcript_path(session_id);
    assert!(transcript.exists());
}

#[test]
fn ensure_session_idempotent() {
    let (_guard, store) = open_test_store();
    let work = fresh_tempdir();
    let session_id = Uuid::new_v4();
    store
        .ensure_session(session_id, work.path(), "mock", None, Some("first"))
        .expect("first ensure");
    store
        .ensure_session(session_id, work.path(), "mock", None, Some("second"))
        .expect("second ensure");
    let list = store.list_sessions().expect("list");
    assert_eq!(list.len(), 1);
}

#[test]
fn list_sessions_returns_multiple() {
    let (_guard, store) = open_test_store();
    let work = fresh_tempdir();
    let id1 = Uuid::new_v4();
    let id2 = Uuid::new_v4();
    store
        .ensure_session(id1, work.path(), "mock", None, Some("session one"))
        .expect("ensure 1");
    store
        .ensure_session(id2, work.path(), "mock", None, Some("session two"))
        .expect("ensure 2");
    let list = store.list_sessions().expect("list");
    assert_eq!(list.len(), 2);
}

#[test]
fn get_session_summary_returns_correct_data() {
    let (_guard, store) = open_test_store();
    let work = fresh_tempdir();
    let session_id = Uuid::new_v4();
    store
        .ensure_session(
            session_id,
            work.path(),
            "test-provider",
            Some("gpt-4"),
            Some("my session"),
        )
        .expect("ensure");
    let summary = store.get_session_summary(session_id).expect("summary");
    assert_eq!(summary.session_id, session_id);
    assert_eq!(summary.provider_name, "test-provider");
    assert_eq!(summary.model.as_deref(), Some("gpt-4"));
}

#[test]
fn get_session_summary_fails_for_missing() {
    let (_guard, store) = open_test_store();
    let result = store.get_session_summary(Uuid::new_v4());
    assert!(result.is_err());
}

#[test]
fn append_and_load_conversation() {
    let (_guard, store) = open_test_store();
    let work = fresh_tempdir();
    let session_id = Uuid::new_v4();
    store
        .ensure_session(session_id, work.path(), "mock", None, None)
        .expect("ensure");

    store
        .append_conversation_entry(session_id, &ConversationEntry::system("system prompt"))
        .expect("append system");
    store
        .append_conversation_entry(session_id, &ConversationEntry::user("hello"))
        .expect("append user");
    store
        .append_conversation_entry(session_id, &ConversationEntry::assistant("world"))
        .expect("append assistant");

    let conversation = store.load_conversation(session_id).expect("load");
    assert_eq!(conversation.len(), 3);
    assert_eq!(conversation[0].text, "system prompt");
    assert_eq!(conversation[1].text, "hello");
    assert_eq!(conversation[2].text, "world");
}

#[test]
fn append_named_event_and_load_events() {
    let (_guard, store) = open_test_store();
    let work = fresh_tempdir();
    let session_id = Uuid::new_v4();
    store
        .ensure_session(session_id, work.path(), "mock", None, None)
        .expect("ensure");

    store
        .append_named_event(session_id, "custom_event", json!({"key": "value"}))
        .expect("append event");

    let events = store.load_events(session_id).expect("load events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_type, "custom_event");
    let payload = events[0].payload.as_ref().expect("payload");
    assert_eq!(payload["key"], "value");
}

#[test]
fn export_session_creates_ndjson() {
    let (_guard, store) = open_test_store();
    let work = fresh_tempdir();
    let session_id = Uuid::new_v4();
    store
        .ensure_session(session_id, work.path(), "mock", None, None)
        .expect("ensure");
    store
        .append_conversation_entry(session_id, &ConversationEntry::user("test"))
        .expect("append");

    let exported = store.export_session(session_id, None).expect("export");
    assert!(exported.exists());
    assert!(exported.to_string_lossy().ends_with(".ndjson"));
}

#[test]
fn export_session_bundle_json() {
    let (_guard, store) = open_test_store();
    let work = fresh_tempdir();
    let session_id = Uuid::new_v4();
    store
        .ensure_session(session_id, work.path(), "mock", None, Some("bundle test"))
        .expect("ensure");
    store
        .append_conversation_entry(session_id, &ConversationEntry::user("hello"))
        .expect("append");

    let exported = store
        .export_session_bundle_json(session_id, None)
        .expect("export bundle");
    assert!(exported.exists());
    let content = std::fs::read_to_string(&exported).expect("read");
    let parsed: serde_json::Value = serde_json::from_str(&content).expect("parse");
    assert_eq!(parsed["summary"]["session_id"], session_id.to_string());
}

#[test]
fn load_session_bundle_includes_stats() {
    let (_guard, store) = open_test_store();
    let work = fresh_tempdir();
    let session_id = Uuid::new_v4();
    store
        .ensure_session(session_id, work.path(), "mock", None, None)
        .expect("ensure");
    store
        .append_conversation_entry(session_id, &ConversationEntry::user("hi"))
        .expect("append user");
    store
        .append_conversation_entry(session_id, &ConversationEntry::assistant("hello"))
        .expect("append assistant");
    store
        .append_named_event(
            session_id,
            "result",
            json!({"stop_reason": "end_turn", "usage": {"input_tokens": 10, "output_tokens": 5}}),
        )
        .expect("append result");

    let bundle = store.load_session_bundle(session_id).expect("bundle");
    assert_eq!(bundle.stats.total_events, 3);
    assert_eq!(bundle.stats.conversation_entries, 2);
    assert_eq!(bundle.stats.usage.input_tokens, 10);
    assert_eq!(bundle.stats.usage.output_tokens, 5);
    assert_eq!(bundle.stats.last_stop_reason.as_deref(), Some("end_turn"));
}

#[test]
fn paths_returns_app_paths() {
    let (_guard, store) = open_test_store();
    let paths = store.paths();
    assert!(paths.profile_dir.exists());
    assert!(paths.sessions_dir.exists());
}

#[test]
fn archive_and_restore_session_updates_visibility_lists() {
    let (_guard, store) = open_test_store();
    let work = fresh_tempdir();
    let session_id = Uuid::new_v4();
    store
        .ensure_session(session_id, work.path(), "mock", None, Some("archive me"))
        .expect("ensure");

    assert_eq!(store.list_active_sessions().expect("active").len(), 1);
    assert_eq!(store.list_archived_sessions().expect("archived").len(), 0);

    store.set_archived(session_id, true).expect("archive");

    let summary = store.get_session_summary(session_id).expect("summary");
    assert!(summary.archived);
    assert_eq!(store.list_active_sessions().expect("active").len(), 0);
    assert_eq!(store.list_archived_sessions().expect("archived").len(), 1);

    store.set_archived(session_id, false).expect("restore");

    let summary = store.get_session_summary(session_id).expect("summary");
    assert!(!summary.archived);
    assert_eq!(store.list_active_sessions().expect("active").len(), 1);
    assert_eq!(store.list_archived_sessions().expect("archived").len(), 0);
}
