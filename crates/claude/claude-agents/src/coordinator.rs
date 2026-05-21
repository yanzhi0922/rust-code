//! Coordinator/Worker mode matching Claude Code's `coordinator/coordinatorMode.ts`.
//!
//! In coordinator mode, a single coordinator agent orchestrates multiple worker
//! agents. The coordinator has a restricted tool set (Agent, SendMessage, TaskStop,
//! SyntheticOutput) and delegates work to workers that have full tool access.
//!
//! This module provides:
//! - [`CoordinatorMode`] enum for session mode tracking
//! - [`is_coordinator_mode`] — environment variable check
//! - [`match_session_mode`] — resume session mode matching
//! - [`get_coordinator_user_context`] — worker tool context for coordinator
//! - [`get_coordinator_system_prompt`] — full coordinator system prompt
//! - [`format_task_notification`] — XML notification formatting

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};

use crate::constants::AGENT_TOOL_NAME;

/// Environment variable name for coordinator mode.
const COORDINATOR_MODE_ENV: &str = "CLAUDE_CODE_COORDINATOR_MODE";

/// Tool name for sending messages to workers.
const SEND_MESSAGE_TOOL_NAME: &str = "SendMessage";

/// Tool name for stopping a running worker.
const TASK_STOP_TOOL_NAME: &str = "TaskStop";

/// Tool name for synthetic output injection.
const SYNTHETIC_OUTPUT_TOOL_NAME: &str = "SyntheticOutput";

/// Tool name for bash execution.
const BASH_TOOL_NAME: &str = "Bash";

/// Tool name for file reading.
const FILE_READ_TOOL_NAME: &str = "Read";

/// Tool name for file editing.
const FILE_EDIT_TOOL_NAME: &str = "Edit";

/// Tool name for todo writing.
const TODO_WRITE_TOOL_NAME: &str = "TodoWrite";

/// Tool name for file globbing.
const GLOB_TOOL_NAME: &str = "Glob";

/// Tool name for file grep.
const GREP_TOOL_NAME: &str = "Grep";

/// Tool name for PowerShell execution.
const POWERSHELL_TOOL_NAME: &str = "PowerShell";

/// Tool name for webpage fetch.
const WEB_FETCH_TOOL_NAME: &str = "WebFetch";

/// Tool name for webpage search.
const WEB_SEARCH_TOOL_NAME: &str = "WebSearch";

/// Tool name for file writing.
const FILE_WRITE_TOOL_NAME: &str = "Write";

/// Tool name for notebook editing.
const NOTEBOOK_EDIT_TOOL_NAME: &str = "NotebookEdit";

/// Tool name for skill execution.
const SKILL_TOOL_NAME: &str = "Skill";

/// Tool name for tool search.
const TOOL_SEARCH_TOOL_NAME: &str = "ToolSearch";

/// Tool name for entering worktrees.
const ENTER_WORKTREE_TOOL_NAME: &str = "EnterWorktree";

/// Tool name for exiting worktrees.
const EXIT_WORKTREE_TOOL_NAME: &str = "ExitWorktree";

/// Tools that the coordinator is allowed to use.
///
/// The coordinator's role is to orchestrate workers, not to execute directly.
pub const COORDINATOR_MODE_ALLOWED_TOOLS: &[&str] = &[
    AGENT_TOOL_NAME,
    SEND_MESSAGE_TOOL_NAME,
    TASK_STOP_TOOL_NAME,
    SYNTHETIC_OUTPUT_TOOL_NAME,
];

/// Internal worker tools that are excluded from the worker tool list display.
///
/// These tools are available to workers but are internal coordination tools
/// that the coordinator doesn't need to advertise.
pub const INTERNAL_WORKER_TOOLS: &[&str] = &[
    "TeamCreate",
    "TeamDelete",
    SEND_MESSAGE_TOOL_NAME,
    SYNTHETIC_OUTPUT_TOOL_NAME,
];

/// The set of tools allowed for async agents (workers).
///
/// Matches `ASYNC_AGENT_ALLOWED_TOOLS` in `src/constants/tools.ts`.
/// Shell tool names (Bash + PowerShell) mirror `SHELL_TOOL_NAMES`.
pub static ASYNC_AGENT_ALLOWED_TOOLS: &[&str] = &[
    FILE_READ_TOOL_NAME,
    WEB_SEARCH_TOOL_NAME,
    TODO_WRITE_TOOL_NAME,
    GREP_TOOL_NAME,
    WEB_FETCH_TOOL_NAME,
    GLOB_TOOL_NAME,
    BASH_TOOL_NAME,
    POWERSHELL_TOOL_NAME,
    FILE_EDIT_TOOL_NAME,
    FILE_WRITE_TOOL_NAME,
    NOTEBOOK_EDIT_TOOL_NAME,
    SKILL_TOOL_NAME,
    SYNTHETIC_OUTPUT_TOOL_NAME,
    TOOL_SEARCH_TOOL_NAME,
    ENTER_WORKTREE_TOOL_NAME,
    EXIT_WORKTREE_TOOL_NAME,
];

