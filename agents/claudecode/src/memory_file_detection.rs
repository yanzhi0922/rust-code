use std::path::Path;

use claude_config::RuntimeConfig;
use claude_runtime_prompt::{
    AutoMemoryScope, MemoryPromptFeatures, RuntimePromptSettings, SessionMemoryFileType,
    detect_memory_session_file_type, detect_memory_session_pattern_type,
    is_auto_memory_path as runtime_is_auto_memory_path,
    is_team_memory_file_with_features as runtime_is_team_memory_file,
    memory_scope_for_path_with_features as runtime_memory_scope_for_path,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionFileType {
    SessionMemory,
    SessionTranscript,
}

impl From<SessionMemoryFileType> for SessionFileType {
    fn from(value: SessionMemoryFileType) -> Self {
        match value {
            SessionMemoryFileType::SessionMemory => Self::SessionMemory,
            SessionMemoryFileType::SessionTranscript => Self::SessionTranscript,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MemoryScope {
    Personal,
    Team,
}

impl From<AutoMemoryScope> for MemoryScope {
    fn from(value: AutoMemoryScope) -> Self {
        match value {
            AutoMemoryScope::Personal => Self::Personal,
            AutoMemoryScope::Team => Self::Team,
        }
    }
}

fn features(config: &RuntimeConfig) -> MemoryPromptFeatures {
    RuntimePromptSettings::from_config(config).memory_prompt_features
}

pub(crate) fn detect_session_file_type(file_path: &Path) -> Option<SessionFileType> {
    detect_memory_session_file_type(file_path).map(Into::into)
}

pub(crate) fn detect_session_pattern_type(pattern: &str) -> Option<SessionFileType> {
    detect_memory_session_pattern_type(pattern).map(Into::into)
}

pub(crate) fn is_auto_mem_file(config: &RuntimeConfig, file_path: &Path) -> bool {
    runtime_is_auto_memory_path(config, file_path)
}

pub(crate) fn is_team_mem_file(config: &RuntimeConfig, file_path: &Path) -> bool {
    runtime_is_team_memory_file(config, &features(config), file_path)
}

pub(crate) fn memory_scope_for_path(
    config: &RuntimeConfig,
    file_path: &Path,
) -> Option<MemoryScope> {
    runtime_memory_scope_for_path(config, &features(config), file_path).map(Into::into)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{
        MemoryScope, SessionFileType, detect_session_file_type, detect_session_pattern_type,
        memory_scope_for_path,
    };
    use claude_runtime_prompt::RuntimePromptSettings;

    fn test_config() -> (tempfile::TempDir, claude_config::RuntimeConfig) {
        let temp = tempdir().expect("tempdir");
        let cwd = temp.path().join("cwd");
        let profile = temp.path().join("profile");
        fs::create_dir_all(&cwd).expect("cwd dir");
        fs::create_dir_all(&profile).expect("profile dir");
        let config = claude_config::load_runtime_config(
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
            claude_config::ProviderOverrides::default(),
            claude_config::RuntimeOverrides::default(),
        )
        .expect("config");
        (temp, config)
    }

    #[test]
    fn session_pattern_type_matches_research_rules() {
        assert_eq!(
            detect_session_pattern_type("C:/x/session-memory/*.md"),
            Some(SessionFileType::SessionMemory)
        );
        assert_eq!(
            detect_session_pattern_type("C:/x/projects/*.jsonl"),
            Some(SessionFileType::SessionTranscript)
        );
        assert_eq!(detect_session_pattern_type("C:/x/sessions/*.ndjson"), None);
    }

    #[test]
    fn session_file_detection_uses_claude_config_projects_jsonl() {
        let config_home = claude_runtime_prompt::memory_base_dir();
        let transcript_path = config_home
            .join("projects")
            .join("repo")
            .join("session.jsonl");
        assert_eq!(
            detect_session_file_type(&transcript_path),
            Some(SessionFileType::SessionTranscript)
        );
    }

    #[test]
    fn runtime_ndjson_is_not_research_session_transcript() {
        let (_temp, config) = test_config();
        let path = config
            .paths
            .sessions_dir
            .join(format!("{}.ndjson", config.session_id));
        assert_eq!(detect_session_file_type(&path), None);
    }

    #[test]
    fn memory_scope_prefers_team() {
        let (_temp, config) = test_config();
        let features = RuntimePromptSettings::from_config(&config).memory_prompt_features;
        if !features.team_memory_enabled {
            return;
        }
        let team_dir = claude_runtime_prompt::team_memory_path_with_features(&config, &features)
            .expect("team path")
            .expect("team enabled");
        assert_eq!(
            memory_scope_for_path(&config, &team_dir.join("x.md")),
            Some(MemoryScope::Team)
        );
    }
}
