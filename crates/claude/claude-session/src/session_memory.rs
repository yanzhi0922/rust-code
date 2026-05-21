use parking_lot::Mutex;
use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::thread::sleep;
use std::time::{Duration, Instant};

use anyhow::Result;
use claude_config::RuntimeConfig;

const MAX_SECTION_LENGTH: usize = 2_000;
const MAX_TOTAL_SESSION_MEMORY_TOKENS: usize = 12_000;
const EXTRACTION_WAIT_TIMEOUT: Duration = Duration::from_secs(15);
const EXTRACTION_STALE_THRESHOLD: Duration = Duration::from_secs(60);
const SESSION_MEMORY_DIRNAME: &str = "session-memory";
const SESSION_MEMORY_FILENAME: &str = "summary.md";
const MAX_SANITIZED_LENGTH: usize = 200;

pub const DEFAULT_SESSION_MEMORY_TEMPLATE: &str = r#"
# Session Title
_A short and distinctive 5-10 word descriptive title for the session. Super info dense, no filler_

# Current State
_What is actively being worked on right now? Pending tasks not yet completed. Immediate next steps._

# Task specification
_What did the user ask to build? Any design decisions or other explanatory context_

# Files and Functions
_What are the important files? In short, what do they contain and why are they relevant?_

# Workflow
_What bash commands are usually run and in what order? How to interpret their output if not obvious?_

# Errors & Corrections
_Errors encountered and how they were fixed. What did the user correct? What approaches failed and should not be tried again?_

# Codebase and System Documentation
_What are the important system components? How do they work/fit together?_

# Learnings
_What has worked well? What has not? What to avoid? Do not duplicate items from other sections_

# Key results
_If the user asked a specific output such as an answer to a question, a table, or other document, repeat the exact result here_

# Worklog
_Step by step, what was attempted, done? Very terse summary for each step_
"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionMemoryConfig {
    pub minimum_message_tokens_to_init: u64,
    pub minimum_tokens_between_update: u64,
    pub tool_calls_between_updates: u64,
}