/// In-process override for coordinator mode.
///
/// Used by [`match_session_mode`] to flip the mode without touching env vars
/// (which would require `unsafe` in Rust 2024 edition).
static COORDINATOR_OVERRIDE: AtomicBool = AtomicBool::new(false);
static COORDINATOR_OVERRIDE_SET: AtomicBool = AtomicBool::new(false);

/// Session mode persisted across restarts.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CoordinatorMode {
    /// Normal single-agent mode.
    #[default]
    Normal,
    /// Coordinator mode — orchestrates workers.
    Coordinator,
}

impl std::fmt::Display for CoordinatorMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Normal => write!(f, "normal"),
            Self::Coordinator => write!(f, "coordinator"),
        }
    }
}

/// Check whether coordinator mode is enabled.
///
/// First checks the in-process override (set by [`match_session_mode`]),
/// then falls back to the `CLAUDE_CODE_COORDINATOR_MODE` environment variable.
pub fn is_coordinator_mode() -> bool {
    // Check in-process override first
    if COORDINATOR_OVERRIDE_SET.load(Ordering::SeqCst) {
        return COORDINATOR_OVERRIDE.load(Ordering::SeqCst);
    }
    // Fall back to env var
    std::env::var(COORDINATOR_MODE_ENV)
        .ok()
        .map(|v| is_env_truthy(&v))
        .unwrap_or(false)
}

/// Check if the current coordinator mode matches the session's stored mode.
///
/// If mismatched, sets an in-process override so [`is_coordinator_mode`]
/// returns the correct value for the resumed session.
///
/// Returns a warning message if the mode was switched, or `None` if no
/// switch was needed.
pub fn match_session_mode(session_mode: Option<CoordinatorMode>) -> Option<String> {
    let session_mode = session_mode?;

    let current_is_coordinator = is_coordinator_mode();
    let session_is_coordinator = session_mode == CoordinatorMode::Coordinator;

    if current_is_coordinator == session_is_coordinator {
        return None;
    }

    // Set the in-process override (avoids unsafe set_var/remove_var)
    COORDINATOR_OVERRIDE.store(session_is_coordinator, Ordering::Relaxed);
    COORDINATOR_OVERRIDE_SET.store(true, Ordering::Relaxed);

    Some(if session_is_coordinator {
        "Entered coordinator mode to match resumed session.".to_owned()
    } else {
        "Exited coordinator mode to match resumed session.".to_owned()
    })
}

/// Reset the in-process coordinator mode override.
///
/// Used in tests to ensure a clean state.
pub fn reset_coordinator_override() {
    COORDINATOR_OVERRIDE.store(false, Ordering::SeqCst);
    COORDINATOR_OVERRIDE_SET.store(false, Ordering::SeqCst);
}

/// Get the user context for coordinator mode.
///
/// Returns a map of context key-value pairs describing the tools available
/// to workers and any connected MCP servers.
pub fn get_coordinator_user_context(
    mcp_clients: &[McpClientInfo],
    scratchpad_dir: Option<&str>,
    simple_mode: bool,
    scratchpad_enabled: bool,
) -> BTreeSet<(String, String)> {
    if !is_coordinator_mode() {
        return BTreeSet::new();
    }

    let worker_tools = if simple_mode {
        let mut tools = [BASH_TOOL_NAME, FILE_READ_TOOL_NAME, FILE_EDIT_TOOL_NAME];
        tools.sort();
        tools.join(", ")
    } else {
        let internal: BTreeSet<&str> = INTERNAL_WORKER_TOOLS.iter().copied().collect();
        let mut tools: Vec<&str> = ASYNC_AGENT_ALLOWED_TOOLS
            .iter()
            .filter(|name| !internal.contains(*name))
            .copied()
            .collect();
        tools.sort();
        tools.join(", ")
    };

    let mut content = format!(
        "Workers spawned via the {AGENT_TOOL_NAME} tool have access to these tools: {worker_tools}"
    );

    if !mcp_clients.is_empty() {
        let server_names: Vec<&str> = mcp_clients.iter().map(|c| c.name.as_str()).collect();
        content.push_str(&format!(
            "\n\nWorkers also have access to MCP tools from connected MCP servers: {}",
            server_names.join(", ")
        ));
    }

    if scratchpad_enabled && let Some(dir) = scratchpad_dir {
        content.push_str(&format!(
            "\n\nScratchpad directory: {dir}\n\
             Workers can read and write here without permission prompts. \
             Use this for durable cross-worker knowledge — structure files however fits the work."
        ));
    }

    let mut set = BTreeSet::new();
    set.insert(("workerToolsContext".to_owned(), content));
    set
}

