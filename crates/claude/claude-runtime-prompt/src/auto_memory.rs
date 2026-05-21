use std::fs;
use std::path::{Component, MAIN_SEPARATOR, Path, PathBuf};
use std::time::UNIX_EPOCH;

use anyhow::Result;
use claude_config::RuntimeConfig;
use claude_config::settings_layers::load_runtime_settings;
use regex::Regex;
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

const AUTO_MEMORY_DIRNAME: &str = "memory";
const AUTO_MEMORY_PROJECTS_DIRNAME: &str = "projects";
const ENTRYPOINT_NAME: &str = "MEMORY.md";
const MAX_ENTRYPOINT_LINES: usize = 200;
const MAX_ENTRYPOINT_BYTES: usize = 25_000;
const MAX_SANITIZED_LENGTH: usize = 200;
const DIR_EXISTS_GUIDANCE: &str = "This directory already exists — write to it directly with the Write tool (do not run mkdir or check for its existence).";
const MEMORY_DRIFT_CAVEAT: &str = "- Memory records can become stale over time. Use memory as context for what was true at a given point in time. Before answering the user or building assumptions based solely on information in memory records, verify that the memory is still correct and up-to-date by reading the current state of the files or resources. If a recalled memory conflicts with current information, trust what you observe now — and update or remove the stale memory rather than acting on it.";

const TYPES_SECTION_INDIVIDUAL: &[&str] = &[
    "## Types of memory",
    "",
    "There are several discrete types of memory that you can store in your memory system:",
    "",
    "<types>",
    "<type>",
    "    <name>user</name>",
    "    <description>Contain information about the user's role, goals, responsibilities, and knowledge. Great user memories help you tailor your future behavior to the user's preferences and perspective. Your goal in reading and writing these memories is to build up an understanding of who the user is and how you can be most helpful to them specifically. For example, you should collaborate with a senior software engineer differently than a student who is coding for the very first time. Keep in mind, that the aim here is to be helpful to the user. Avoid writing memories about the user that could be viewed as a negative judgement or that are not relevant to the work you're trying to accomplish together.</description>",
    "    <when_to_save>When you learn any details about the user's role, preferences, responsibilities, or knowledge</when_to_save>",
    "    <how_to_use>When your work should be informed by the user's profile or perspective. For example, if the user is asking you to explain a part of the code, you should answer that question in a way that is tailored to the specific details that they will find most valuable or that helps them build their mental model in relation to domain knowledge they already have.</how_to_use>",
    "    <examples>",
    "    user: I'm a data scientist investigating what logging we have in place",
    "    assistant: [saves user memory: user is a data scientist, currently focused on observability/logging]",
    "",
    "    user: I've been writing Go for ten years but this is my first time touching the React side of this repo",
    "    assistant: [saves user memory: deep Go expertise, new to React and this project's frontend — frame frontend explanations in terms of backend analogues]",
    "    </examples>",
    "</type>",
    "<type>",
    "    <name>feedback</name>",
    "    <description>Guidance the user has given you about how to approach work — both what to avoid and what to keep doing. These are a very important type of memory to read and write as they allow you to remain coherent and responsive to the way you should approach work in the project. Record from failure AND success: if you only save corrections, you will avoid past mistakes but drift away from approaches the user has already validated, and may grow overly cautious.</description>",
    "    <when_to_save>Any time the user corrects your approach (\"no not that\", \"don't\", \"stop doing X\") OR confirms a non-obvious approach worked (\"yes exactly\", \"perfect, keep doing that\", accepting an unusual choice without pushback). Corrections are easy to notice; confirmations are quieter — watch for them. In both cases, save what is applicable to future conversations, especially if surprising or not obvious from the code. Include *why* so you can judge edge cases later.</when_to_save>",
    "    <how_to_use>Let these memories guide your behavior so that the user does not need to offer the same guidance twice.</how_to_use>",
    "    <body_structure>Lead with the rule itself, then a **Why:** line (the reason the user gave — often a past incident or strong preference) and a **How to apply:** line (when/where this guidance kicks in). Knowing *why* lets you judge edge cases instead of blindly following the rule.</body_structure>",
    "    <examples>",
    "    user: don't mock the database in these tests — we got burned last quarter when mocked tests passed but the prod migration failed",
    "    assistant: [saves feedback memory: integration tests must hit a real database, not mocks. Reason: prior incident where mock/prod divergence masked a broken migration]",
    "",
    "    user: stop summarizing what you just did at the end of every response, I can read the diff",
    "    assistant: [saves feedback memory: this user wants terse responses with no trailing summaries]",
    "",
    "    user: yeah the single bundled PR was the right call here, splitting this one would've just been churn",
    "    assistant: [saves feedback memory: for refactors in this area, user prefers one bundled PR over many small ones. Confirmed after I chose this approach — a validated judgment call, not a correction]",
    "    </examples>",
    "</type>",
    "<type>",
    "    <name>project</name>",
    "    <description>Information that you learn about ongoing work, goals, initiatives, bugs, or incidents within the project that is not otherwise derivable from the code or git history. Project memories help you understand the broader context and motivation behind the work the user is doing within this working directory.</description>",
    "    <when_to_save>When you learn who is doing what, why, or by when. These states change relatively quickly so try to keep your understanding of this up to date. Always convert relative dates in user messages to absolute dates when saving (e.g., \"Thursday\" → \"2026-03-05\"), so the memory remains interpretable after time passes.</when_to_save>",
    "    <how_to_use>Use these memories to more fully understand the details and nuance behind the user's request and make better informed suggestions.</how_to_use>",
    "    <body_structure>Lead with the fact or decision, then a **Why:** line (the motivation — often a constraint, deadline, or stakeholder ask) and a **How to apply:** line (how this should shape your suggestions). Project memories decay fast, so the why helps future-you judge whether the memory is still load-bearing.</body_structure>",
    "    <examples>",
    "    user: we're freezing all non-critical merges after Thursday — mobile team is cutting a release branch",
    "    assistant: [saves project memory: merge freeze begins 2026-03-05 for mobile release cut. Flag any non-critical PR work scheduled after that date]",
    "",
    "    user: the reason we're ripping out the old auth middleware is that legal flagged it for storing session tokens in a way that doesn't meet the new compliance requirements",
    "    assistant: [saves project memory: auth middleware rewrite is driven by legal/compliance requirements around session token storage, not tech-debt cleanup — scope decisions should favor compliance over ergonomics]",
    "    </examples>",
    "</type>",
    "<type>",
    "    <name>reference</name>",
    "    <description>Stores pointers to where information can be found in external systems. These memories allow you to remember where to look to find up-to-date information outside of the project directory.</description>",
    "    <when_to_save>When you learn about resources in external systems and their purpose. For example, that bugs are tracked in a specific project in Linear or that feedback can be found in a specific Slack channel.</when_to_save>",
    "    <how_to_use>When the user references an external system or information that may be in an external system.</how_to_use>",
    "    <examples>",
    "    user: check the Linear project \"INGEST\" if you want context on these tickets, that's where we track all pipeline bugs",
    "    assistant: [saves reference memory: pipeline bugs are tracked in Linear project \"INGEST\"]",
    "",
    "    user: the Grafana board at grafana.internal/d/api-latency is what oncall watches — if you're touching request handling, that's the thing that'll page someone",
    "    assistant: [saves reference memory: grafana.internal/d/api-latency is the oncall latency dashboard — check it when editing request-path code]",
    "    </examples>",
    "</type>",
    "</types>",
    "",
];

const WHAT_NOT_TO_SAVE_SECTION: &[&str] = &[
    "## What NOT to save in memory",
    "",
    "- Code patterns, conventions, architecture, file paths, or project structure — these can be derived by reading the current project state.",
    "- Git history, recent changes, or who-changed-what — `git log` / `git blame` are authoritative.",
    "- Debugging solutions or fix recipes — the fix is in the code; the commit message has the context.",
    "- Anything already documented in CLAUDE.md files.",
    "- Ephemeral task details: in-progress work, temporary state, current conversation context.",
    "",
    "These exclusions apply even when the user explicitly asks you to save. If they ask you to save a PR list or activity summary, ask what was *surprising* or *non-obvious* about it — that is the part worth keeping.",
];

const WHEN_TO_ACCESS_SECTION: &[&str] = &[
    "## When to access memories",
    "- When memories seem relevant, or the user references prior-conversation work.",
    "- You MUST access memory when the user explicitly asks you to check, recall, or remember.",
    "- If the user says to *ignore* or *not use* memory: proceed as if MEMORY.md were empty. Do not apply remembered facts, cite, compare against, or mention memory content.",
    MEMORY_DRIFT_CAVEAT,
];

const WHEN_TO_ACCESS_COMBINED_SECTION: &[&str] = &[
    "## When to access memories",
    "- When memories (personal or team) seem relevant, or the user references prior work with them or others in their organization.",
    "- You MUST access memory when the user explicitly asks you to check, recall, or remember.",
    "- If the user says to *ignore* or *not use* memory: proceed as if MEMORY.md were empty. Do not apply remembered facts, cite, compare against, or mention memory content.",
    MEMORY_DRIFT_CAVEAT,
];

const TRUSTING_RECALL_SECTION: &[&str] = &[
    "## Before recommending from memory",
    "",
    "A memory that names a specific function, file, or flag is a claim that it existed *when the memory was written*. It may have been renamed, removed, or never merged. Before recommending it:",
    "",
    "- If the memory names a file path: check the file exists.",
    "- If the memory names a function or flag: grep for it.",
    "- If the user is about to act on your recommendation (not just asking about history), verify first.",
    "",
    "\"The memory says X exists\" is not the same as \"X exists now.\"",
    "",
    "A memory that summarizes repo state (activity logs, architecture snapshots) is frozen in time. If the user asks about *recent* or *current* state, prefer `git log` or reading the code over recalling the snapshot.",
];

const MEMORY_FRONTMATTER_EXAMPLE: &[&str] = &[
    "```markdown",
    "---",
    "name: {{memory name}}",
    "description: {{one-line description — used to decide relevance in future conversations, so be specific}}",
    "type: {{user, feedback, project, reference}}",
    "---",
    "",
    "{{memory content — for feedback/project types, structure as: rule/fact, then **Why:** and **How to apply:** lines}}",
    "```",
];

