use std::path::PathBuf;

use anyhow::Result;
use claude_config::RuntimeConfig;
use claude_core::ToolCall;
use claude_session::SessionStore;
use serde_json::Value;

use crate::memory_file_detection::{
    SessionFileType, detect_session_file_type, detect_session_pattern_type, is_auto_mem_file,
    is_team_mem_file, memory_scope_for_path,
};

fn tool_file_path(tool_call: &ToolCall) -> Option<PathBuf> {
    match tool_call.name.as_str() {
        "read_file" | "write_file" | "replace_in_file" | "edit_file" => tool_call
            .input
            .get("file_path")
            .or_else(|| tool_call.input.get("path"))
            .and_then(Value::as_str)
            .map(PathBuf::from),
        "notebook_edit" => tool_call
            .input
            .get("notebook_path")
            .or_else(|| tool_call.input.get("file_path"))
            .or_else(|| tool_call.input.get("path"))
            .and_then(Value::as_str)
            .map(PathBuf::from),
        _ => None,
    }
}

fn session_file_type_from_tool_call(
    _config: &RuntimeConfig,
    tool_call: &ToolCall,
) -> Option<SessionFileType> {
    match tool_call.name.as_str() {
        "read_file" => tool_file_path(tool_call)
            .as_deref()
            .and_then(detect_session_file_type),
        "grep" => {
            if let Some(path) = tool_call
                .input
                .get("path")
                .and_then(Value::as_str)
                .map(PathBuf::from)
                .as_deref()
                .and_then(detect_session_file_type)
            {
                return Some(path);
            }
            tool_call
                .input
                .get("file_pattern")
                .and_then(Value::as_str)
                .and_then(detect_session_pattern_type)
        }
        "glob" => {
            if let Some(path) = tool_call
                .input
                .get("path")
                .and_then(Value::as_str)
                .map(PathBuf::from)
                .as_deref()
                .and_then(detect_session_file_type)
            {
                return Some(path);
            }
            tool_call
                .input
                .get("pattern")
                .and_then(Value::as_str)
                .and_then(detect_session_pattern_type)
        }
        _ => None,
    }
}

fn append_access_event(
    store: &SessionStore,
    config: &RuntimeConfig,
    event_type: &str,
    payload: Value,
) -> Result<()> {
    store.append_named_event(config.session_id, event_type, payload)
}