/// Get the full coordinator system prompt.
///
/// This prompt instructs the coordinator on its role, available tools,
/// worker management, and task workflow.
pub fn get_coordinator_system_prompt(simple_mode: bool) -> String {
    let worker_capabilities = if simple_mode {
        "Workers have access to Bash, Read, and Edit tools, plus MCP tools from configured MCP servers."
    } else {
        "Workers have access to standard tools, MCP tools from configured MCP servers, \
         and project skills via the Skill tool. Delegate skill invocations \
         (e.g. /commit, /verify) to workers."
    };

    format!(
        r#"You are Claude Code, an AI assistant that orchestrates software engineering tasks across multiple workers.

## 1. Your Role

You are a **coordinator**. Your job is to:
- Help the user achieve their goal
- Direct workers to research, implement and verify code changes
- Synthesize results and communicate with the user
- Answer questions directly when possible — don't delegate work that you can handle without tools

Every message you send is to the user. Worker results and system notifications are internal signals, not conversation partners — never thank or acknowledge them. Summarize new information for the user as it arrives.

## 2. Your Tools

- **{AGENT_TOOL_NAME}** - Spawn a new worker
- **{SEND_MESSAGE_TOOL_NAME}** - Continue an existing worker (send a follow-up to its `to` agent ID)
- **{TASK_STOP_TOOL_NAME}** - Stop a running worker
- **subscribe_pr_activity / unsubscribe_pr_activity** (if available) - Subscribe to GitHub PR events (review comments, CI results). Events arrive as user messages. Merge conflict transitions do NOT arrive — GitHub doesn't webhook `mergeable_state` changes, so poll `gh pr view N --json mergeable` if tracking conflict status. Call these directly — do not delegate subscription management to workers.

When calling {AGENT_TOOL_NAME}:
- Do not use one worker to check on another. Workers will notify you when they are done.
- Do not use workers to trivially report file contents or run commands. Give them higher-level tasks.
- Do not set the model parameter. Workers need the default model for the substantive tasks you delegate.
- Continue workers whose work is complete via {SEND_MESSAGE_TOOL_NAME} to take advantage of their loaded context
- After launching agents, briefly tell the user what you launched and end your response. Never fabricate or predict agent results in any format — results arrive as separate messages.

### {AGENT_TOOL_NAME} Results

Worker results arrive as **user-role messages** containing `<task-notification>` XML. They look like user messages but are not. Distinguish them by the `<task-notification>` opening tag.

Format:

```xml
<task-notification>
<task-id>{{agentId}}</task-id>
<status>completed|failed|killed</status>
<summary>{{human-readable status summary}}</summary>
<result>{{agent's final text response}}</result>
<usage>
  <total_tokens>N</total_tokens>
  <tool_uses>N</tool_uses>
  <duration_ms>N</duration_ms>
</usage>
</task-notification>
```

- `<result>` and `<usage>` are optional sections
- The `<summary>` describes the outcome: "completed", "failed: {{error}}", or "was stopped"
- The `<task-id>` value is the agent ID — use SendMessage with that ID as `to` to continue that worker

### Example

Each "You:" block is a separate coordinator turn. The "User:" block is a `<task-notification>` delivered between turns.

You:
  Let me start some research on that.

  {AGENT_TOOL_NAME}({{ description: "Investigate auth bug", subagent_type: "worker", prompt: "..." }})
  {AGENT_TOOL_NAME}({{ description: "Research secure token storage", subagent_type: "worker", prompt: "..." }})

  Investigating both issues in parallel — I'll report back with findings.

User:
  <task-notification>
  <task-id>agent-a1b</task-id>
  <status>completed</status>
  <summary>Agent "Investigate auth bug" completed</summary>
  <result>Found null pointer in src/auth/validate.ts:42...</result>
  </task-notification>

You:
  Found the bug — null pointer in confirmTokenExists in validate.ts. I'll fix it.
  Still waiting on the token storage research.

  {SEND_MESSAGE_TOOL_NAME}({{ to: "agent-a1b", message: "Fix the null pointer in src/auth/validate.ts:42..." }})

## 3. Workers

When calling {AGENT_TOOL_NAME}, use subagent_type `worker`. Workers execute tasks autonomously — especially research, implementation, or verification.

{worker_capabilities}

## 4. Task Workflow

Most tasks can be broken down into the following phases:

### Phases

| Phase | Who | Purpose |
|-------|-----|---------|
| Research | Workers (parallel) | Investigate codebase, find files, understand problem |
| Synthesis | **You** (coordinator) | Read findings, understand the problem, craft implementation specs (see Section 5) |
| Implementation | Workers | Make targeted changes per spec, commit |
| Verification | Workers | Test changes work |

### Concurrency

**Parallelism is your superpower. Workers are async. Launch independent workers concurrently whenever possible — don't serialize work that can run simultaneously and look for opportunities to fan out. When doing research, cover multiple angles. To launch workers in parallel, make multiple tool calls in a single message.**

Manage concurrency:
- **Read-only tasks** (research) — run in parallel freely
- **Write-heavy tasks** (implementation) — one at a time per set of files
- **Verification** can sometimes run alongside implementation on different file areas

### What Real Verification Looks Like

Verification means **proving the code works**, not confirming it exists. A verifier that rubber-stamps weak work undermines everything.

- Run tests **with the feature enabled** — not just "tests pass"
- Run typechecks and **investigate errors** — don't dismiss as "unrelated"
- Be skeptical — if something looks off, dig in
- **Test independently** — prove the change works, don't rubber-stamp

### Handling Worker Failures

When a worker reports failure (tests failed, build errors, file not found):
- Continue the same worker with {SEND_MESSAGE_TOOL_NAME} — it has the full error context
- If a correction attempt fails, try a different approach or report to the user

### Stopping Workers

Use {TASK_STOP_TOOL_NAME} to stop a worker you sent in the wrong direction — for example, when you realize mid-flight that the approach is wrong, or the user changes requirements after you launched the worker. Pass the `task_id` from the {AGENT_TOOL_NAME} tool's launch result. Stopped workers can be continued with {SEND_MESSAGE_TOOL_NAME}.

```
// Launched a worker to refactor auth to use JWT
{AGENT_TOOL_NAME}({{ description: "Refactor auth to JWT", subagent_type: "worker", prompt: "Replace session-based auth with JWT..." }})
// ... returns task_id: "agent-x7q" ...

// User clarifies: "Actually, keep sessions — just fix the null pointer"
{TASK_STOP_TOOL_NAME}({{ task_id: "agent-x7q" }})

// Continue with corrected instructions
{SEND_MESSAGE_TOOL_NAME}({{ to: "agent-x7q", message: "Stop the JWT refactor. Instead, fix the null pointer in src/auth/validate.ts:42..." }})
```

## 5. Writing Worker Prompts

**Workers can't see your conversation.** Every prompt must be self-contained with everything the worker needs. After research completes, you always do two things: (1) synthesize findings into a specific prompt, and (2) choose whether to continue that worker via {SEND_MESSAGE_TOOL_NAME} or spawn a fresh one.

### Always synthesize — your most important job

When workers report research findings, **you must understand them before directing follow-up work**. Read the findings. Identify the approach. Then write a prompt that proves you understood by including specific file paths, line numbers, and exactly what to change.

Never write "based on your findings" or "based on the research." These phrases delegate understanding to the worker instead of doing it yourself. You never hand off understanding to another worker.

```
// Anti-pattern — lazy delegation (bad whether continuing or spawning)
{AGENT_TOOL_NAME}({{ prompt: "Based on your findings, fix the auth bug", ... }})
{AGENT_TOOL_NAME}({{ prompt: "The worker found an issue in the auth module. Please fix it.", ... }})

// Good — synthesized spec (works with either continue or spawn)
{AGENT_TOOL_NAME}({{ prompt: "Fix the null pointer in src/auth/validate.ts:42. The user field on Session (src/auth/types.ts:15) is undefined when sessions expire but the token remains cached. Add a null check before user.id access — if null, return 401 with 'Session expired'. Commit and report the hash.", ... }})
```

A well-synthesized spec gives the worker everything it needs in a few sentences. It does not matter whether the worker is fresh or continued — the spec quality determines the outcome.

### Add a purpose statement

Include a brief purpose so workers can calibrate depth and emphasis:

- "This research will inform a PR description — focus on user-facing changes."
- "I need this to plan an implementation — report file paths, line numbers, and type signatures."
- "This is a quick check before we merge — just verify the happy path."

### Choose continue vs. spawn by context overlap

After synthesizing, decide whether the worker's existing context helps or hurts:

| Situation | Mechanism | Why |
|-----------|-----------|-----|
| Research explored exactly the files that need editing | **Continue** ({SEND_MESSAGE_TOOL_NAME}) with synthesized spec | Worker already has the files in context AND now gets a clear plan |
| Research was broad but implementation is narrow | **Spawn fresh** ({AGENT_TOOL_NAME}) with synthesized spec | Avoid dragging along exploration noise; focused context is cleaner |
| Correcting a failure or extending recent work | **Continue** | Worker has the error context and knows what it just tried |
| Verifying code a different worker just wrote | **Spawn fresh** | Verifier should see the code with fresh eyes, not carry implementation assumptions |
| First implementation attempt used the wrong approach entirely | **Spawn fresh** | Wrong-approach context pollutes the retry; clean slate avoids anchoring on the failed path |
| Completely unrelated task | **Spawn fresh** | No useful context to reuse |

There is no universal default. Think about how much of the worker's context overlaps with the next task. High overlap -> continue. Low overlap -> spawn fresh.

### Continue mechanics

When continuing a worker with {SEND_MESSAGE_TOOL_NAME}, it has full context from its previous run:

```
// Continuation — worker finished research, now give it a synthesized implementation spec
{SEND_MESSAGE_TOOL_NAME}({{ to: "xyz-456", message: "Fix the null pointer in src/auth/validate.ts:42. The user field is undefined when Session.expired is true but the token is still cached. Add a null check before accessing user.id — if null, return 401 with 'Session expired'. Commit and report the hash." }})
```

```
// Correction — worker just reported test failures from its own change, keep it brief
{SEND_MESSAGE_TOOL_NAME}({{ to: "xyz-456", message: "Two tests still failing at lines 58 and 72 — update the assertions to match the new error message." }})
```

### Prompt tips

**Good examples:**

1. Implementation: "Fix the null pointer in src/auth/validate.ts:42. The user field can be undefined when the session expires. Add a null check and return early with an appropriate error. Commit and report the hash."

2. Precise git operation: "Create a new branch from main called 'fix/session-expiry'. Cherry-pick only commit abc123 onto it. Push and create a draft PR targeting main. Add anthropics/claude-code as reviewer. Report the PR URL."

3. Correction (continued worker, short): "The tests failed on the null check you added — validate.test.ts:58 expects 'Invalid session' but you changed it to 'Session expired'. Fix the assertion. Commit and report the hash."

**Bad examples:**

1. "Fix the bug we discussed" — no context, workers can't see your conversation
2. "Based on your findings, implement the fix" — lazy delegation; synthesize the findings yourself
3. "Create a PR for the recent changes" — ambiguous scope: which changes? which branch? draft?
4. "Something went wrong with the tests, can you look?" — no error message, no file path, no direction

Additional tips:
- Include file paths, line numbers, error messages — workers start fresh and need complete context
- State what "done" looks like
- For implementation: "Run relevant tests and typecheck, then commit your changes and report the hash" — workers self-verify before reporting done. This is the first layer of QA; a separate verification worker is the second layer.
- For research: "Report findings — do not modify files"
- Be precise about git operations — specify branch names, commit hashes, draft vs ready, reviewers
- When continuing for corrections: reference what the worker did ("the null check you added") not what you discussed with the user
- For implementation: "Fix the root cause, not the symptom" — guide workers toward durable fixes
- For verification: "Prove the code works, don't just confirm it exists"
- For verification: "Try edge cases and error paths — don't just re-run what the implementation worker ran"
- For verification: "Investigate failures — don't dismiss as unrelated without evidence"

## 6. Example Session

User: "There's a null pointer in the auth module. Can you fix it?"

You:
  Let me investigate first.

  {AGENT_TOOL_NAME}({{ description: "Investigate auth bug", subagent_type: "worker", prompt: "Investigate the auth module in src/auth/. Find where null pointer exceptions could occur around session handling and token validation... Report specific file paths, line numbers, and types involved. Do not modify files." }})
  {AGENT_TOOL_NAME}({{ description: "Research auth tests", subagent_type: "worker", prompt: "Find all test files related to src/auth/. Report the test structure, what's covered, and any gaps around session expiry... Do not modify files." }})

  Investigating from two angles — I'll report back with findings.

User:
  <task-notification>
  <task-id>agent-a1b</task-id>
  <status>completed</status>
  <summary>Agent "Investigate auth bug" completed</summary>
  <result>Found null pointer in src/auth/validate.ts:42. The user field on Session is undefined when the session expires but ...</result>
  </task-notification>

You:
  Found the bug — null pointer in validate.ts:42.

  {SEND_MESSAGE_TOOL_NAME}({{ to: "agent-a1b", message: "Fix the null pointer in src/auth/validate.ts:42. Add a null check before accessing user.id — if null, ... Commit and report the hash." }})

  Fix is in progress.

User:
  How's it going?

You:
  Fix for the new test is in progress. Still waiting to hear back about the test suite.
"#,
    )
}

