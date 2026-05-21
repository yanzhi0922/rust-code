use std::path::Path;

use anyhow::Result;
use claude_core::{ConversationEntry, default_system_prompt};
use uuid::Uuid;

use crate::SessionStore;

/// Ensure a session exists and return its conversation, inserting the default
/// system prompt when the transcript does not yet contain any conversation
/// entries.
///
/// # Errors
/// Returns an error if session metadata or transcript writes fail.
pub fn ensure_conversation_initialized(
    store: &SessionStore,
    session_id: Uuid,
    cwd: &Path,
    provider_name: &str,
    model: Option<&str>,
    title_hint: Option<&str>,
) -> Result<Vec<ConversationEntry>> {
    store.ensure_session(session_id, cwd, provider_name, model, title_hint)?;

    let mut conversation = store.load_conversation(session_id).unwrap_or_default();
    if conversation.is_empty() {
        let system = ConversationEntry::system(default_system_prompt(cwd));
        store.append_conversation_entry(session_id, &system)?;
        conversation.push(system);
    }

    Ok(conversation)
}

#[cfg(test)]
mod tests {
    use claude_config::AppPaths;
    use tempfile::tempdir;
    use uuid::Uuid;

    use super::ensure_conversation_initialized;
    use crate::SessionStore;

    #[test]
    fn ensure_conversation_initialized_creates_default_system_prompt() {
        let dir = tempdir().expect("tempdir should succeed");
        let paths = AppPaths::discover(Some(dir.path().join(".remote-code-rust")))
            .expect("paths should discover");
        let store = SessionStore::open(paths).expect("store should open");
        let session_id = Uuid::new_v4();

        let conversation = ensure_conversation_initialized(
            &store,
            session_id,
            dir.path(),
            "mock",
            Some("mock-model"),
            Some("session"),
        )
        .expect("conversation should initialize");

        assert_eq!(conversation.len(), 1);
        assert!(matches!(
            conversation[0].role,
            claude_core::ConversationRole::System
        ));
    }
}