pub(crate) fn handle_session_file_access_post_tool(
    config: &RuntimeConfig,
    store: &SessionStore,
    tool_call: &ToolCall,
) -> Result<()> {
    let session_file_type = session_file_type_from_tool_call(config, tool_call);
    match session_file_type {
        Some(SessionFileType::SessionMemory) => {
            append_access_event(
                store,
                config,
                "session_memory_accessed",
                serde_json::json!({
                    "tool_name": tool_call.name,
                    "tool_use_id": tool_call.id,
                }),
            )?;
        }
        Some(SessionFileType::SessionTranscript) => {
            append_access_event(
                store,
                config,
                "session_transcript_accessed",
                serde_json::json!({
                    "tool_name": tool_call.name,
                    "tool_use_id": tool_call.id,
                }),
            )?;
        }
        None => {}
    }

    let Some(file_path) = tool_file_path(tool_call) else {
        return Ok(());
    };

    if is_auto_mem_file(config, &file_path) {
        append_access_event(
            store,
            config,
            "auto_memory_accessed",
            serde_json::json!({
                "tool_name": tool_call.name,
                "tool_use_id": tool_call.id,
                "file_path": file_path,
            }),
        )?;
    }

    if is_team_mem_file(config, &file_path) {
        append_access_event(
            store,
            config,
            "team_memory_accessed",
            serde_json::json!({
                "tool_name": tool_call.name,
                "tool_use_id": tool_call.id,
                "file_path": file_path,
            }),
        )?;
    }

    if let Some(scope) = memory_scope_for_path(config, &file_path)
        && matches!(tool_call.name.as_str(), "edit_file" | "write_file")
    {
        append_access_event(
            store,
            config,
            "memory_write_shape",
            serde_json::json!({
                "tool_name": tool_call.name,
                "tool_use_id": tool_call.id,
                "file_path": file_path,
                "scope": match scope {
                    crate::memory_file_detection::MemoryScope::Personal => "personal",
                    crate::memory_file_detection::MemoryScope::Team => "team",
                },
            }),
        )?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::fs;
    use std::sync::{Mutex, MutexGuard, OnceLock};

    use claude_config::{ProviderOverrides, RuntimeOverrides, load_runtime_config};
    use claude_core::ToolCall;
    use claude_runtime_prompt::RuntimePromptSettings;
    use claude_session::SessionStore;
    use tempfile::tempdir;

    use super::handle_session_file_access_post_tool;

    struct TestEnv {
        _env_guard: MutexGuard<'static, ()>,
        previous_claude_config_dir: Option<OsString>,
    }

    impl Drop for TestEnv {
        fn drop(&mut self) {
            match &self.previous_claude_config_dir {
                Some(value) => unsafe { std::env::set_var("CLAUDE_CONFIG_DIR", value) },
                None => unsafe { std::env::remove_var("CLAUDE_CONFIG_DIR") },
            }
        }
    }

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn config_and_store() -> (
        tempfile::TempDir,
        claude_config::RuntimeConfig,
        SessionStore,
        TestEnv,
    ) {
        let env_guard = env_lock().lock().expect("env lock");
        let temp = tempdir().expect("tempdir");
        let cwd = temp.path().join("cwd");
        let profile = temp.path().join("profile");
        fs::create_dir_all(&cwd).expect("cwd");
        fs::create_dir_all(&profile).expect("profile");
        let previous_claude_config_dir = std::env::var_os("CLAUDE_CONFIG_DIR");
        // These tests exercise runtime helpers that intentionally read
        // Claude's global config dir. Keep the global lookup isolated.
        unsafe { std::env::set_var("CLAUDE_CONFIG_DIR", &profile) };
        let config = load_runtime_config(
            Some(cwd),
            Some(profile),
            None,
            claude_core::PermissionMode::Default,
            claude_core::InputFormat::Text,
            claude_core::OutputFormat::Text,
            false,
            false,
            false,
            false,
            4,
            ProviderOverrides::default(),
            RuntimeOverrides::default(),
        )
        .expect("config");
        let store = SessionStore::open(config.paths.clone()).expect("store");
        store
            .ensure_session(config.session_id, &config.cwd, "mock", None, None)
            .expect("ensure session");
        let env = TestEnv {
            _env_guard: env_guard,
            previous_claude_config_dir,
        };
        (temp, config, store, env)
    }

    #[test]
    fn records_session_memory_access_for_read_file() {
        let (_temp, config, store, _env) = config_and_store();
        let session_memory_path = claude_runtime_prompt::claude_config_home()
            .join("projects")
            .join("repo")
            .join(config.session_id.to_string())
            .join("session-memory")
            .join("summary.md");
        fs::create_dir_all(session_memory_path.parent().expect("parent")).expect("mkdir");
        fs::write(&session_memory_path, "summary").expect("write");

        handle_session_file_access_post_tool(
            &config,
            &store,
            &ToolCall {
                id: "tool-1".to_owned(),
                name: "read_file".to_owned(),
                input: serde_json::json!({
                    "path": session_memory_path,
                }),
            },
        )
        .expect("handle");

        let transcript = store
            .load_transcript(config.session_id)
            .expect("load transcript");
        let event = transcript
            .latest_named_event_payload("session_memory_accessed")
            .expect("event");
        assert_eq!(
            event.get("tool_name").and_then(serde_json::Value::as_str),
            Some("read_file")
        );
        assert_eq!(
            event.get("tool_use_id").and_then(serde_json::Value::as_str),
            Some("tool-1")
        );
    }

    #[test]
    fn records_session_transcript_access_for_projects_jsonl() {
        let (_temp, config, store, _env) = config_and_store();
        let transcript_path = claude_runtime_prompt::claude_config_home()
            .join("projects")
            .join("repo")
            .join(format!("{}.jsonl", config.session_id));
        fs::create_dir_all(transcript_path.parent().expect("parent")).expect("mkdir");
        fs::write(&transcript_path, "").expect("write");

        handle_session_file_access_post_tool(
            &config,
            &store,
            &ToolCall {
                id: "tool-2".to_owned(),
                name: "read_file".to_owned(),
                input: serde_json::json!({
                    "path": transcript_path,
                }),
            },
        )
        .expect("handle");

        let transcript = store
            .load_transcript(config.session_id)
            .expect("load transcript");
        let event = transcript
            .latest_named_event_payload("session_transcript_accessed")
            .expect("event");
        assert_eq!(
            event.get("tool_name").and_then(serde_json::Value::as_str),
            Some("read_file")
        );
        assert_eq!(
            event.get("tool_use_id").and_then(serde_json::Value::as_str),
            Some("tool-2")
        );
    }

    #[test]
    fn records_auto_and_team_memory_access_and_scope() {
        let (_temp, config, store, _env) = config_and_store();
        let features = RuntimePromptSettings::from_config(&config).memory_prompt_features;
        let Some(auto_dir) = claude_runtime_prompt::auto_memory_entrypoint(&config)
            .expect("auto memory")
            .and_then(|entrypoint| entrypoint.parent().map(std::path::Path::to_path_buf))
        else {
            return;
        };
        let Some(team_dir) =
            claude_runtime_prompt::team_memory_path_with_features(&config, &features)
                .expect("team memory")
        else {
            return;
        };
        fs::create_dir_all(&team_dir).expect("team dir");
        let auto_path = auto_dir.join("prefs.md");
        let team_path = team_dir.join("shared.md");
        fs::write(&auto_path, "prefs").expect("auto write");
        fs::write(&team_path, "shared").expect("team write");

        handle_session_file_access_post_tool(
            &config,
            &store,
            &ToolCall {
                id: "tool-3".to_owned(),
                name: "write_file".to_owned(),
                input: serde_json::json!({
                    "path": auto_path,
                    "content": "prefs",
                }),
            },
        )
        .expect("auto handle");

        handle_session_file_access_post_tool(
            &config,
            &store,
            &ToolCall {
                id: "tool-4".to_owned(),
                name: "edit_file".to_owned(),
                input: serde_json::json!({
                    "path": team_path,
                    "edits": [{"search": "shared", "replace": "shared"}],
                }),
            },
        )
        .expect("team handle");

        let transcript = store
            .load_transcript(config.session_id)
            .expect("load transcript");
        assert!(
            transcript
                .latest_named_event_payload("auto_memory_accessed")
                .is_some()
        );
        assert!(
            transcript
                .latest_named_event_payload("team_memory_accessed")
                .is_some()
        );
        let shape = transcript
            .latest_named_event_payload("memory_write_shape")
            .expect("shape");
        assert_eq!(
            shape.get("tool_name").and_then(serde_json::Value::as_str),
            Some("edit_file")
        );
        assert_eq!(
            shape.get("scope").and_then(serde_json::Value::as_str),
            Some("team")
        );
    }
}