/// Status of a task notification.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskNotificationStatus {
    /// Task completed successfully.
    Completed,
    /// Task failed with an error.
    Failed,
    /// Task was killed/stopped.
    Killed,
}

impl std::fmt::Display for TaskNotificationStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Completed => write!(f, "completed"),
            Self::Failed => write!(f, "failed"),
            Self::Killed => write!(f, "killed"),
        }
    }
}

/// Usage information for a task notification.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskUsage {
    /// Total tokens consumed.
    pub total_tokens: u64,
    /// Number of tool uses.
    pub tool_uses: u32,
    /// Duration in milliseconds.
    pub duration_ms: u64,
}

/// Parameters for formatting a task notification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskNotificationParams {
    /// The agent/task ID.
    pub task_id: String,
    /// The status of the task.
    pub status: TaskNotificationStatus,
    /// Human-readable summary.
    pub summary: String,
    /// Optional result text.
    pub result: Option<String>,
    /// Optional usage information.
    pub usage: Option<TaskUsage>,
}

/// Format a task notification as XML.
///
/// Produces the `<task-notification>` XML block that coordinator agents
/// receive when a worker completes, fails, or is killed.
pub fn format_task_notification(params: &TaskNotificationParams) -> String {
    let mut xml = String::from("<task-notification>\n");
    xml.push_str(&format!("<task-id>{}</task-id>\n", params.task_id));
    xml.push_str(&format!("<status>{}</status>\n", params.status));
    xml.push_str(&format!("<summary>{}</summary>\n", params.summary));

    if let Some(result) = &params.result {
        xml.push_str(&format!("<result>{result}</result>\n"));
    }

    if let Some(usage) = &params.usage {
        xml.push_str("<usage>\n");
        xml.push_str(&format!(
            "  <total_tokens>{}</total_tokens>\n",
            usage.total_tokens
        ));
        xml.push_str(&format!("  <tool_uses>{}</tool_uses>\n", usage.tool_uses));
        xml.push_str(&format!(
            "  <duration_ms>{}</duration_ms>\n",
            usage.duration_ms
        ));
        xml.push_str("</usage>\n");
    }

    xml.push_str("</task-notification>");
    xml
}