impl Default for SessionMemoryConfig {
    fn default() -> Self {
        Self {
            minimum_message_tokens_to_init: 10_000,
            minimum_tokens_between_update: 5_000,
            tool_calls_between_updates: 3,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SessionMemoryPromptConfig {
    pub template: String,
    pub update_prompt: String,
}

impl Default for SessionMemoryPromptConfig {
    fn default() -> Self {
        Self {
            template: DEFAULT_SESSION_MEMORY_TEMPLATE.to_owned(),
            update_prompt: default_update_prompt(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SessionMemoryState {
    pub config: SessionMemoryConfig,
    pub last_summarized_message_id: Option<String>,
    pub extraction_started_at: Option<Instant>,
    pub tokens_at_last_extraction: u64,
    pub initialized: bool,
}

impl SessionMemoryState {
    #[must_use]
    pub fn has_met_initialization_threshold(&self, current_token_count: u64) -> bool {
        current_token_count >= self.config.minimum_message_tokens_to_init
    }

    #[must_use]
    pub fn has_met_update_threshold(&self, current_token_count: u64) -> bool {
        current_token_count.saturating_sub(self.tokens_at_last_extraction)
            >= self.config.minimum_tokens_between_update
    }

    pub fn mark_initialized(&mut self) {
        self.initialized = true;
    }

    pub fn mark_extraction_started(&mut self) {
        self.extraction_started_at = Some(Instant::now());
    }

    pub fn mark_extraction_completed(&mut self) {
        self.extraction_started_at = None;
    }

    pub fn record_extraction_token_count(&mut self, current_token_count: u64) {
        self.tokens_at_last_extraction = current_token_count;
    }

    pub fn set_last_summarized_message_id(&mut self, message_id: Option<String>) {
        self.last_summarized_message_id = message_id;
    }
}

pub fn wait_for_session_memory_extraction(state: &Mutex<SessionMemoryState>) {
    let wait_started_at = Instant::now();
    loop {
        let extraction_started_at = state.lock().extraction_started_at;
        let Some(extraction_started_at) = extraction_started_at else {
            return;
        };

        if extraction_started_at.elapsed() > EXTRACTION_STALE_THRESHOLD {
            return;
        }
        if wait_started_at.elapsed() > EXTRACTION_WAIT_TIMEOUT {
            return;
        }

        sleep(Duration::from_secs(1));
    }
}

#[must_use]
pub fn sanitize_path(name: &str) -> String {
    let sanitized = name
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect::<String>();
    if sanitized.len() <= MAX_SANITIZED_LENGTH {
        return sanitized;
    }

    format!(
        "{}-{}",
        &sanitized[..MAX_SANITIZED_LENGTH],
        simple_hash(name)
    )
}

#[must_use]
pub fn projects_dir() -> PathBuf {
    claude_config_home_dir().join("projects")
}

#[must_use]
pub fn project_dir(cwd: &Path) -> PathBuf {
    projects_dir().join(sanitize_path(&cwd.to_string_lossy()))
}

#[must_use]
pub fn session_memory_dir_for_cwd(cwd: &Path, session_id: uuid::Uuid) -> PathBuf {
    project_dir(cwd)
        .join(session_id.to_string())
        .join(SESSION_MEMORY_DIRNAME)
}

#[must_use]
pub fn session_memory_path_for_cwd(cwd: &Path, session_id: uuid::Uuid) -> PathBuf {
    session_memory_dir_for_cwd(cwd, session_id).join(SESSION_MEMORY_FILENAME)
}

#[must_use]
pub fn session_memory_dir(config: &RuntimeConfig) -> PathBuf {
    session_memory_dir_for_cwd(&config.cwd, config.session_id)
}

#[must_use]
pub fn session_memory_path(config: &RuntimeConfig) -> PathBuf {
    session_memory_path_for_cwd(&config.cwd, config.session_id)
}

pub fn ensure_session_memory_file(config: &RuntimeConfig) -> Result<PathBuf> {
    let memory_dir = session_memory_dir(config);
    fs::create_dir_all(&memory_dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&memory_dir, fs::Permissions::from_mode(0o700))?;
    }
    let memory_path = memory_dir.join(SESSION_MEMORY_FILENAME);

    if !memory_path.exists() {
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&memory_path)
        {
            Ok(mut file) => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    file.set_permissions(fs::Permissions::from_mode(0o600))?;
                }
                file.write_all(load_session_memory_template(config).as_bytes())?;
                file.flush()?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
    }

    Ok(memory_path)
}

pub fn load_session_memory_content(config: &RuntimeConfig) -> Result<Option<String>> {
    let memory_path = session_memory_path(config);
    match fs::read_to_string(&memory_path) {
        Ok(content) => Ok(Some(content)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub fn load_session_memory_template(_config: &RuntimeConfig) -> String {
    let custom = claude_config_home_dir()
        .join(SESSION_MEMORY_DIRNAME)
        .join("config")
        .join("template.md");
    fs::read_to_string(custom).unwrap_or_else(|_| DEFAULT_SESSION_MEMORY_TEMPLATE.to_owned())
}

pub fn load_session_memory_prompt(_config: &RuntimeConfig) -> String {
    let custom = claude_config_home_dir()
        .join(SESSION_MEMORY_DIRNAME)
        .join("config")
        .join("prompt.md");
    fs::read_to_string(custom).unwrap_or_else(|_| default_update_prompt())
}

pub fn is_session_memory_empty(config: &RuntimeConfig, content: &str) -> bool {
    content.trim() == load_session_memory_template(config).trim()
}

pub fn build_session_memory_update_prompt(
    config: &RuntimeConfig,
    current_notes: &str,
    notes_path: &Path,
) -> String {
    let prompt_template = load_session_memory_prompt(config);
    let section_sizes = analyze_section_sizes(current_notes);
    let total_tokens = rough_token_count_estimation(current_notes);
    let section_reminders = generate_section_reminders(&section_sizes, total_tokens);

    substitute_variables(
        &prompt_template,
        &[
            ("currentNotes", current_notes),
            ("notesPath", &notes_path.to_string_lossy()),
        ],
    ) + &section_reminders
}

pub fn truncate_session_memory_for_compact(content: &str) -> (String, bool) {
    let lines = content.split('\n').collect::<Vec<_>>();
    let max_chars_per_section = MAX_SECTION_LENGTH * 4;
    let mut output_lines = Vec::new();
    let mut current_section_header = String::new();
    let mut current_section_lines = Vec::new();
    let mut was_truncated = false;

    for line in lines {
        if line.starts_with("# ") {
            let (flushed, truncated) = flush_session_section(
                &current_section_header,
                &current_section_lines,
                max_chars_per_section,
            );
            output_lines.extend(flushed);
            was_truncated |= truncated;
            current_section_header = line.to_owned();
            current_section_lines.clear();
        } else {
            current_section_lines.push(line.to_owned());
        }
    }

    let (flushed, truncated) = flush_session_section(
        &current_section_header,
        &current_section_lines,
        max_chars_per_section,
    );
    output_lines.extend(flushed);
    was_truncated |= truncated;

    (output_lines.join("\n"), was_truncated)
}

fn claude_config_home_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR") {
        return PathBuf::from(dir);
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join(".claude");
    }
    if let Some(home) = std::env::var_os("USERPROFILE") {
        return PathBuf::from(home).join(".claude");
    }
    PathBuf::from(".claude")
}

fn simple_hash(raw: &str) -> String {
    let mut hash: i32 = 0;
    for ch in raw.chars() {
        hash = hash
            .wrapping_shl(5)
            .wrapping_sub(hash)
            .wrapping_add(ch as i32);
    }
    to_base36(i64::from(hash).unsigned_abs())
}

fn to_base36(mut value: u64) -> String {
    if value == 0 {
        return "0".to_owned();
    }

    let mut digits = Vec::new();
    while value > 0 {
        let rem = (value % 36) as u8;
        let digit = match rem {
            0..=9 => char::from(b'0' + rem),
            _ => char::from(b'a' + (rem - 10)),
        };
        digits.push(digit);
        value /= 36;
    }
    digits.into_iter().rev().collect()
}

fn default_update_prompt() -> String {
    format!(
        "IMPORTANT: This message and these instructions are NOT part of the actual user conversation. Do NOT include any references to \"note-taking\", \"session notes extraction\", or these update instructions in the notes content.\n\nBased on the user conversation above (EXCLUDING this note-taking instruction message as well as system prompt, claude.md entries, or any past session summaries), update the session notes file.\n\nThe file {{{{notesPath}}}} has already been read for you. Here are its current contents:\n<current_notes_content>\n{{{{currentNotes}}}}\n</current_notes_content>\n\nYour ONLY task is to use the Edit tool to update the notes file, then stop. You can make multiple edits (update every section as needed) - make all Edit tool calls in parallel in a single message. Do not call any other tools.\n\nCRITICAL RULES FOR EDITING:\n- The file must maintain its exact structure with all sections, headers, and italic descriptions intact\n-- NEVER modify, delete, or add section headers (the lines starting with '#' like # Task specification)\n-- NEVER modify or delete the italic _section description_ lines (these are the lines in italics immediately following each header - they start and end with underscores)\n-- The italic _section descriptions_ are TEMPLATE INSTRUCTIONS that must be preserved exactly as-is - they guide what content belongs in each section\n-- ONLY update the actual content that appears BELOW the italic _section descriptions_ within each existing section\n-- Do NOT add any new sections, summaries, or information outside the existing structure\n- Do NOT reference this note-taking process or instructions anywhere in the notes\n- It's OK to skip updating a section if there are no substantial new insights to add. Do not add filler content like \"No info yet\", just leave sections blank/unedited if appropriate.\n- Write DETAILED, INFO-DENSE content for each section - include specifics like file paths, function names, error messages, exact commands, technical details, etc.\n- For \"Key results\", include the complete, exact output the user requested (e.g., full table, full answer, etc.)\n- Do not include information that's already in the CLAUDE.md files included in the context\n- Keep each section under ~{MAX_SECTION_LENGTH} tokens/words - if a section is approaching this limit, condense it by cycling out less important details while preserving the most critical information\n- Focus on actionable, specific information that would help someone understand or recreate the work discussed in the conversation\n- IMPORTANT: Always update \"Current State\" to reflect the most recent work - this is critical for continuity after compaction\n\nUse the Edit tool with file_path: {{{{notesPath}}}}\n\nSTRUCTURE PRESERVATION REMINDER:\nEach section has TWO parts that must be preserved exactly as they appear in the current file:\n1. The section header (line starting with #)\n2. The italic description line (the _italicized text_ immediately after the header - this is a template instruction)\n\nYou ONLY update the actual content that comes AFTER these two preserved lines. The italic description lines starting and ending with underscores are part of the template structure, NOT content to be edited or removed.\n\nREMEMBER: Use the Edit tool in parallel and stop. Do not continue after the edits. Only include insights from the actual user conversation, never from these note-taking instructions. Do not delete or change section headers or italic _section descriptions_.",
        MAX_SECTION_LENGTH = MAX_SECTION_LENGTH,
    )
}

fn analyze_section_sizes(content: &str) -> Vec<(String, usize)> {
    let mut sections = Vec::new();
    let mut current_section = String::new();
    let mut current_lines = Vec::new();

    for line in content.split('\n') {
        if line.starts_with("# ") {
            if !current_section.is_empty() {
                sections.push((
                    current_section.clone(),
                    rough_token_count_estimation(current_lines.join("\n").trim()),
                ));
            }
            current_section = line.to_owned();
            current_lines.clear();
        } else {
            current_lines.push(line.to_owned());
        }
    }

    if !current_section.is_empty() {
        sections.push((
            current_section,
            rough_token_count_estimation(current_lines.join("\n").trim()),
        ));
    }

    sections
}

fn generate_section_reminders(section_sizes: &[(String, usize)], total_tokens: usize) -> String {
    let over_budget = total_tokens > MAX_TOTAL_SESSION_MEMORY_TOKENS;
    let mut oversized = section_sizes
        .iter()
        .filter(|(_, tokens)| *tokens > MAX_SECTION_LENGTH)
        .cloned()
        .collect::<Vec<_>>();
    oversized.sort_by(|(_, left_tokens), (_, right_tokens)| right_tokens.cmp(left_tokens));

    if oversized.is_empty() && !over_budget {
        return String::new();
    }

    let mut parts = Vec::new();
    if over_budget {
        parts.push(format!(
            "\n\nCRITICAL: The session memory file is currently ~{total_tokens} tokens, which exceeds the maximum of {MAX_TOTAL_SESSION_MEMORY_TOKENS} tokens. You MUST condense the file to fit within this budget. Aggressively shorten oversized sections by removing less important details, merging related items, and summarizing older entries. Prioritize keeping \"Current State\" and \"Errors & Corrections\" accurate and detailed."
        ));
    }
    if !oversized.is_empty() {
        let header = if over_budget {
            "Oversized sections to condense"
        } else {
            "IMPORTANT: The following sections exceed the per-section limit and MUST be condensed"
        };
        let items = oversized
            .into_iter()
            .map(|(section, tokens)| {
                format!("- \"{section}\" is ~{tokens} tokens (limit: {MAX_SECTION_LENGTH})")
            })
            .collect::<Vec<_>>()
            .join("\n");
        parts.push(format!("\n\n{header}:\n{items}"));
    }
    parts.join("")
}

fn substitute_variables(template: &str, variables: &[(&str, &str)]) -> String {
    let mut rendered = String::with_capacity(template.len());
    let mut rest = template;

    while let Some(start) = rest.find("{{") {
        rendered.push_str(&rest[..start]);
        let tail = &rest[start + 2..];
        let Some(end) = tail.find("}}") else {
            rendered.push_str(&rest[start..]);
            return rendered;
        };

        let key = &tail[..end];
        if key
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
        {
            if let Some((_, value)) = variables.iter().find(|(candidate, _)| *candidate == key) {
                rendered.push_str(value);
            } else {
                rendered.push_str("{{");
                rendered.push_str(key);
                rendered.push_str("}}");
            }
            rest = &tail[end + 2..];
            continue;
        }

        rendered.push_str("{{");
        rest = tail;
    }

    rendered.push_str(rest);
    rendered
}

fn flush_session_section(
    section_header: &str,
    section_lines: &[String],
    max_chars_per_section: usize,
) -> (Vec<String>, bool) {
    if section_header.is_empty() {
        return (section_lines.to_vec(), false);
    }

    let section_content = section_lines.join("\n");
    if section_content.encode_utf16().count() <= max_chars_per_section {
        let mut lines = vec![section_header.to_owned()];
        lines.extend(section_lines.to_vec());
        return (lines, false);
    }

    let mut kept = vec![section_header.to_owned()];
    let mut char_count = 0usize;
    for line in section_lines {
        let line_chars = line.encode_utf16().count();
        if char_count + line_chars + 1 > max_chars_per_section {
            break;
        }
        kept.push(line.clone());
        char_count += line_chars + 1;
    }
    kept.push("\n[... section truncated for length ...]".to_owned());
    (kept, true)
}

fn rough_token_count_estimation(text: &str) -> usize {
    (text.encode_utf16().count() + 2) / 4
}

#[cfg(test)]
mod tests {
    use parking_lot::Mutex;
    use std::ffi::OsString;
    use std::path::PathBuf;
    use std::sync::OnceLock;

    use claude_config::load_runtime_config;
    use claude_config::settings_layers::RuntimeOverrides;
    use claude_config::{ProviderOverrides, RuntimeConfig};
    use claude_core::{InputFormat, OutputFormat, PermissionMode};
    use tempfile::{TempDir, tempdir};

    use super::{
        DEFAULT_SESSION_MEMORY_TEMPLATE, SessionMemoryState, build_session_memory_update_prompt,
        ensure_session_memory_file, is_session_memory_empty, load_session_memory_content,
        project_dir, sanitize_path, session_memory_dir, session_memory_path,
        truncate_session_memory_for_compact, wait_for_session_memory_extraction,
    };

    struct TestRuntime {
        _env_guard: parking_lot::MutexGuard<'static, ()>,
        _tempdir: TempDir,
        config: RuntimeConfig,
        cleanup_project_dir: PathBuf,
        previous_claude_config_dir: Option<OsString>,
    }

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn test_runtime() -> TestRuntime {
        let env_guard = env_lock().lock();
        let tempdir = tempdir().expect("tempdir");
        let cwd = tempdir.path().join("workspace");
        let profile = tempdir.path().join(".remote-code-rust");
        std::fs::create_dir_all(&cwd).expect("cwd");
        std::fs::create_dir_all(&profile).expect("profile");
        let previous_claude_config_dir = std::env::var_os("CLAUDE_CONFIG_DIR");
        // Session memory intentionally follows Claude's global config dir. Keep
        // tests scoped to their temp profile so they never touch user state.
        unsafe { std::env::set_var("CLAUDE_CONFIG_DIR", &profile) };
        let config = load_runtime_config(
            Some(cwd),
            Some(profile),
            None,
            PermissionMode::BypassPermissions,
            InputFormat::Text,
            OutputFormat::Text,
            false,
            false,
            false,
            false,
            8,
            ProviderOverrides::default(),
            RuntimeOverrides::default(),
        )
        .expect("runtime config");
        let cleanup_project_dir = project_dir(&config.cwd);
        TestRuntime {
            _env_guard: env_guard,
            _tempdir: tempdir,
            config,
            cleanup_project_dir,
            previous_claude_config_dir,
        }
    }

    impl Drop for TestRuntime {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.cleanup_project_dir);
            match &self.previous_claude_config_dir {
                Some(value) => unsafe { std::env::set_var("CLAUDE_CONFIG_DIR", value) },
                None => unsafe { std::env::remove_var("CLAUDE_CONFIG_DIR") },
            }
        }
    }

    #[test]
    fn session_memory_paths_are_project_scoped() {
        let runtime = test_runtime();
        let dir = session_memory_dir(&runtime.config);
        let path = session_memory_path(&runtime.config);
        let project_dir = project_dir(&runtime.config.cwd);
        assert!(dir.ends_with("session-memory"));
        assert!(path.ends_with("summary.md"));
        assert!(path.starts_with(&dir));
        assert!(path.starts_with(project_dir.join(runtime.config.session_id.to_string())));
    }

    #[test]
    fn ensure_session_memory_file_creates_template() {
        let runtime = test_runtime();
        let memory_path = ensure_session_memory_file(&runtime.config).expect("memory path");
        let content = std::fs::read_to_string(memory_path).expect("read memory");
        assert_eq!(content, DEFAULT_SESSION_MEMORY_TEMPLATE);
    }

    #[test]
    fn load_session_memory_content_returns_none_when_missing() {
        let runtime = test_runtime();
        assert!(
            load_session_memory_content(&runtime.config)
                .expect("content")
                .is_none()
        );
    }

    #[test]
    fn session_memory_empty_matches_template() {
        let runtime = test_runtime();
        assert!(is_session_memory_empty(
            &runtime.config,
            DEFAULT_SESSION_MEMORY_TEMPLATE
        ));
    }

    #[test]
    fn update_prompt_substitutes_current_notes_and_path() {
        let runtime = test_runtime();
        let prompt = build_session_memory_update_prompt(
            &runtime.config,
            "# Current State\nworking\n",
            &session_memory_path(&runtime.config),
        );
        assert!(prompt.contains("The file"));
        assert!(prompt.contains("Current State"));
        assert!(prompt.contains("summary.md"));
    }

    #[test]
    fn update_prompt_does_not_double_substitute_placeholder_text_from_notes() {
        let runtime = test_runtime();
        let prompt = build_session_memory_update_prompt(
            &runtime.config,
            "# Current State\n{{notesPath}}\n",
            &session_memory_path(&runtime.config),
        );
        assert!(prompt.contains("<current_notes_content>\n# Current State\n{{notesPath}}\n"));
    }

    #[test]
    fn truncate_session_memory_marks_long_sections() {
        let long = format!("# Current State\n{}\n", "a".repeat(12_000));
        let (content, truncated) = truncate_session_memory_for_compact(&long);
        assert!(truncated);
        assert!(content.contains("[... section truncated for length ...]"));
    }

    #[test]
    fn session_memory_state_thresholds_follow_config() {
        let mut state = SessionMemoryState::default();
        assert!(!state.has_met_initialization_threshold(5_000));
        assert!(state.has_met_initialization_threshold(10_000));
        state.mark_initialized();
        state.record_extraction_token_count(10_000);
        assert!(!state.has_met_update_threshold(14_000));
        assert!(state.has_met_update_threshold(15_000));
    }

    #[test]
    fn wait_for_session_memory_extraction_returns_immediately_when_idle() {
        let state = Mutex::new(SessionMemoryState::default());
        wait_for_session_memory_extraction(&state);
    }

    #[test]
    fn sanitize_path_matches_research_examples() {
        assert_eq!(
            sanitize_path("/Users/foo/my-project"),
            "-Users-foo-my-project"
        );
        assert_eq!(sanitize_path("plugin:name:server"), "plugin-name-server");
    }

    #[test]
    fn update_prompt_sorts_oversized_sections_descending() {
        let runtime = test_runtime();
        let notes = format!(
            "# Small\n{}\n# Large\n{}\n",
            "a".repeat(9_000),
            "b".repeat(10_000)
        );
        let prompt = build_session_memory_update_prompt(
            &runtime.config,
            &notes,
            &session_memory_path(&runtime.config),
        );
        let large_index = prompt.find("- \"# Large\"").expect("large reminder");
        let small_index = prompt.find("- \"# Small\"").expect("small reminder");
        assert!(large_index < small_index);
    }
}