const MAX_MEMORY_FILES: usize = 200;
const FRONTMATTER_MAX_LINES: usize = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionMemoryFileType {
    SessionMemory,
    SessionTranscript,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryScope {
    Personal,
    Team,
}

const TYPES_SECTION_COMBINED: &[&str] = &[
    "## Types of memory",
    "",
    "There are several discrete types of memory that you can store in your memory system. Each type below declares a <scope> of `private`, `team`, or guidance for choosing between the two.",
    "",
    "<types>",
    "<type>",
    "    <name>user</name>",
    "    <scope>always private</scope>",
    "    <description>Contain information about the user's role, goals, responsibilities, and knowledge. Great user memories help you tailor your future behavior to the user's preferences and perspective. Your goal in reading and writing these memories is to build up an understanding of who the user is and how you can be most helpful to them specifically. For example, you should collaborate with a senior software engineer differently than a student who is coding for the very first time. Keep in mind, that the aim here is to be helpful to the user. Avoid writing memories about the user that could be viewed as a negative judgement or that are not relevant to the work you're trying to accomplish together.</description>",
    "    <when_to_save>When you learn any details about the user's role, preferences, responsibilities, or knowledge</when_to_save>",
    "    <how_to_use>When your work should be informed by the user's profile or perspective. For example, if the user is asking you to explain a part of the code, you should answer that question in a way that is tailored to the specific details that they will find most valuable or that helps them build their mental model in relation to domain knowledge they already have.</how_to_use>",
    "    <examples>",
    "    user: I'm a data scientist investigating what logging we have in place",
    "    assistant: [saves private user memory: user is a data scientist, currently focused on observability/logging]",
    "",
    "    user: I've been writing Go for ten years but this is my first time touching the React side of this repo",
    "    assistant: [saves private user memory: deep Go expertise, new to React and this project's frontend — frame frontend explanations in terms of backend analogues]",
    "    </examples>",
    "</type>",
    "<type>",
    "    <name>feedback</name>",
    "    <scope>default to private. Save as team only when the guidance is clearly a project-wide convention that every contributor should follow (e.g., a testing policy, a build invariant), not a personal style preference.</scope>",
    "    <description>Guidance the user has given you about how to approach work — both what to avoid and what to keep doing. These are a very important type of memory to read and write as they allow you to remain coherent and responsive to the way you should approach work in the project. Record from failure AND success: if you only save corrections, you will avoid past mistakes but drift away from approaches the user has already validated, and may grow overly cautious. Before saving a private feedback memory, check that it doesn't contradict a team feedback memory — if it does, either don't save it or note the override explicitly.</description>",
    "    <when_to_save>Any time the user corrects your approach (\"no not that\", \"don't\", \"stop doing X\") OR confirms a non-obvious approach worked (\"yes exactly\", \"perfect, keep doing that\", accepting an unusual choice without pushback). Corrections are easy to notice; confirmations are quieter — watch for them. In both cases, save what is applicable to future conversations, especially if surprising or not obvious from the code. Include *why* so you can judge edge cases later.</when_to_save>",
    "    <how_to_use>Let these memories guide your behavior so that the user and other users in the project do not need to offer the same guidance twice.</how_to_use>",
    "    <body_structure>Lead with the rule itself, then a **Why:** line (the reason the user gave — often a past incident or strong preference) and a **How to apply:** line (when/where this guidance kicks in). Knowing *why* lets you judge edge cases instead of blindly following the rule.</body_structure>",
    "    <examples>",
    "    user: don't mock the database in these tests — we got burned last quarter when mocked tests passed but the prod migration failed",
    "    assistant: [saves team feedback memory: integration tests must hit a real database, not mocks. Reason: prior incident where mock/prod divergence masked a broken migration. Team scope: this is a project testing policy, not a personal preference]",
    "",
    "    user: stop summarizing what you just did at the end of every response, I can read the diff",
    "    assistant: [saves private feedback memory: this user wants terse responses with no trailing summaries. Private because it's a communication preference, not a project convention]",
    "",
    "    user: yeah the single bundled PR was the right call here, splitting this one would've just been churn",
    "    assistant: [saves private feedback memory: for refactors in this area, user prefers one bundled PR over many small ones. Confirmed after I chose this approach — a validated judgment call, not a correction]",
    "    </examples>",
    "</type>",
    "<type>",
    "    <name>project</name>",
    "    <scope>private or team, but strongly bias toward team</scope>",
    "    <description>Information that you learn about ongoing work, goals, initiatives, bugs, or incidents within the project that is not otherwise derivable from the code or git history. Project memories help you understand the broader context and motivation behind the work users are working on within this working directory.</description>",
    "    <when_to_save>When you learn who is doing what, why, or by when. These states change relatively quickly so try to keep your understanding of this up to date. Always convert relative dates in user messages to absolute dates when saving (e.g., \"Thursday\" → \"2026-03-05\"), so the memory remains interpretable after time passes.</when_to_save>",
    "    <how_to_use>Use these memories to more fully understand the details and nuance behind the user's request, anticipate coordination issues across users, make better informed suggestions.</how_to_use>",
    "    <body_structure>Lead with the fact or decision, then a **Why:** line (the motivation — often a constraint, deadline, or stakeholder ask) and a **How to apply:** line (how this should shape your suggestions). Project memories decay fast, so the why helps future-you judge whether the memory is still load-bearing.</body_structure>",
    "    <examples>",
    "    user: we're freezing all non-critical merges after Thursday — mobile team is cutting a release branch",
    "    assistant: [saves team project memory: merge freeze begins 2026-03-05 for mobile release cut. Flag any non-critical PR work scheduled after that date]",
    "",
    "    user: the reason we're ripping out the old auth middleware is that legal flagged it for storing session tokens in a way that doesn't meet the new compliance requirements",
    "    assistant: [saves team project memory: auth middleware rewrite is driven by legal/compliance requirements around session token storage, not tech-debt cleanup — scope decisions should favor compliance over ergonomics]",
    "    </examples>",
    "</type>",
    "<type>",
    "    <name>reference</name>",
    "    <scope>usually team</scope>",
    "    <description>Stores pointers to where information can be found in external systems. These memories allow you to remember where to look to find up-to-date information outside of the project directory.</description>",
    "    <when_to_save>When you learn about resources in external systems and their purpose. For example, that bugs are tracked in a specific project in Linear or that feedback can be found in a specific Slack channel.</when_to_save>",
    "    <how_to_use>When the user references an external system or information that may be in an external system.</how_to_use>",
    "    <examples>",
    "    user: check the Linear project \"INGEST\" if you want context on these tickets, that's where we track all pipeline bugs",
    "    assistant: [saves team reference memory: pipeline bugs are tracked in Linear project \"INGEST\"]",
    "",
    "    user: the Grafana board at grafana.internal/d/api-latency is what oncall watches — if you're touching request handling, that's the thing that'll page someone",
    "    assistant: [saves team reference memory: grafana.internal/d/api-latency is the oncall latency dashboard — check it when editing request-path code]",
    "    </examples>",
    "</type>",
    "</types>",
    "",
];

#[must_use]
pub fn has_valid_cowork_memory_path_override() -> bool {
    validated_cowork_memory_path_override_from(
        std::env::var("CLAUDE_COWORK_MEMORY_PATH_OVERRIDE").ok(),
    )
    .is_some()
}

pub fn default_memory_dir_for_permissions(config: &RuntimeConfig) -> Result<Option<PathBuf>> {
    let inputs = AutoMemoryInputs::from_process_env(config)?;
    if !inputs.auto_memory_enabled || inputs.cowork_memory_path_override.is_some() {
        return Ok(None);
    }

    Ok(Some(PathBuf::from(resolve_default_memory_dir(
        config, &inputs,
    )?)))
}

pub fn memory_dir_for_read_permissions(config: &RuntimeConfig) -> Result<Option<PathBuf>> {
    let inputs = AutoMemoryInputs::from_process_env(config)?;
    if !inputs.auto_memory_enabled {
        return Ok(None);
    }

    Ok(Some(PathBuf::from(resolve_default_memory_dir(
        config, &inputs,
    )?)))
}

pub fn memory_base_dir() -> PathBuf {
    std::env::var_os("CLAUDE_CODE_REMOTE_MEMORY_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(claude_config_home_dir)
}

pub fn claude_config_home() -> PathBuf {
    claude_config_home_dir()
}

pub fn auto_memory_path(config: &RuntimeConfig) -> Result<Option<PathBuf>> {
    let inputs = AutoMemoryInputs::from_process_env(config)?;
    if !inputs.auto_memory_enabled {
        return Ok(None);
    }
    Ok(Some(PathBuf::from(resolve_default_memory_dir(
        config, &inputs,
    )?)))
}

pub fn auto_memory_entrypoint(config: &RuntimeConfig) -> Result<Option<PathBuf>> {
    Ok(auto_memory_path(config)?.map(|path| path.join(ENTRYPOINT_NAME)))
}

pub fn auto_memory_daily_log_path(
    config: &RuntimeConfig,
    date: chrono::NaiveDate,
) -> Result<Option<PathBuf>> {
    Ok(auto_memory_path(config)?.map(|path| {
        path.join("logs")
            .join(date.format("%Y").to_string())
            .join(date.format("%m").to_string())
            .join(format!("{}.md", date.format("%Y-%m-%d")))
    }))
}

pub fn team_memory_path_with_features(
    config: &RuntimeConfig,
    features: &MemoryPromptFeatures,
) -> Result<Option<PathBuf>> {
    team_memory_dir_for_read_permissions_with_features(config, features)
}

pub fn team_memory_entrypoint_with_features(
    config: &RuntimeConfig,
    features: &MemoryPromptFeatures,
) -> Result<Option<PathBuf>> {
    Ok(team_memory_path_with_features(config, features)?.map(|path| path.join(ENTRYPOINT_NAME)))
}

pub fn is_auto_memory_path(config: &RuntimeConfig, path: &Path) -> bool {
    auto_memory_path(config)
        .ok()
        .flatten()
        .is_some_and(|dir| path_starts_with_normalized(path, &dir))
}

pub fn is_team_memory_path_with_features(
    config: &RuntimeConfig,
    features: &MemoryPromptFeatures,
    path: &Path,
) -> bool {
    team_memory_path_with_features(config, features)
        .ok()
        .flatten()
        .is_some_and(|dir| path_starts_with_normalized(&absolute_normalized_path(path), &dir))
}

pub fn is_team_memory_file_with_features(
    config: &RuntimeConfig,
    features: &MemoryPromptFeatures,
    path: &Path,
) -> bool {
    features.team_memory_enabled && is_team_memory_path_with_features(config, features, path)
}

pub fn memory_scope_for_path_with_features(
    config: &RuntimeConfig,
    features: &MemoryPromptFeatures,
    path: &Path,
) -> Option<MemoryScope> {
    if is_team_memory_file_with_features(config, features, path) {
        return Some(MemoryScope::Team);
    }
    if is_auto_memory_path(config, path) {
        return Some(MemoryScope::Personal);
    }
    None
}

pub fn agent_memory_dir(
    config: &RuntimeConfig,
    agent_type: &str,
    scope: claude_agents::definition::AgentMemoryScope,
) -> PathBuf {
    let dir_name = sanitize_agent_type_for_path(agent_type);
    match scope {
        claude_agents::definition::AgentMemoryScope::User => {
            memory_base_dir().join("agent-memory").join(dir_name)
        }
        claude_agents::definition::AgentMemoryScope::Project => config
            .original_cwd
            .join(".claude")
            .join("agent-memory")
            .join(dir_name),
        claude_agents::definition::AgentMemoryScope::Local => {
            local_agent_memory_dir(config, &dir_name)
        }
    }
}

pub fn agent_memory_entrypoint(
    config: &RuntimeConfig,
    agent_type: &str,
    scope: claude_agents::definition::AgentMemoryScope,
) -> PathBuf {
    agent_memory_dir(config, agent_type, scope).join(ENTRYPOINT_NAME)
}

pub fn agent_memory_dirs(config: &RuntimeConfig) -> Vec<PathBuf> {
    let mut dirs = vec![
        memory_base_dir().join("agent-memory"),
        config.original_cwd.join(".claude").join("agent-memory"),
    ];
    if std::env::var_os("CLAUDE_CODE_REMOTE_MEMORY_DIR").is_some() {
        dirs.push(memory_base_dir().join(AUTO_MEMORY_PROJECTS_DIRNAME));
    } else {
        dirs.push(
            config
                .original_cwd
                .join(".claude")
                .join("agent-memory-local"),
        );
    }
    dirs.sort();
    dirs.dedup();
    dirs
}

pub fn is_agent_memory_path(config: &RuntimeConfig, path: &Path) -> bool {
    if !is_auto_memory_enabled(config) {
        return false;
    }
    let normalized = comparable_path(&absolute_normalized_path(path));
    agent_memory_dirs(config).into_iter().any(|root| {
        let root_cmp = comparable_path(&root);
        if root
            .file_name()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value == AUTO_MEMORY_PROJECTS_DIRNAME)
        {
            return is_same_or_child_dir_cmp(&normalized, &root_cmp)
                && normalized.contains("/agent-memory-local/");
        }
        is_same_or_child_dir_cmp(&normalized, &root_cmp)
    })
}

pub fn detect_session_file_type(file_path: &Path) -> Option<SessionMemoryFileType> {
    let normalized = comparable_path(file_path);
    let config_dir = comparable_path(&claude_config_home_dir());
    if !normalized.starts_with(&config_dir) {
        return None;
    }
    if normalized.contains("/session-memory/") && normalized.ends_with(".md") {
        return Some(SessionMemoryFileType::SessionMemory);
    }
    if normalized.contains("/projects/") && normalized.ends_with(".jsonl") {
        return Some(SessionMemoryFileType::SessionTranscript);
    }
    None
}

pub fn detect_session_pattern_type(pattern: &str) -> Option<SessionMemoryFileType> {
    let normalized = comparable_str(pattern);
    if normalized.contains("session-memory")
        && (normalized.contains(".md") || normalized.ends_with('*'))
    {
        return Some(SessionMemoryFileType::SessionMemory);
    }
    if normalized.contains(".jsonl")
        || (normalized.contains("projects") && normalized.contains("*.jsonl"))
    {
        return Some(SessionMemoryFileType::SessionTranscript);
    }
    None
}

pub fn is_auto_managed_memory_file_with_features(
    config: &RuntimeConfig,
    features: &MemoryPromptFeatures,
    file_path: &Path,
) -> bool {
    is_auto_memory_path(config, file_path)
        || is_team_memory_file_with_features(config, features, file_path)
        || detect_session_file_type(file_path).is_some()
        || is_agent_memory_path(config, file_path)
}

pub fn is_memory_directory_with_features(
    config: &RuntimeConfig,
    features: &MemoryPromptFeatures,
    dir_path: &Path,
) -> bool {
    let normalized_path = normalize_memory_pathbuf(dir_path);
    let normalized_cmp = comparable_path(&normalized_path);

    if is_auto_memory_enabled(config)
        && (normalized_cmp.contains("/agent-memory/")
            || normalized_cmp.contains("/agent-memory-local/"))
    {
        return true;
    }

    if features.team_memory_enabled
        && is_team_memory_path_with_features(config, features, &normalized_path)
    {
        return true;
    }

    if is_auto_memory_enabled(config)
        && let Ok(Some(auto_dir)) = auto_memory_path(config)
    {
        let auto_dir_cmp = comparable_path(&strip_trailing_separator(&auto_dir));
        let auto_path_cmp = comparable_path(&auto_dir);
        if normalized_cmp == auto_dir_cmp || normalized_cmp.starts_with(&auto_path_cmp) {
            return true;
        }
    }

    let config_dir_cmp = comparable_path(&claude_config_home_dir());
    let memory_base_cmp = comparable_path(&memory_base_dir());
    let under_config = normalized_cmp.starts_with(&config_dir_cmp);
    let under_memory_base = normalized_cmp.starts_with(&memory_base_cmp);
    if !under_config && !under_memory_base {
        return false;
    }
    if normalized_cmp.contains("/session-memory/") {
        return true;
    }
    if under_config && normalized_cmp.contains("/projects/") {
        return true;
    }
    if is_auto_memory_enabled(config) && normalized_cmp.contains("/memory/") {
        return true;
    }
    false
}

pub fn is_shell_command_targeting_memory_with_features(
    config: &RuntimeConfig,
    features: &MemoryPromptFeatures,
    command: &str,
) -> bool {
    let command_cmp = comparable_str(command);
    let mut dirs = vec![claude_config_home_dir(), memory_base_dir()];
    if let Ok(Some(auto_dir)) = auto_memory_path(config) {
        dirs.push(strip_trailing_separator(&auto_dir));
    }
    let matches_any_dir = dirs.iter().any(|dir| {
        let comparable = comparable_path(dir);
        command_cmp.contains(&comparable)
            || (cfg!(windows)
                && command_cmp.contains(&windows_path_to_posix_path(dir).to_ascii_lowercase()))
    });
    if !matches_any_dir {
        return false;
    }

    let path_like_tokens =
        Regex::new(r#"(?:[A-Za-z]:[/\\]|/)[^\s'"]+"#).expect("valid memory shell path regex");
    path_like_tokens.find_iter(command).any(|capture| {
        let cleaned = capture.as_str().trim_end_matches([',', ';', '|', '&', '>']);
        let path = shell_capture_to_path(cleaned);
        is_auto_managed_memory_file_with_features(config, features, &path)
            || is_memory_directory_with_features(config, features, &path)
    })
}

pub fn is_auto_managed_memory_pattern(config: &RuntimeConfig, pattern: &str) -> bool {
    if detect_session_pattern_type(pattern).is_some() {
        return true;
    }
    if !is_auto_memory_enabled(config) {
        return false;
    }
    let comparable = comparable_str(pattern);
    comparable.contains("agent-memory/") || comparable.contains("agent-memory-local/")
}

pub fn load_cowork_memory_mechanics_prompt(config: &RuntimeConfig) -> Result<Option<String>> {
    load_cowork_memory_mechanics_prompt_with(config, &AutoMemoryInputs::from_process_env(config)?)
}

#[derive(Debug, Clone, Default)]
pub struct MemoryPromptFeatures {
    pub team_memory_enabled: bool,
    pub skip_index: bool,
    pub searching_past_context_enabled: bool,
    pub kairos_active: bool,
    pub embedded_search_tools: bool,
    pub repl_mode_active: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryType {
    User,
    Feedback,
    Project,
    Reference,
}

impl MemoryType {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Feedback => "feedback",
            Self::Project => "project",
            Self::Reference => "reference",
        }
    }
}

#[must_use]
pub fn parse_memory_type(raw: &str) -> Option<MemoryType> {
    match raw {
        "user" => Some(MemoryType::User),
        "feedback" => Some(MemoryType::Feedback),
        "project" => Some(MemoryType::Project),
        "reference" => Some(MemoryType::Reference),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MemoryHeader {
    pub filename: String,
    pub file_path: PathBuf,
    pub mtime_ms: f64,
    pub description: Option<String>,
    pub memory_type: Option<MemoryType>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SurfacedMemoryContent {
    pub path: PathBuf,
    pub content: String,
    pub mtime_ms: f64,
    pub line_limit: Option<usize>,
}

pub fn team_memory_dir_for_read_permissions_with_features(
    config: &RuntimeConfig,
    features: &MemoryPromptFeatures,
) -> Result<Option<PathBuf>> {
    let mut inputs = AutoMemoryInputs::from_process_env(config)?;
    inputs.team_memory_enabled = features.team_memory_enabled;
    if !is_team_memory_enabled(&inputs) {
        return Ok(None);
    }

    Ok(Some(team_memory_dir(config, &inputs)?))
}

pub fn is_auto_memory_enabled(config: &RuntimeConfig) -> bool {
    AutoMemoryInputs::from_process_env(config)
        .map(|inputs| inputs.auto_memory_enabled)
        .unwrap_or(false)
}

pub fn load_default_memory_prompt_with_features(
    config: &RuntimeConfig,
    features: &MemoryPromptFeatures,
) -> Result<Option<String>> {
    let mut inputs = AutoMemoryInputs::from_process_env(config)?;
    inputs.team_memory_enabled = features.team_memory_enabled;
    load_default_memory_prompt_with(config, &inputs, features)
}

#[must_use]
pub fn build_extract_auto_only_prompt(
    new_message_count: usize,
    existing_memories: &str,
    skip_index: bool,
) -> String {
    let mut how_to_save = if skip_index {
        vec![
            "## How to save memories".to_owned(),
            String::new(),
            "Write each memory to its own file (e.g., `user_role.md`, `feedback_testing.md`) using this frontmatter format:".to_owned(),
            String::new(),
        ]
    } else {
        vec![
            "## How to save memories".to_owned(),
            String::new(),
            "Saving a memory is a two-step process:".to_owned(),
            String::new(),
            "**Step 1** — write the memory to its own file (e.g., `user_role.md`, `feedback_testing.md`) using this frontmatter format:".to_owned(),
            String::new(),
        ]
    };
    how_to_save.extend(
        MEMORY_FRONTMATTER_EXAMPLE
            .iter()
            .map(|line| (*line).to_owned()),
    );
    if skip_index {
        how_to_save.extend([
            String::new(),
            "- Organize memory semantically by topic, not chronologically".to_owned(),
            "- Update or remove memories that turn out to be wrong or outdated".to_owned(),
            "- Do not write duplicate memories. First check if there is an existing memory you can update before writing a new one.".to_owned(),
        ]);
    } else {
        how_to_save.extend([
            String::new(),
            format!(
                "**Step 2** — add a pointer to that file in `{ENTRYPOINT_NAME}`. `{ENTRYPOINT_NAME}` is an index, not a memory — each entry should be one line, under ~150 characters: `- [Title](file.md) — one-line hook`. It has no frontmatter. Never write memory content directly into `{ENTRYPOINT_NAME}`."
            ),
            String::new(),
            format!(
                "- `{ENTRYPOINT_NAME}` is always loaded into your system prompt — lines after {MAX_ENTRYPOINT_LINES} will be truncated, so keep the index concise"
            ),
            "- Organize memory semantically by topic, not chronologically".to_owned(),
            "- Update or remove memories that turn out to be wrong or outdated".to_owned(),
            "- Do not write duplicate memories. First check if there is an existing memory you can update before writing a new one.".to_owned(),
        ]);
    }

    let mut lines = vec![
        extract_prompt_opener(new_message_count, existing_memories),
        String::new(),
        "If the user explicitly asks you to remember something, save it immediately as whichever type fits best. If they ask you to forget something, find and remove the relevant entry.".to_owned(),
        String::new(),
    ];
    lines.extend(
        TYPES_SECTION_INDIVIDUAL
            .iter()
            .map(|line| (*line).to_owned()),
    );
    lines.extend(
        WHAT_NOT_TO_SAVE_SECTION
            .iter()
            .map(|line| (*line).to_owned()),
    );
    lines.push(String::new());
    lines.extend(how_to_save);
    lines.join("\n")
}

#[must_use]
pub fn build_extract_combined_prompt(
    new_message_count: usize,
    existing_memories: &str,
    skip_index: bool,
) -> String {
    let mut how_to_save = if skip_index {
        vec![
            "## How to save memories".to_owned(),
            String::new(),
            "Write each memory to its own file in the chosen directory (private or team, per the type's scope guidance) using this frontmatter format:".to_owned(),
            String::new(),
        ]
    } else {
        vec![
            "## How to save memories".to_owned(),
            String::new(),
            "Saving a memory is a two-step process:".to_owned(),
            String::new(),
            "**Step 1** — write the memory to its own file in the chosen directory (private or team, per the type's scope guidance) using this frontmatter format:".to_owned(),
            String::new(),
        ]
    };
    how_to_save.extend(
        MEMORY_FRONTMATTER_EXAMPLE
            .iter()
            .map(|line| (*line).to_owned()),
    );
    if skip_index {
        how_to_save.extend([
            String::new(),
            "- Organize memory semantically by topic, not chronologically".to_owned(),
            "- Update or remove memories that turn out to be wrong or outdated".to_owned(),
            "- Do not write duplicate memories. First check if there is an existing memory you can update before writing a new one.".to_owned(),
        ]);
    } else {
        how_to_save.extend([
            String::new(),
            format!(
                "**Step 2** — add a pointer to that file in the same directory's `{ENTRYPOINT_NAME}`. Each directory (private and team) has its own `{ENTRYPOINT_NAME}` index — each entry should be one line, under ~150 characters: `- [Title](file.md) — one-line hook`. They have no frontmatter. Never write memory content directly into a `{ENTRYPOINT_NAME}`."
            ),
            String::new(),
            format!(
                "- Both `{ENTRYPOINT_NAME}` indexes are loaded into your system prompt — lines after {MAX_ENTRYPOINT_LINES} will be truncated, so keep them concise"
            ),
            "- Organize memory semantically by topic, not chronologically".to_owned(),
            "- Update or remove memories that turn out to be wrong or outdated".to_owned(),
            "- Do not write duplicate memories. First check if there is an existing memory you can update before writing a new one.".to_owned(),
        ]);
    }

    let mut lines = vec![
        extract_prompt_opener(new_message_count, existing_memories),
        String::new(),
        "If the user explicitly asks you to remember something, save it immediately as whichever type fits best. If they ask you to forget something, find and remove the relevant entry.".to_owned(),
        String::new(),
    ];
    lines.extend(TYPES_SECTION_COMBINED.iter().map(|line| (*line).to_owned()));
    lines.extend(
        WHAT_NOT_TO_SAVE_SECTION
            .iter()
            .map(|line| (*line).to_owned()),
    );
    lines.extend([
        "- You MUST avoid saving sensitive data within shared team memories. For example, never save API keys or user credentials.".to_owned(),
        String::new(),
    ]);
    lines.extend(how_to_save);
    lines.join("\n")
}

#[must_use]
fn extract_prompt_opener(new_message_count: usize, existing_memories: &str) -> String {
    let manifest = if existing_memories.is_empty() {
        String::new()
    } else {
        format!(
            "\n\n## Existing memory files\n\n{existing_memories}\n\nCheck this list before writing — update an existing file rather than creating a duplicate."
        )
    };

    [
        format!(
            "You are now acting as the memory extraction subagent. Analyze the most recent ~{new_message_count} messages above and use them to update your persistent memory systems."
        ),
        String::new(),
        "Available tools: Read, Grep, Glob, read-only Bash (ls/find/cat/stat/wc/head/tail and similar), and Edit/Write for paths inside the memory directory only. Bash rm is not permitted. All other tools — MCP, Agent, write-capable Bash, etc — will be denied.".to_owned(),
        String::new(),
        "You have a limited turn budget. Edit requires a prior Read of the same file, so the efficient strategy is: turn 1 — issue all Read calls in parallel for every file you might update; turn 2 — issue all Write/Edit calls in parallel. Do not interleave reads and writes across multiple turns.".to_owned(),
        String::new(),
        format!(
            "You MUST only use content from the last ~{new_message_count} messages to update your persistent memories. Do not waste any turns attempting to investigate or verify that content further — no grepping source files, no reading code to confirm a pattern exists, no git commands.{manifest}"
        ),
    ]
    .join("\n")
}

#[must_use]
pub fn scan_memory_files(memory_dir: &Path) -> Vec<MemoryHeader> {
    let mut headers = Vec::new();
    for entry in WalkDir::new(memory_dir)
        .into_iter()
        .filter_map(std::result::Result::ok)
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("md") {
            continue;
        }
        if path.file_name().and_then(|value| value.to_str()) == Some(ENTRYPOINT_NAME) {
            continue;
        }
        let Some(header) = read_memory_header(memory_dir, path) else {
            continue;
        };
        headers.push(header);
    }
    headers.sort_by(|left, right| right.mtime_ms.total_cmp(&left.mtime_ms));
    headers.truncate(MAX_MEMORY_FILES);
    headers
}

#[must_use]
pub fn format_memory_manifest(memories: &[MemoryHeader]) -> String {
    memories
        .iter()
        .map(|memory| {
            let tag = memory
                .memory_type
                .map(|kind| format!("[{}] ", kind.as_str()))
                .unwrap_or_default();
            let ts = iso_timestamp_from_mtime_ms(memory.mtime_ms);
            match memory.description.as_deref() {
                Some(description) if !description.is_empty() => {
                    format!("- {tag}{} ({ts}): {description}", memory.filename)
                }
                _ => format!("- {tag}{} ({ts})", memory.filename),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[allow(dead_code)]
pub fn collect_surfaced_memory_bytes(
    conversation: &[claude_core::ConversationEntry],
    marker_prefix: &str,
) -> (std::collections::BTreeSet<String>, usize) {
    let mut paths = std::collections::BTreeSet::new();
    let mut total_bytes = 0usize;
    for entry in conversation {
        let Some(history_text) = entry.history_text.as_deref() else {
            continue;
        };
        let Some(payload) = history_text.strip_prefix(marker_prefix) else {
            continue;
        };
        let Ok(memories) = serde_json::from_str::<Vec<SurfacedMemoryContent>>(payload) else {
            continue;
        };
        for memory in memories {
            paths.insert(memory.path.display().to_string());
            total_bytes = total_bytes.saturating_add(memory.content.len());
        }
    }
    (paths, total_bytes)
}

#[allow(dead_code)]
pub fn read_memories_for_surfacing(
    memories: &[MemoryHeader],
    max_lines: usize,
    max_bytes: usize,
) -> Vec<SurfacedMemoryContent> {
    memories
        .iter()
        .filter_map(|memory| {
            let raw = fs::read_to_string(&memory.file_path).ok()?;
            let mut selected = Vec::new();
            let mut bytes = 0usize;
            let mut truncated = false;

            for (index, line) in raw.lines().enumerate() {
                if index >= max_lines {
                    truncated = true;
                    break;
                }
                let line_bytes = line.len() + 1;
                if bytes + line_bytes > max_bytes {
                    truncated = true;
                    break;
                }
                selected.push(line.to_owned());
                bytes += line_bytes;
            }

            let mut content = selected.join("\n").trim_end().to_owned();
            if truncated {
                if !content.is_empty() {
                    content.push_str("\n\n");
                }
                content.push_str(
                    "[truncated memory; use the Read tool on this file if you need the full contents]",
                );
            }

            Some(SurfacedMemoryContent {
                path: memory.file_path.clone(),
                content,
                mtime_ms: memory.mtime_ms,
                line_limit: truncated.then_some(max_lines),
            })
        })
        .collect()
}

fn read_memory_header(memory_dir: &Path, file_path: &Path) -> Option<MemoryHeader> {
    let metadata = fs::metadata(file_path).ok()?;
    let mtime_ms = metadata
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_secs_f64()
        * 1000.0;
    let content = fs::read_to_string(file_path).ok()?;
    let frontmatter = parse_frontmatter_fields(&first_lines(&content, FRONTMATTER_MAX_LINES));
    let filename = file_path
        .strip_prefix(memory_dir)
        .ok()
        .unwrap_or(file_path)
        .to_string_lossy()
        .replace('\\', "/");
    Some(MemoryHeader {
        filename,
        file_path: file_path.to_path_buf(),
        mtime_ms,
        description: frontmatter.get("description").cloned(),
        memory_type: frontmatter
            .get("type")
            .and_then(|value| parse_memory_type(value)),
    })
}

fn first_lines(content: &str, max_lines: usize) -> String {
    content
        .lines()
        .take(max_lines)
        .collect::<Vec<_>>()
        .join("\n")
}

fn parse_frontmatter_fields(content: &str) -> std::collections::BTreeMap<String, String> {
    let mut fields = std::collections::BTreeMap::new();
    let mut lines = content.lines();
    if lines.next() != Some("---") {
        return fields;
    }

    for line in lines {
        if line.trim() == "---" {
            break;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        fields.insert(
            key.trim().to_owned(),
            value.trim().trim_matches('"').trim_matches('\'').to_owned(),
        );
    }
    fields
}

fn iso_timestamp_from_mtime_ms(mtime_ms: f64) -> String {
    let secs = (mtime_ms / 1000.0).floor() as i64;
    let millis = (mtime_ms.rem_euclid(1000.0).round() as u32).min(999);
    let nanos = millis * 1_000_000;
    chrono::DateTime::<chrono::Utc>::from_timestamp(secs, nanos)
        .unwrap_or(chrono::DateTime::<chrono::Utc>::UNIX_EPOCH)
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn load_cowork_memory_mechanics_prompt_with(
    config: &RuntimeConfig,
    inputs: &AutoMemoryInputs,
) -> Result<Option<String>> {
    if !inputs.auto_memory_enabled {
        return Ok(None);
    }
    let Some(memory_dir) =
        validated_cowork_memory_path_override_from(inputs.cowork_memory_path_override.clone())
    else {
        return Ok(None);
    };
    let _ = fs::create_dir_all(Path::new(&memory_dir));

    Ok(Some(
        build_memory_lines(
            config,
            "auto memory",
            &memory_dir,
            inputs.cowork_memory_extra_guidelines.clone(),
            false,
            &MemoryPromptFeatures::default(),
        )
        .join("\n"),
    ))
}

pub(crate) fn truncate_entrypoint_content(raw: &str) -> String {
    let trimmed = raw.trim();
    let line_count = trimmed.lines().count();
    let byte_count = trimmed.len();
    let was_line_truncated = line_count > MAX_ENTRYPOINT_LINES;
    let was_byte_truncated = byte_count > MAX_ENTRYPOINT_BYTES;

    if !was_line_truncated && !was_byte_truncated {
        return trimmed.to_owned();
    }

    let mut truncated = if was_line_truncated {
        trimmed
            .lines()
            .take(MAX_ENTRYPOINT_LINES)
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        trimmed.to_owned()
    };

    if truncated.len() > MAX_ENTRYPOINT_BYTES {
        if let Some(cut_at) = truncated[..MAX_ENTRYPOINT_BYTES].rfind('\n') {
            truncated.truncate(if cut_at > 0 {
                cut_at
            } else {
                MAX_ENTRYPOINT_BYTES
            });
        } else {
            truncated.truncate(MAX_ENTRYPOINT_BYTES);
        }
    }

    let reason = if was_byte_truncated && !was_line_truncated {
        format!("{byte_count} bytes (limit: {MAX_ENTRYPOINT_BYTES}) — index entries are too long")
    } else if was_line_truncated && !was_byte_truncated {
        format!("{line_count} lines (limit: {MAX_ENTRYPOINT_LINES})")
    } else {
        format!("{line_count} lines and {byte_count} bytes")
    };

    format!(
        "{truncated}\n\n> WARNING: {ENTRYPOINT_NAME} is {reason}. Only part of it was loaded. Keep index entries to one line under ~200 chars; move detail into topic files."
    )
}

#[allow(dead_code)]
fn append_entrypoint_section(lines: &mut Vec<String>, memory_dir: &str) {
    let entrypoint_path = Path::new(memory_dir).join(ENTRYPOINT_NAME);
    match fs::read_to_string(&entrypoint_path) {
        Ok(raw) if !raw.trim().is_empty() => {
            let content = truncate_entrypoint_content(&raw);
            lines.push(format!("## {ENTRYPOINT_NAME}"));
            lines.push(String::new());
            lines.push(content);
        }
        _ => {
            lines.push(format!("## {ENTRYPOINT_NAME}"));
            lines.push(String::new());
            lines.push(format!(
                "Your {ENTRYPOINT_NAME} is currently empty. When you save new memories, they will appear here."
            ));
        }
    }
}

fn load_default_memory_prompt_with(
    config: &RuntimeConfig,
    inputs: &AutoMemoryInputs,
    features: &MemoryPromptFeatures,
) -> Result<Option<String>> {
    if !inputs.auto_memory_enabled {
        return Ok(None);
    }

    if features.kairos_active {
        let memory_dir = resolve_default_memory_dir(config, inputs)?;
        let _ = fs::create_dir_all(Path::new(&memory_dir));
        return Ok(Some(build_assistant_daily_log_prompt(
            config,
            &memory_dir,
            features.skip_index,
            features,
        )));
    }

    let memory_dir = resolve_default_memory_dir(config, inputs)?;
    if is_team_memory_enabled(inputs) {
        let team_dir = team_memory_dir(config, inputs)?;
        let _ = fs::create_dir_all(&team_dir);
        return Ok(Some(build_combined_memory_prompt(
            config,
            &memory_dir,
            &with_trailing_separator(team_dir),
            inputs.cowork_memory_extra_guidelines.clone(),
            features.skip_index,
            features,
        )));
    }
    let _ = fs::create_dir_all(Path::new(&memory_dir));

    Ok(Some(
        build_memory_lines(
            config,
            "auto memory",
            &memory_dir,
            inputs.cowork_memory_extra_guidelines.clone(),
            features.skip_index,
            features,
        )
        .join("\n"),
    ))
}

#[derive(Debug, Clone, Default)]
struct AutoMemoryInputs {
    auto_memory_enabled: bool,
    cowork_memory_path_override: Option<String>,
    cowork_memory_extra_guidelines: Option<String>,
    remote_memory_dir: Option<std::ffi::OsString>,
    team_memory_enabled: bool,
}

impl AutoMemoryInputs {
    fn from_process_env(config: &RuntimeConfig) -> Result<Self> {
        let remote_memory_dir = std::env::var_os("CLAUDE_CODE_REMOTE_MEMORY_DIR");
        Ok(Self {
            auto_memory_enabled: resolve_auto_memory_enabled(
                config,
                std::env::var("CLAUDE_CODE_DISABLE_AUTO_MEMORY").ok(),
                std::env::var("CLAUDE_CODE_SIMPLE").ok(),
                std::env::var("CLAUDE_CODE_REMOTE").ok(),
                remote_memory_dir.clone(),
            )?,
            cowork_memory_path_override: std::env::var("CLAUDE_COWORK_MEMORY_PATH_OVERRIDE").ok(),
            cowork_memory_extra_guidelines: std::env::var("CLAUDE_COWORK_MEMORY_EXTRA_GUIDELINES")
                .ok()
                .filter(|value| !value.trim().is_empty()),
            remote_memory_dir,
            team_memory_enabled: false,
        })
    }
}

fn is_team_memory_enabled(inputs: &AutoMemoryInputs) -> bool {
    inputs.auto_memory_enabled && inputs.team_memory_enabled
}

fn resolve_auto_memory_enabled(
    config: &RuntimeConfig,
    disable_auto_memory_env: Option<String>,
    simple_env: Option<String>,
    remote_env: Option<String>,
    remote_memory_dir: Option<std::ffi::OsString>,
) -> Result<bool> {
    if env_truthy(disable_auto_memory_env.as_deref()) {
        return Ok(false);
    }
    if env_defined_falsy(disable_auto_memory_env.as_deref()) {
        return Ok(true);
    }
    if env_truthy(simple_env.as_deref()) {
        return Ok(false);
    }
    if env_truthy(remote_env.as_deref()) && remote_memory_dir.is_none() {
        return Ok(false);
    }
    let settings = load_runtime_settings(&config.settings_files)?;
    if let Some(auto_memory_enabled) = settings.auto_memory_enabled {
        return Ok(auto_memory_enabled);
    }
    Ok(true)
}

fn env_truthy(value: Option<&str>) -> bool {
    value.is_some_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

fn env_defined_falsy(value: Option<&str>) -> bool {
    value.is_some_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "" | "0" | "false" | "no" | "off"
        )
    })
}

fn validated_cowork_memory_path_override_from(raw: Option<String>) -> Option<String> {
    validate_memory_path(raw.as_deref(), false)
}

fn validated_auto_memory_directory_setting_from(raw: Option<String>) -> Option<String> {
    validate_memory_path(raw.as_deref(), true)
}

fn validate_memory_path(raw: Option<&str>, expand_tilde: bool) -> Option<String> {
    let raw = raw?;
    if raw.is_empty() || raw.contains('\0') || raw.starts_with("\\\\") || raw.starts_with("//") {
        return None;
    }

    let candidate = if expand_tilde {
        expand_home_tilde(raw)?
    } else {
        raw.to_owned()
    };

    let normalized = normalize_memory_path(&candidate);
    let stripped = normalized.trim_end_matches(['/', '\\']);
    if stripped.is_empty()
        || !Path::new(stripped).is_absolute()
        || stripped.len() < 3
        || is_windows_drive_root(stripped)
    {
        return None;
    }

    Some(format!("{stripped}{MAIN_SEPARATOR}"))
}

fn normalize_memory_path(raw: &str) -> String {
    let mut normalized = PathBuf::new();
    for component in Path::new(raw).components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized.to_string_lossy().into_owned()
}

fn normalize_memory_pathbuf(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

fn absolute_normalized_path(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    };
    normalize_memory_pathbuf(&absolute)
}

fn comparable_path(path: &Path) -> String {
    let rendered = normalize_memory_pathbuf(path)
        .to_string_lossy()
        .replace('\\', "/");
    if cfg!(windows) {
        rendered.to_ascii_lowercase()
    } else {
        rendered
    }
}

fn comparable_str(value: &str) -> String {
    let rendered = value.replace('\\', "/");
    if cfg!(windows) {
        rendered.to_ascii_lowercase()
    } else {
        rendered
    }
}

fn is_same_or_child_dir_cmp(path: &str, dir: &str) -> bool {
    let dir = dir.trim_end_matches('/');
    path == dir || path.starts_with(&format!("{dir}/"))
}

fn path_starts_with_normalized(path: &Path, dir: &Path) -> bool {
    let path = comparable_path(&normalize_memory_pathbuf(path));
    let dir = comparable_path(dir);
    path.starts_with(&format!("{}/", dir.trim_end_matches('/')))
}

fn strip_trailing_separator(path: &Path) -> PathBuf {
    let rendered = path.to_string_lossy();
    PathBuf::from(rendered.trim_end_matches(['/', '\\']))
}

fn windows_path_to_posix_path(path: &Path) -> String {
    let rendered = path.to_string_lossy().replace('\\', "/");
    let bytes = rendered.as_bytes();
    if bytes.len() >= 3 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
        format!(
            "/{}/{}",
            rendered[..1].to_ascii_lowercase(),
            rendered[3..].trim_start_matches('/')
        )
    } else {
        rendered
    }
}

fn shell_capture_to_path(cleaned: &str) -> PathBuf {
    if cfg!(windows)
        && cleaned.starts_with('/')
        && cleaned.len() > 3
        && cleaned.as_bytes()[2] == b'/'
        && cleaned.as_bytes()[1].is_ascii_alphabetic()
    {
        return PathBuf::from(format!("{}:/{}", &cleaned[1..2], &cleaned[3..]));
    }
    PathBuf::from(cleaned)
}

fn is_windows_drive_root(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() == 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic()
}

fn expand_home_tilde(raw: &str) -> Option<String> {
    if !(raw.starts_with("~/") || raw.starts_with("~\\")) {
        return Some(raw.to_owned());
    }

    let rest = &raw[2..];
    let normalized_rest = normalize_memory_path(if rest.is_empty() { "." } else { rest });
    if normalized_rest.is_empty() || normalized_rest == "." || normalized_rest == ".." {
        return None;
    }

    let home = home_dir_from_env()?;
    Some(home.join(rest).to_string_lossy().into_owned())
}

fn home_dir_from_env() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("USERPROFILE").map(PathBuf::from)
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("HOME").map(PathBuf::from)
    }
}

fn resolve_default_memory_dir(config: &RuntimeConfig, inputs: &AutoMemoryInputs) -> Result<String> {
    if let Some(memory_dir) =
        validated_cowork_memory_path_override_from(inputs.cowork_memory_path_override.clone())
    {
        return Ok(memory_dir);
    }

    if let Some(memory_dir) = trusted_auto_memory_directory_from_config(config)?
        .and_then(|value| validated_auto_memory_directory_setting_from(Some(value)))
    {
        return Ok(memory_dir);
    }

    Ok(default_auto_memory_dir(
        config,
        inputs.remote_memory_dir.as_deref(),
    ))
}

fn team_memory_dir(config: &RuntimeConfig, inputs: &AutoMemoryInputs) -> Result<PathBuf> {
    Ok(PathBuf::from(resolve_default_memory_dir(config, inputs)?).join("team"))
}

fn sanitize_agent_type_for_path(agent_type: &str) -> String {
    agent_type.replace(':', "-")
}

fn local_agent_memory_dir(config: &RuntimeConfig, dir_name: &str) -> PathBuf {
    if let Some(remote_memory_dir) = std::env::var_os("CLAUDE_CODE_REMOTE_MEMORY_DIR") {
        return PathBuf::from(remote_memory_dir)
            .join(AUTO_MEMORY_PROJECTS_DIRNAME)
            .join(sanitize_path_component(
                &canonical_project_root(&config.original_cwd).to_string_lossy(),
            ))
            .join("agent-memory-local")
            .join(dir_name);
    }
    config
        .original_cwd
        .join(".claude")
        .join("agent-memory-local")
        .join(dir_name)
}

fn trusted_auto_memory_directory_from_config(config: &RuntimeConfig) -> Result<Option<String>> {
    let trusted_files = trusted_auto_memory_setting_files(config);
    if trusted_files.is_empty() {
        return Ok(None);
    }

    Ok(load_runtime_settings(&trusted_files)?.auto_memory_directory)
}

fn trusted_auto_memory_setting_files(config: &RuntimeConfig) -> Vec<PathBuf> {
    if !config.cli_settings_files.is_empty() {
        return dedup_paths(&config.cli_settings_files);
    }

    let legacy_import = config
        .paths
        .profiles_dir
        .join("legacy-import")
        .join("settings.json");
    let profile = config.paths.profile_dir.join("settings.json");

    dedup_paths(
        &config
            .settings_files
            .iter()
            .filter(|path| {
                path.as_path() == legacy_import.as_path()
                    || path.as_path() == profile.as_path()
                    || path
                        .file_name()
                        .and_then(|value| value.to_str())
                        .is_some_and(|value| value.eq_ignore_ascii_case("settings.local.json"))
            })
            .cloned()
            .collect::<Vec<_>>(),
    )
}

fn dedup_paths(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut unique = Vec::new();
    for path in paths {
        if unique.iter().any(|existing| existing == path) {
            continue;
        }
        unique.push(path.clone());
    }
    unique
}

fn default_auto_memory_dir(
    config: &RuntimeConfig,
    remote_memory_dir: Option<&std::ffi::OsStr>,
) -> String {
    let memory_base = remote_memory_dir
        .map(PathBuf::from)
        .unwrap_or_else(claude_config_home_dir);
    let project_root = canonical_project_root(&config.original_cwd);
    let sanitized = sanitize_path_component(&project_root.to_string_lossy());
    with_trailing_separator(
        memory_base
            .join(AUTO_MEMORY_PROJECTS_DIRNAME)
            .join(sanitized)
            .join(AUTO_MEMORY_DIRNAME),
    )
}

fn claude_config_home_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR") {
        return PathBuf::from(dir);
    }
    home_dir_from_env()
        .map(|home| home.join(".claude"))
        .unwrap_or_else(|| PathBuf::from(".claude"))
}

fn canonical_project_root(cwd: &Path) -> PathBuf {
    find_canonical_git_root(cwd)
        .or_else(|| fs::canonicalize(cwd).ok())
        .unwrap_or_else(|| cwd.to_path_buf())
}

fn find_canonical_git_root(cwd: &Path) -> Option<PathBuf> {
    let _git_root = git_absolute_path(
        cwd,
        &["rev-parse", "--path-format=absolute", "--show-toplevel"],
    )?;
    let common_dir = git_absolute_path(
        cwd,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )
    .or_else(|| git_absolute_path(cwd, &["rev-parse", "--git-common-dir"]))?;
    let common_dir = fs::canonicalize(&common_dir).unwrap_or(common_dir);
    if common_dir
        .file_name()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value == ".git")
    {
        return common_dir.parent().map(Path::to_path_buf);
    }
    Some(common_dir)
}

fn git_absolute_path(cwd: &Path, args: &[&str]) -> Option<PathBuf> {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let value = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if value.is_empty() {
        return None;
    }

    Some(PathBuf::from(value))
}

pub(crate) fn sanitize_path_component(raw: &str) -> String {
    let sanitized = raw
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect::<String>();
    if sanitized.len() <= MAX_SANITIZED_LENGTH {
        return sanitized;
    }

    format!(
        "{}-{}",
        &sanitized[..MAX_SANITIZED_LENGTH],
        simple_hash(raw)
    )
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

fn with_trailing_separator(path: PathBuf) -> String {
    let mut rendered = path.to_string_lossy().into_owned();
    if !rendered.ends_with(MAIN_SEPARATOR) {
        rendered.push(MAIN_SEPARATOR);
    }
    rendered
}

fn build_memory_lines(
    config: &RuntimeConfig,
    display_name: &str,
    memory_dir: &str,
    extra_guideline: Option<String>,
    skip_index: bool,
    features: &MemoryPromptFeatures,
) -> Vec<String> {
    let mut how_to_save = if skip_index {
        vec![
            "## How to save memories".to_owned(),
            String::new(),
            "Write each memory to its own file (e.g., `user_role.md`, `feedback_testing.md`) using this frontmatter format:".to_owned(),
            String::new(),
        ]
    } else {
        vec![
            "## How to save memories".to_owned(),
            String::new(),
            "Saving a memory is a two-step process:".to_owned(),
            String::new(),
            "**Step 1** — write the memory to its own file (e.g., `user_role.md`, `feedback_testing.md`) using this frontmatter format:".to_owned(),
            String::new(),
        ]
    };
    how_to_save.extend(
        MEMORY_FRONTMATTER_EXAMPLE
            .iter()
            .map(|line| (*line).to_owned()),
    );
    if skip_index {
        how_to_save.extend([
            String::new(),
            "- Keep the name, description, and type fields in memory files up-to-date with the content"
                .to_owned(),
            "- Organize memory semantically by topic, not chronologically".to_owned(),
            "- Update or remove memories that turn out to be wrong or outdated".to_owned(),
            "- Do not write duplicate memories. First check if there is an existing memory you can update before writing a new one.".to_owned(),
        ]);
    } else {
        how_to_save.extend([
            String::new(),
            format!(
                "**Step 2** — add a pointer to that file in `{ENTRYPOINT_NAME}`. `{ENTRYPOINT_NAME}` is an index, not a memory — each entry should be one line, under ~150 characters: `- [Title](file.md) — one-line hook`. It has no frontmatter. Never write memory content directly into `{ENTRYPOINT_NAME}`."
            ),
            String::new(),
            format!(
                "- `{ENTRYPOINT_NAME}` is always loaded into your conversation context — lines after {MAX_ENTRYPOINT_LINES} will be truncated, so keep the index concise"
            ),
            "- Keep the name, description, and type fields in memory files up-to-date with the content"
                .to_owned(),
            "- Organize memory semantically by topic, not chronologically".to_owned(),
            "- Update or remove memories that turn out to be wrong or outdated".to_owned(),
            "- Do not write duplicate memories. First check if there is an existing memory you can update before writing a new one.".to_owned(),
        ]);
    }

    let mut lines = vec![
        format!("# {display_name}"),
        String::new(),
        format!(
            "You have a persistent, file-based memory system at `{memory_dir}`. {DIR_EXISTS_GUIDANCE}"
        ),
        String::new(),
        "You should build up this memory system over time so that future conversations can have a complete picture of who the user is, how they'd like to collaborate with you, what behaviors to avoid or repeat, and the context behind the work the user gives you.".to_owned(),
        String::new(),
        "If the user explicitly asks you to remember something, save it immediately as whichever type fits best. If they ask you to forget something, find and remove the relevant entry.".to_owned(),
        String::new(),
    ];
    lines.extend(
        TYPES_SECTION_INDIVIDUAL
            .iter()
            .map(|line| (*line).to_owned()),
    );
    lines.extend(
        WHAT_NOT_TO_SAVE_SECTION
            .iter()
            .map(|line| (*line).to_owned()),
    );
    lines.push(String::new());
    lines.extend(how_to_save);
    lines.push(String::new());
    lines.extend(WHEN_TO_ACCESS_SECTION.iter().map(|line| (*line).to_owned()));
    lines.push(String::new());
    lines.extend(
        TRUSTING_RECALL_SECTION
            .iter()
            .map(|line| (*line).to_owned()),
    );
    lines.extend([
        String::new(),
        "## Memory and other forms of persistence".to_owned(),
        "Memory is one of several persistence mechanisms available to you as you assist the user in a given conversation. The distinction is often that memory can be recalled in future conversations and should not be used for persisting information that is only useful within the scope of the current conversation.".to_owned(),
        "- When to use or update a plan instead of memory: If you are about to start a non-trivial implementation task and would like to reach alignment with the user on your approach you should use a Plan rather than saving this information to memory. Similarly, if you already have a plan within the conversation and you have changed your approach persist that change by updating the plan rather than saving a memory.".to_owned(),
        "- When to use or update tasks instead of memory: When you need to break your work in current conversation into discrete steps or keep track of your progress use tasks instead of saving to memory. Tasks are great for persisting information about the work that needs to be done in the current conversation, but memory should be reserved for information that will be useful in future conversations.".to_owned(),
        String::new(),
    ]);
    if let Some(extra_guideline) = extra_guideline {
        lines.push(extra_guideline);
    }
    lines.push(String::new());
    lines.extend(build_searching_past_context_section(
        config, memory_dir, features,
    ));

    lines
}

fn build_combined_memory_prompt(
    config: &RuntimeConfig,
    auto_dir: &str,
    team_dir: &str,
    extra_guideline: Option<String>,
    skip_index: bool,
    features: &MemoryPromptFeatures,
) -> String {
    let mut how_to_save = if skip_index {
        vec![
            "## How to save memories".to_owned(),
            String::new(),
            "Write each memory to its own file in the chosen directory (private or team, per the type's scope guidance) using this frontmatter format:".to_owned(),
            String::new(),
        ]
    } else {
        vec![
            "## How to save memories".to_owned(),
            String::new(),
            "Saving a memory is a two-step process:".to_owned(),
            String::new(),
            "**Step 1** — write the memory to its own file in the chosen directory (private or team, per the type's scope guidance) using this frontmatter format:".to_owned(),
            String::new(),
        ]
    };
    how_to_save.extend(
        MEMORY_FRONTMATTER_EXAMPLE
            .iter()
            .map(|line| (*line).to_owned()),
    );
    if skip_index {
        how_to_save.extend([
            String::new(),
            "- Keep the name, description, and type fields in memory files up-to-date with the content"
                .to_owned(),
            "- Organize memory semantically by topic, not chronologically".to_owned(),
            "- Update or remove memories that turn out to be wrong or outdated".to_owned(),
            "- Do not write duplicate memories. First check if there is an existing memory you can update before writing a new one.".to_owned(),
        ]);
    } else {
        how_to_save.extend([
            String::new(),
            format!(
                "**Step 2** — add a pointer to that file in the same directory's `{ENTRYPOINT_NAME}`. Each directory (private and team) has its own `{ENTRYPOINT_NAME}` index — each entry should be one line, under ~150 characters: `- [Title](file.md) — one-line hook`. They have no frontmatter. Never write memory content directly into a `{ENTRYPOINT_NAME}`."
            ),
            String::new(),
            format!(
                "- Both `{ENTRYPOINT_NAME}` indexes are loaded into your conversation context — lines after {MAX_ENTRYPOINT_LINES} will be truncated, so keep them concise"
            ),
            "- Keep the name, description, and type fields in memory files up-to-date with the content"
                .to_owned(),
            "- Organize memory semantically by topic, not chronologically".to_owned(),
            "- Update or remove memories that turn out to be wrong or outdated".to_owned(),
            "- Do not write duplicate memories. First check if there is an existing memory you can update before writing a new one.".to_owned(),
        ]);
    }

    let mut lines = vec![
        "# Memory".to_owned(),
        String::new(),
        format!(
            "You have a persistent, file-based memory system with two directories: a private directory at `{auto_dir}` and a shared team directory at `{team_dir}`. Both directories already exist — write to them directly with the Write tool (do not run mkdir or check for their existence)."
        ),
        String::new(),
        "You should build up this memory system over time so that future conversations can have a complete picture of who the user is, how they'd like to collaborate with you, what behaviors to avoid or repeat, and the context behind the work the user gives you.".to_owned(),
        String::new(),
        "If the user explicitly asks you to remember something, save it immediately as whichever type fits best. If they ask you to forget something, find and remove the relevant entry.".to_owned(),
        String::new(),
        "## Memory scope".to_owned(),
        String::new(),
        "There are two scope levels:".to_owned(),
        String::new(),
        format!("- private: memories that are private between you and the current user. They persist across conversations with only this specific user and are stored at the root `{auto_dir}`."),
        format!("- team: memories that are shared with and contributed by all of the users who work within this project directory. Team memories are synced at the beginning of every session and they are stored at `{team_dir}`."),
        String::new(),
    ];
    lines.extend(TYPES_SECTION_COMBINED.iter().map(|line| (*line).to_owned()));
    lines.extend(
        WHAT_NOT_TO_SAVE_SECTION
            .iter()
            .map(|line| (*line).to_owned()),
    );
    lines.extend([
        "- You MUST avoid saving sensitive data within shared team memories. For example, never save API keys or user credentials.".to_owned(),
        String::new(),
    ]);
    lines.extend(how_to_save);
    lines.push(String::new());
    lines.extend(
        WHEN_TO_ACCESS_COMBINED_SECTION
            .iter()
            .map(|line| (*line).to_owned()),
    );
    lines.push(String::new());
    lines.extend(
        TRUSTING_RECALL_SECTION
            .iter()
            .map(|line| (*line).to_owned()),
    );
    lines.extend([
        String::new(),
        "## Memory and other forms of persistence".to_owned(),
        "Memory is one of several persistence mechanisms available to you as you assist the user in a given conversation. The distinction is often that memory can be recalled in future conversations and should not be used for persisting information that is only useful within the scope of the current conversation.".to_owned(),
        "- When to use or update a plan instead of memory: If you are about to start a non-trivial implementation task and would like to reach alignment with the user on your approach you should use a Plan rather than saving this information to memory. Similarly, if you already have a plan within the conversation and you have changed your approach persist that change by updating the plan rather than saving a memory.".to_owned(),
        "- When to use or update tasks instead of memory: When you need to break your work in current conversation into discrete steps or keep track of your progress use tasks instead of saving to memory. Tasks are great for persisting information about the work that needs to be done in the current conversation, but memory should be reserved for information that will be useful in future conversations.".to_owned(),
    ]);
    if let Some(extra_guideline) = extra_guideline {
        lines.push(extra_guideline);
    }
    lines.push(String::new());
    lines.extend(build_searching_past_context_section(
        config, auto_dir, features,
    ));

    lines.join("\n")
}

fn build_assistant_daily_log_prompt(
    config: &RuntimeConfig,
    memory_dir: &str,
    skip_index: bool,
    features: &MemoryPromptFeatures,
) -> String {
    let log_path_pattern = PathBuf::from(memory_dir)
        .join("logs")
        .join("YYYY")
        .join("MM")
        .join("YYYY-MM-DD.md")
        .to_string_lossy()
        .into_owned();
    let mut lines = vec![
        "# auto memory".to_owned(),
        String::new(),
        format!("You have a persistent, file-based memory system found at: `{memory_dir}`"),
        String::new(),
        "This session is long-lived. As you work, record anything worth remembering by **appending** to today's daily log file:".to_owned(),
        String::new(),
        format!("`{log_path_pattern}`"),
        String::new(),
        "Substitute today's date (from `currentDate` in your context) for `YYYY-MM-DD`. When the date rolls over mid-session, start appending to the new day's file.".to_owned(),
        String::new(),
        "Write each entry as a short timestamped bullet. Create the file (and parent directories) on first write if it does not exist. Do not rewrite or reorganize the log — it is append-only. A separate nightly process distills these logs into `MEMORY.md` and topic files.".to_owned(),
        String::new(),
        "## What to log".to_owned(),
        "- User corrections and preferences (\"use bun, not npm\"; \"stop summarizing diffs\")".to_owned(),
        "- Facts about the user, their role, or their goals".to_owned(),
        "- Project context that is not derivable from the code (deadlines, incidents, decisions and their rationale)".to_owned(),
        "- Pointers to external systems (dashboards, Linear projects, Slack channels)".to_owned(),
        "- Anything the user explicitly asks you to remember".to_owned(),
        String::new(),
    ];
    lines.extend(
        WHAT_NOT_TO_SAVE_SECTION
            .iter()
            .map(|line| (*line).to_owned()),
    );
    lines.push(String::new());
    if !skip_index {
        lines.extend([
            format!("## {ENTRYPOINT_NAME}"),
            format!(
                "`{ENTRYPOINT_NAME}` is the distilled index (maintained nightly from your logs) and is loaded into your context automatically. Read it for orientation, but do not edit it directly — record new information in today's log instead."
            ),
            String::new(),
        ]);
    }
    lines.extend(build_searching_past_context_section(
        config, memory_dir, features,
    ));
    lines.join("\n")
}

fn build_searching_past_context_section(
    config: &RuntimeConfig,
    auto_memory_dir: &str,
    features: &MemoryPromptFeatures,
) -> Vec<String> {
    if !features.searching_past_context_enabled {
        return Vec::new();
    }

    let transcripts_dir = config.paths.sessions_dir.to_string_lossy().into_owned();
    let use_shell_grep = features.embedded_search_tools || features.repl_mode_active;
    let memory_search = if use_shell_grep {
        format!("grep -rn \"<search term>\" {auto_memory_dir} --include=\"*.md\"")
    } else {
        format!("Grep with pattern=\"<search term>\" path=\"{auto_memory_dir}\" glob=\"*.md\"")
    };
    let transcript_search = if use_shell_grep {
        format!("grep -rn \"<search term>\" {transcripts_dir} --include=\"*.jsonl\"")
    } else {
        format!("Grep with pattern=\"<search term>\" path=\"{transcripts_dir}\" glob=\"*.jsonl\"")
    };

    vec![
        "## Searching past context".to_owned(),
        String::new(),
        "When looking for past context:".to_owned(),
        "1. Search topic files in your memory directory:".to_owned(),
        "```".to_owned(),
        memory_search,
        "```".to_owned(),
        "2. Session transcript logs (last resort — large files, slow):".to_owned(),
        "```".to_owned(),
        transcript_search,
        "```".to_owned(),
        "Use narrow search terms (error messages, file paths, function names) rather than broad keywords.".to_owned(),
        String::new(),
    ]
}

#[cfg(test)]
mod tests {
    use super::{
        AutoMemoryInputs, MemoryPromptFeatures, agent_memory_dirs, build_memory_lines,
        is_auto_memory_path, load_cowork_memory_mechanics_prompt_with,
        load_default_memory_prompt_with, resolve_auto_memory_enabled, validate_memory_path,
    };
    use claude_config::settings_layers::RuntimeOverrides;
    use claude_config::{ProviderOverrides, RuntimeConfig, load_runtime_config};
    use claude_core::{InputFormat, OutputFormat, PermissionMode};
    use std::ffi::OsString;
    use std::fs;
    use std::path::MAIN_SEPARATOR;
    use tempfile::tempdir;

    #[test]
    fn validate_memory_path_accepts_absolute_paths_and_enforces_single_trailing_separator() {
        let raw = if cfg!(windows) {
            r"C:\Users\Yanzh\memory\"
        } else {
            "/tmp/memory/"
        };

        let normalized = validate_memory_path(Some(raw), false).expect("normalized path");
        assert!(normalized.ends_with(MAIN_SEPARATOR));
        assert!(!normalized.ends_with(&format!("{MAIN_SEPARATOR}{MAIN_SEPARATOR}")));
    }

    #[test]
    fn validate_memory_path_rejects_relative_unc_and_drive_roots() {
        assert!(validate_memory_path(Some("../memory"), false).is_none());
        assert!(validate_memory_path(Some("//server/share"), false).is_none());
        if cfg!(windows) {
            assert!(validate_memory_path(Some(r"C:\"), false).is_none());
        }
    }

    #[test]
    fn build_memory_lines_preserves_source_sections() {
        let prompt = build_memory_lines(
            &test_runtime_config(tempdir().expect("tempdir").path(), None),
            "auto memory",
            "/tmp/auto-memory/",
            Some("Custom extra guideline".to_owned()),
            false,
            &MemoryPromptFeatures::default(),
        )
        .join("\n");

        assert!(prompt.contains("# auto memory"));
        assert!(prompt.contains("## How to save memories"));
        assert!(prompt.contains("## Before recommending from memory"));
        assert!(prompt.contains("type: {{user, feedback, project, reference}}"));
        assert!(prompt.contains("Custom extra guideline"));
    }

    #[test]
    fn extract_auto_only_prompt_matches_research_opener_and_index_rules() {
        let prompt = super::build_extract_auto_only_prompt(
            7,
            "- [user] user_role.md (2026-04-22T01:02:03.004Z): role",
            false,
        );

        assert!(prompt.starts_with(
            "You are now acting as the memory extraction subagent. Analyze the most recent ~7 messages above and use them to update your persistent memory systems."
        ));
        assert!(prompt.contains(
            "Available tools: Read, Grep, Glob, read-only Bash (ls/find/cat/stat/wc/head/tail and similar), and Edit/Write for paths inside the memory directory only. Bash rm is not permitted. All other tools — MCP, Agent, write-capable Bash, etc — will be denied."
        ));
        assert!(prompt.contains(
            "## Existing memory files\n\n- [user] user_role.md (2026-04-22T01:02:03.004Z): role\n\nCheck this list before writing — update an existing file rather than creating a duplicate."
        ));
        assert!(prompt.contains(
            "**Step 2** — add a pointer to that file in `MEMORY.md`. `MEMORY.md` is an index, not a memory — each entry should be one line, under ~150 characters: `- [Title](file.md) — one-line hook`. It has no frontmatter. Never write memory content directly into `MEMORY.md`."
        ));
        assert!(prompt.contains(
            "- `MEMORY.md` is always loaded into your system prompt — lines after 200 will be truncated, so keep the index concise"
        ));
        assert!(!prompt.contains(
            "Keep the name, description, and type fields in memory files up-to-date with the content"
        ));
    }

    #[test]
    fn extract_skip_index_prompt_omits_memory_index_step() {
        let prompt = super::build_extract_auto_only_prompt(3, "", true);

        assert!(prompt.contains(
            "Write each memory to its own file (e.g., `user_role.md`, `feedback_testing.md`) using this frontmatter format:"
        ));
        assert!(!prompt.contains("Saving a memory is a two-step process:"));
        assert!(!prompt.contains("**Step 2**"));
        assert!(!prompt.contains("## Existing memory files"));
    }

    #[test]
    fn extract_combined_prompt_matches_team_specific_rules() {
        let prompt = super::build_extract_combined_prompt(5, "", false);

        assert!(prompt.contains(
            "Each type below declares a <scope> of `private`, `team`, or guidance for choosing between the two."
        ));
        assert!(prompt.contains(
            "- You MUST avoid saving sensitive data within shared team memories. For example, never save API keys or user credentials."
        ));
        assert!(prompt.contains(
            "**Step 1** — write the memory to its own file in the chosen directory (private or team, per the type's scope guidance) using this frontmatter format:"
        ));
        assert!(prompt.contains(
            "**Step 2** — add a pointer to that file in the same directory's `MEMORY.md`. Each directory (private and team) has its own `MEMORY.md` index — each entry should be one line, under ~150 characters: `- [Title](file.md) — one-line hook`. They have no frontmatter. Never write memory content directly into a `MEMORY.md`."
        ));
    }

    #[test]
    fn memory_manifest_formats_headers_like_research() {
        let headers = vec![
            super::MemoryHeader {
                filename: "nested/project.md".to_owned(),
                file_path: std::path::PathBuf::from("nested/project.md"),
                mtime_ms: 1_776_819_723_004.0,
                description: Some("project facts".to_owned()),
                memory_type: Some(super::MemoryType::Project),
            },
            super::MemoryHeader {
                filename: "legacy.md".to_owned(),
                file_path: std::path::PathBuf::from("legacy.md"),
                mtime_ms: 1_776_819_724_000.0,
                description: None,
                memory_type: None,
            },
        ];

        assert_eq!(
            super::format_memory_manifest(&headers),
            "- [project] nested/project.md (2026-04-22T01:02:03.004Z): project facts\n- legacy.md (2026-04-22T01:02:04.000Z)"
        );
    }

    #[test]
    fn load_cowork_memory_mechanics_prompt_includes_source_backed_sections() {
        let tempdir = tempdir().expect("tempdir");
        let config = test_runtime_config(tempdir.path(), None);
        let memory_dir = tempdir.path().join("cowork-memory");
        let prompt = load_cowork_memory_mechanics_prompt_with(
            &config,
            &AutoMemoryInputs {
                auto_memory_enabled: true,
                cowork_memory_path_override: Some(memory_dir.display().to_string()),
                cowork_memory_extra_guidelines: Some("Cowork extra guideline".to_owned()),
                remote_memory_dir: None,
                team_memory_enabled: false,
            },
        )
        .expect("prompt")
        .expect("memory prompt");

        assert!(prompt.contains("# auto memory"));
        assert!(prompt.contains("## How to save memories"));
        assert!(prompt.contains("## Before recommending from memory"));
        assert!(prompt.contains("Cowork extra guideline"));
        assert!(prompt.contains(&format!("{}{}", memory_dir.display(), MAIN_SEPARATOR)));
        assert!(memory_dir.is_dir());
    }

    #[test]
    fn load_cowork_memory_mechanics_prompt_respects_disabled_state() {
        let tempdir = tempdir().expect("tempdir");
        let config = test_runtime_config(tempdir.path(), None);
        let prompt = load_cowork_memory_mechanics_prompt_with(
            &config,
            &AutoMemoryInputs {
                auto_memory_enabled: false,
                cowork_memory_path_override: Some(
                    tempdir.path().join("cowork-memory").display().to_string(),
                ),
                cowork_memory_extra_guidelines: None,
                remote_memory_dir: None,
                team_memory_enabled: false,
            },
        )
        .expect("prompt");

        assert!(prompt.is_none());
    }

    #[test]
    fn resolve_auto_memory_enabled_respects_settings_opt_out() {
        let tempdir = tempdir().expect("tempdir");
        let settings_dir = tempdir.path().join("workspace").join(".remote-code");
        fs::create_dir_all(&settings_dir).expect("settings dir");
        let settings_path = settings_dir.join("settings.json");
        fs::write(&settings_path, r#"{"autoMemoryEnabled": false}"#).expect("settings");
        let config = test_runtime_config(tempdir.path(), Some(settings_path));

        let enabled = resolve_auto_memory_enabled(&config, None, None, None, None::<OsString>)
            .expect("enabled");

        assert!(!enabled);
    }

    #[test]
    fn resolve_auto_memory_enabled_env_priority_matches_source_truth() {
        let tempdir = tempdir().expect("tempdir");
        let config = test_runtime_config(tempdir.path(), None);

        let enabled = resolve_auto_memory_enabled(
            &config,
            Some("0".to_owned()),
            Some("1".to_owned()),
            Some("1".to_owned()),
            None::<OsString>,
        )
        .expect("enabled");
        assert!(enabled);

        let disabled = resolve_auto_memory_enabled(
            &config,
            Some("true".to_owned()),
            None,
            None,
            None::<OsString>,
        )
        .expect("disabled");
        assert!(!disabled);

        let remote_disabled = resolve_auto_memory_enabled(
            &config,
            None,
            None,
            Some("1".to_owned()),
            None::<OsString>,
        )
        .expect("remote disabled");
        assert!(!remote_disabled);
    }

    #[test]
    fn load_default_memory_prompt_uses_default_projects_directory_shape() {
        let tempdir = tempdir().expect("tempdir");
        let config = test_runtime_config(tempdir.path(), None);

        let prompt = load_default_memory_prompt_with(
            &config,
            &AutoMemoryInputs {
                auto_memory_enabled: true,
                cowork_memory_path_override: None,
                cowork_memory_extra_guidelines: Some("Cowork extra guideline".to_owned()),
                remote_memory_dir: None,
                team_memory_enabled: false,
            },
            &MemoryPromptFeatures::default(),
        )
        .expect("prompt")
        .expect("default memory prompt");

        assert!(prompt.contains("# auto memory"));
        assert!(prompt.contains("Cowork extra guideline"));
        assert!(prompt.contains("projects"));
        assert!(prompt.contains("memory"));
    }

    #[test]
    fn load_default_memory_prompt_uses_trusted_auto_memory_directory_setting() {
        let tempdir = tempdir().expect("tempdir");
        let trusted_dir = tempdir.path().join("trusted-auto-memory");
        let settings_path = tempdir.path().join("trusted-settings.json");
        fs::write(
            &settings_path,
            format!(
                r#"{{"autoMemoryDirectory":"{}"}}"#,
                trusted_dir.display().to_string().replace('\\', "\\\\")
            ),
        )
        .expect("settings");
        let config = test_runtime_config(tempdir.path(), Some(settings_path));

        let prompt = load_default_memory_prompt_with(
            &config,
            &AutoMemoryInputs {
                auto_memory_enabled: true,
                cowork_memory_path_override: None,
                cowork_memory_extra_guidelines: None,
                remote_memory_dir: None,
                team_memory_enabled: false,
            },
            &MemoryPromptFeatures::default(),
        )
        .expect("prompt")
        .expect("default memory prompt");

        assert!(prompt.contains(&format!("{}{}", trusted_dir.display(), MAIN_SEPARATOR)));
        assert!(trusted_dir.is_dir());
    }

    #[test]
    fn memory_detection_rejects_prefix_sibling_paths() {
        let temp = tempfile::tempdir().expect("tempdir");
        let cwd = temp.path().join("workspace");
        let settings_dir = temp.path().join("settings");
        fs::create_dir_all(&cwd).expect("cwd");
        fs::create_dir_all(&settings_dir).expect("settings");
        let settings_file = settings_dir.join("settings.json");
        let memory_dir = temp.path().join("memory");
        fs::write(
            &settings_file,
            serde_json::json!({
                "autoMemoryDirectory": memory_dir,
            })
            .to_string(),
        )
        .expect("settings");
        let config = test_runtime_config(temp.path(), Some(settings_file));

        assert!(is_auto_memory_path(&config, &memory_dir.join("topic.md")));
        assert!(!is_auto_memory_path(
            &config,
            &temp.path().join("memoryevil").join("topic.md")
        ));
    }

    #[test]
    fn agent_memory_dirs_use_original_cwd_and_remote_local_root() {
        let temp = tempfile::tempdir().expect("tempdir");
        let original = temp.path().join("workspace");
        let active = temp.path().join("workspace-active");
        fs::create_dir_all(&original).expect("original");
        fs::create_dir_all(&active).expect("active");
        let mut config = test_runtime_config(temp.path(), None);
        config.original_cwd = original.clone();
        config.cwd = active.clone();

        let dirs = agent_memory_dirs(&config);
        assert!(dirs.contains(&original.join(".claude").join("agent-memory")));
        assert!(dirs.contains(&original.join(".claude").join("agent-memory-local")));
        assert!(!dirs.contains(&active.join(".claude").join("agent-memory")));
    }

    #[test]
    fn load_default_memory_prompt_ignores_project_auto_memory_directory_override() {
        let tempdir = tempdir().expect("tempdir");
        let workspace_settings = tempdir
            .path()
            .join("workspace")
            .join(".remote-code")
            .join("settings.json");
        let project_override = tempdir.path().join("project-override-memory");
        fs::create_dir_all(
            workspace_settings
                .parent()
                .expect("workspace settings parent should exist"),
        )
        .expect("workspace settings dir");
        fs::write(
            &workspace_settings,
            format!(
                r#"{{"autoMemoryDirectory":"{}"}}"#,
                project_override.display().to_string().replace('\\', "\\\\")
            ),
        )
        .expect("settings");
        let config = test_runtime_config(tempdir.path(), None);

        let prompt = load_default_memory_prompt_with(
            &config,
            &AutoMemoryInputs {
                auto_memory_enabled: true,
                cowork_memory_path_override: None,
                cowork_memory_extra_guidelines: None,
                remote_memory_dir: None,
                team_memory_enabled: false,
            },
            &MemoryPromptFeatures::default(),
        )
        .expect("prompt")
        .expect("default memory prompt");

        assert!(!prompt.contains(&project_override.display().to_string()));
        assert!(prompt.contains("projects"));
    }

    fn test_runtime_config(
        base: &std::path::Path,
        explicit_settings: Option<std::path::PathBuf>,
    ) -> RuntimeConfig {
        let cwd = base.join("workspace");
        let profile = base.join(".remote-code-rust");
        fs::create_dir_all(&cwd).expect("cwd");
        fs::create_dir_all(&profile).expect("profile");

        load_runtime_config(
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
            RuntimeOverrides {
                settings_files: explicit_settings.into_iter().collect(),
                ..RuntimeOverrides::default()
            },
        )
        .expect("runtime config")
    }
}