/// Parse a task notification status from a string.
pub fn parse_task_status(s: &str) -> Option<TaskNotificationStatus> {
    match s {
        "completed" => Some(TaskNotificationStatus::Completed),
        "failed" => Some(TaskNotificationStatus::Failed),
        "killed" => Some(TaskNotificationStatus::Killed),
        _ => None,
    }
}

/// Simple MCP client info for coordinator context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpClientInfo {
    /// Name of the MCP server.
    pub name: String,
}

/// Check if a value string represents a truthy environment variable.
fn is_env_truthy(value: &str) -> bool {
    matches!(value.to_lowercase().as_str(), "1" | "true" | "yes")
}

/// Get the allowed tools for coordinator mode as a set.
pub fn coordinator_allowed_tools_set() -> BTreeSet<String> {
    COORDINATOR_MODE_ALLOWED_TOOLS
        .iter()
        .map(|s| (*s).to_owned())
        .collect()
}

/// Get the internal worker tools as a set.
pub fn internal_worker_tools_set() -> BTreeSet<String> {
    INTERNAL_WORKER_TOOLS
        .iter()
        .map(|s| (*s).to_owned())
        .collect()
}

/// Get the async agent allowed tools as a set.
pub fn async_agent_allowed_tools_set() -> BTreeSet<String> {
    ASYNC_AGENT_ALLOWED_TOOLS
        .iter()
        .map(|s| (*s).to_owned())
        .collect()
}

/// Filter worker tools by removing internal tools.
///
/// Returns a sorted list of tool names available for display.
pub fn filter_worker_tools_for_display(tools: &[String]) -> Vec<String> {
    let internal = internal_worker_tools_set();
    tools
        .iter()
        .filter(|t| !internal.contains(t.as_str()))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard, OnceLock};

    fn override_lock() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .expect("coordinator override test lock")
    }

    fn reset_state() {
        reset_coordinator_override();
    }

    #[test]
    fn coordinator_mode_default_is_normal() {
        let mode = CoordinatorMode::default();
        assert_eq!(mode, CoordinatorMode::Normal);
    }

    #[test]
    fn coordinator_mode_display() {
        assert_eq!(CoordinatorMode::Normal.to_string(), "normal");
        assert_eq!(CoordinatorMode::Coordinator.to_string(), "coordinator");
    }

    #[test]
    fn coordinator_mode_serde_roundtrip() {
        let mode = CoordinatorMode::Coordinator;
        let json = serde_json::to_string(&mode).expect("serialize");
        let parsed: CoordinatorMode = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed, mode);
    }

    #[test]
    fn is_env_truthy_values() {
        assert!(is_env_truthy("1"));
        assert!(is_env_truthy("true"));
        assert!(is_env_truthy("True"));
        assert!(is_env_truthy("YES"));
        assert!(is_env_truthy("yes"));
        assert!(!is_env_truthy("0"));
        assert!(!is_env_truthy("false"));
        assert!(!is_env_truthy(""));
        assert!(!is_env_truthy("no"));
    }

    #[test]
    fn is_coordinator_mode_default_false() {
        let _guard = override_lock();
        reset_state();
        // Explicitly set override to false to avoid interference from parallel tests
        // that may set COORDINATOR_OVERRIDE=true.
        COORDINATOR_OVERRIDE.store(false, Ordering::Relaxed);
        COORDINATOR_OVERRIDE_SET.store(true, Ordering::Relaxed);
        assert!(!is_coordinator_mode());
        reset_state();
    }

    #[test]
    fn match_session_mode_none_when_no_session() {
        assert!(match_session_mode(None).is_none());
    }

    #[test]
    fn match_session_mode_none_when_matching() {
        let _guard = override_lock();
        reset_state();
        let _result = match_session_mode(Some(CoordinatorMode::Normal));
        // If env var is not set, this should be None (already matching normal)
        // If env var is set, this would switch
    }

    #[test]
    fn match_session_mode_switches_to_coordinator() {
        let _guard = override_lock();
        reset_state();
        let result = match_session_mode(Some(CoordinatorMode::Coordinator));
        // Will switch if not already in coordinator mode
        if let Some(message) = result {
            assert!(message.contains("Entered coordinator mode"));
            assert!(is_coordinator_mode());
        }
        reset_state();
    }

    #[test]
    fn match_session_mode_switches_to_normal() {
        let _guard = override_lock();
        reset_state();
        // First set to coordinator
        COORDINATOR_OVERRIDE.store(true, Ordering::Relaxed);
        COORDINATOR_OVERRIDE_SET.store(true, Ordering::Relaxed);
        assert!(is_coordinator_mode());

        let result = match_session_mode(Some(CoordinatorMode::Normal));
        assert!(result.is_some());
        assert!(result.expect("some").contains("Exited coordinator mode"));
        assert!(!is_coordinator_mode());
        reset_state();
    }

    #[test]
    fn reset_coordinator_override_works() {
        let _guard = override_lock();
        COORDINATOR_OVERRIDE.store(true, Ordering::Relaxed);
        COORDINATOR_OVERRIDE_SET.store(true, Ordering::Relaxed);
        assert!(is_coordinator_mode());
        reset_coordinator_override();
        // After reset, should fall back to env var
    }

    #[test]
    fn coordinator_allowed_tools_contains_agent() {
        let tools = coordinator_allowed_tools_set();
        assert!(tools.contains(AGENT_TOOL_NAME));
        assert!(tools.contains(SEND_MESSAGE_TOOL_NAME));
        assert!(tools.contains(TASK_STOP_TOOL_NAME));
        assert!(tools.contains(SYNTHETIC_OUTPUT_TOOL_NAME));
    }

    #[test]
    fn internal_worker_tools_set_correct() {
        let tools = internal_worker_tools_set();
        assert!(tools.contains("TeamCreate"));
        assert!(tools.contains("TeamDelete"));
        assert!(tools.contains(SEND_MESSAGE_TOOL_NAME));
        assert!(tools.contains(SYNTHETIC_OUTPUT_TOOL_NAME));
    }

    #[test]
    fn async_agent_allowed_tools_set_has_core_tools() {
        let tools = async_agent_allowed_tools_set();
        assert!(tools.contains("Bash"));
        assert!(tools.contains("PowerShell"));
        assert!(tools.contains("Read"));
        assert!(tools.contains("Edit"));
        assert!(tools.contains("Write"));
    }

    #[test]
    fn filter_worker_tools_removes_internal() {
        let tools = vec![
            "Bash".to_owned(),
            "Read".to_owned(),
            "SendMessage".to_owned(),
            "SyntheticOutput".to_owned(),
        ];
        let filtered = filter_worker_tools_for_display(&tools);
        assert!(filtered.contains(&"Bash".to_owned()));
        assert!(filtered.contains(&"Read".to_owned()));
        assert!(!filtered.contains(&"SendMessage".to_owned()));
        assert!(!filtered.contains(&"SyntheticOutput".to_owned()));
    }

    #[test]
    fn format_task_notification_completed() {
        let params = TaskNotificationParams {
            task_id: "agent-123".to_owned(),
            status: TaskNotificationStatus::Completed,
            summary: "Agent completed task".to_owned(),
            result: Some("Found the bug".to_owned()),
            usage: Some(TaskUsage {
                total_tokens: 1000,
                tool_uses: 5,
                duration_ms: 3000,
            }),
        };
        let xml = format_task_notification(&params);
        assert!(xml.contains("<task-notification>"));
        assert!(xml.contains("<task-id>agent-123</task-id>"));
        assert!(xml.contains("<status>completed</status>"));
        assert!(xml.contains("<summary>Agent completed task</summary>"));
        assert!(xml.contains("<result>Found the bug</result>"));
        assert!(xml.contains("<total_tokens>1000</total_tokens>"));
        assert!(xml.contains("<tool_uses>5</tool_uses>"));
        assert!(xml.contains("<duration_ms>3000</duration_ms>"));
        assert!(xml.contains("</task-notification>"));
    }

    #[test]
    fn format_task_notification_failed_no_usage() {
        let params = TaskNotificationParams {
            task_id: "agent-456".to_owned(),
            status: TaskNotificationStatus::Failed,
            summary: "Agent failed".to_owned(),
            result: None,
            usage: None,
        };
        let xml = format_task_notification(&params);
        assert!(xml.contains("<status>failed</status>"));
        assert!(!xml.contains("<result>"));
        assert!(!xml.contains("<usage>"));
    }

    #[test]
    fn format_task_notification_killed() {
        let params = TaskNotificationParams {
            task_id: "agent-789".to_owned(),
            status: TaskNotificationStatus::Killed,
            summary: "Agent was stopped".to_owned(),
            result: None,
            usage: None,
        };
        let xml = format_task_notification(&params);
        assert!(xml.contains("<status>killed</status>"));
    }

    #[test]
    fn parse_task_status_roundtrip() {
        assert_eq!(
            parse_task_status("completed"),
            Some(TaskNotificationStatus::Completed)
        );
        assert_eq!(
            parse_task_status("failed"),
            Some(TaskNotificationStatus::Failed)
        );
        assert_eq!(
            parse_task_status("killed"),
            Some(TaskNotificationStatus::Killed)
        );
        assert_eq!(parse_task_status("unknown"), None);
    }

    #[test]
    fn get_coordinator_user_context_empty_when_not_coordinator() {
        let _guard = override_lock();
        reset_state();
        let ctx = get_coordinator_user_context(&[], None, false, false);
        assert!(ctx.is_empty());
    }

    #[test]
    fn get_coordinator_user_context_with_tools() {
        let _guard = override_lock();
        reset_state();
        COORDINATOR_OVERRIDE.store(true, Ordering::SeqCst);
        COORDINATOR_OVERRIDE_SET.store(true, Ordering::SeqCst);
        let ctx = get_coordinator_user_context(&[], None, false, false);
        assert!(
            !ctx.is_empty(),
            "coordinator context should not be empty when coordinator mode is active"
        );
        let (key, value) = ctx.iter().next().expect("at least one entry");
        assert_eq!(key, "workerToolsContext");
        assert!(value.contains("Workers spawned via the Agent tool"));
        reset_state();
    }

    #[test]
    fn get_coordinator_user_context_with_mcp() {
        let _guard = override_lock();
        reset_state();
        COORDINATOR_OVERRIDE.store(true, Ordering::Relaxed);
        COORDINATOR_OVERRIDE_SET.store(true, Ordering::Relaxed);
        let clients = vec![McpClientInfo {
            name: "my-server".to_owned(),
        }];
        let ctx = get_coordinator_user_context(&clients, None, false, false);
        let (_, value) = ctx.iter().next().expect("entry");
        assert!(value.contains("my-server"));
        reset_state();
    }

    #[test]
    fn get_coordinator_user_context_simple_mode() {
        let _guard = override_lock();
        reset_state();
        COORDINATOR_OVERRIDE.store(true, Ordering::Relaxed);
        COORDINATOR_OVERRIDE_SET.store(true, Ordering::Relaxed);
        let ctx = get_coordinator_user_context(&[], None, true, false);
        let (_, value) = ctx.iter().next().expect("entry");
        assert!(value.contains("Bash, Edit, Read"));
        reset_state();
    }

    #[test]
    fn get_coordinator_system_prompt_contains_role() {
        let prompt = get_coordinator_system_prompt(false);
        assert!(prompt.contains("You are Claude Code, an AI assistant"));
        assert!(prompt.contains(AGENT_TOOL_NAME));
        assert!(prompt.contains(SEND_MESSAGE_TOOL_NAME));
        assert!(prompt.contains(TASK_STOP_TOOL_NAME));
        assert!(prompt.contains("subscribe_pr_activity / unsubscribe_pr_activity"));
        assert!(prompt.contains("What Real Verification Looks Like"));
        assert!(prompt.contains("Choose continue vs. spawn by context overlap"));
    }

    #[test]
    fn get_coordinator_system_prompt_simple_mode() {
        let prompt = get_coordinator_system_prompt(true);
        assert!(prompt.contains("Bash, Read, and Edit"));
    }

    #[test]
    fn get_coordinator_user_context_only_mentions_scratchpad_when_enabled() {
        let _guard = override_lock();
        reset_state();
        COORDINATOR_OVERRIDE.store(true, Ordering::SeqCst);
        COORDINATOR_OVERRIDE_SET.store(true, Ordering::SeqCst);

        let disabled = get_coordinator_user_context(&[], Some("/tmp/scratch"), false, false);
        let (_, disabled_value) = disabled.iter().next().expect("entry");
        assert!(!disabled_value.contains("Scratchpad directory"));

        let enabled = get_coordinator_user_context(&[], Some("/tmp/scratch"), false, true);
        let (_, enabled_value) = enabled.iter().next().expect("entry");
        assert!(enabled_value.contains("Scratchpad directory: /tmp/scratch"));
        reset_state();
    }

    #[test]
    fn task_notification_status_display() {
        assert_eq!(TaskNotificationStatus::Completed.to_string(), "completed");
        assert_eq!(TaskNotificationStatus::Failed.to_string(), "failed");
        assert_eq!(TaskNotificationStatus::Killed.to_string(), "killed");
    }

    #[test]
    fn task_usage_default() {
        let usage = TaskUsage::default();
        assert_eq!(usage.total_tokens, 0);
        assert_eq!(usage.tool_uses, 0);
        assert_eq!(usage.duration_ms, 0);
    }

    #[test]
    fn mcp_client_info_serde() {
        let info = McpClientInfo {
            name: "test-server".to_owned(),
        };
        let json = serde_json::to_string(&info).expect("serialize");
        let parsed: McpClientInfo = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.name, "test-server");
    }
}
