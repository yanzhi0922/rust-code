//! Shared task-list tools plus tracked background task persistence helpers.

use parking_lot::Mutex;
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::fs::OpenOptions;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use claude_ui_bridge::{UiTaskKind, UiTaskNode, UiTaskStatus};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::task_output;

static TASK_STORE: Lazy<Mutex<HashMap<String, BackgroundTask>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
static TASK_OUTPUT_DIR: Lazy<Mutex<Option<PathBuf>>> = Lazy::new(|| Mutex::new(None));
static TASK_LIST_ID: Lazy<Mutex<Option<String>>> = Lazy::new(|| Mutex::new(None));
static TASK_LIST_BASE_DIR: Lazy<Mutex<Option<PathBuf>>> = Lazy::new(|| Mutex::new(None));
static LEADER_TEAM_NAME: Lazy<Mutex<Option<String>>> = Lazy::new(|| Mutex::new(None));

#[cfg(test)]
static TASK_TEST_GUARD: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

const TASK_LIST_HIGH_WATER_MARK_FILE: &str = ".highwatermark";
const TASK_LIST_LOCK_FILE: &str = ".lock";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SharedTaskStatus {
    Pending,
    InProgress,
    Completed,
}

impl SharedTaskStatus {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in_progress",
            Self::Completed => "completed",
        }
    }

    fn parse(value: &str) -> Result<Self> {
        match value {
            "pending" => Ok(Self::Pending),
            "in_progress" => Ok(Self::InProgress),
            "completed" => Ok(Self::Completed),
            other => Err(anyhow!("invalid status '{other}'")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SharedTask {
    pub id: String,
    pub subject: String,
    pub description: String,
    #[serde(
        default,
        rename = "activeForm",
        skip_serializing_if = "Option::is_none"
    )]
    pub active_form: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    pub status: SharedTaskStatus,
    #[serde(default)]
    pub blocks: Vec<String>,
    #[serde(default, rename = "blockedBy")]
    pub blocked_by: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<BTreeMap<String, Value>>,
}

struct TaskListLockGuard {
    path: PathBuf,
}

impl Drop for TaskListLockGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackgroundTask {
    pub id: String,
    #[serde(default)]
    pub parent_task_id: Option<String>,
    #[serde(default)]
    pub depth: u32,
    #[serde(default)]
    pub kind: TaskKind,
    pub title: String,
    pub status: TaskStatus,
    #[serde(default)]
    pub summary: String,
    pub output: String,
    pub output_path: Option<String>,
    #[serde(default)]
    pub turns_used: Option<u32>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedTaskRecord {
    id: String,
    #[serde(default)]
    parent_task_id: Option<String>,
    #[serde(default)]
    depth: u32,
    #[serde(default)]
    kind: TaskKind,
    title: String,
    status: TaskStatus,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    turns_used: Option<u32>,
    created_at: String,
    updated_at: String,
    #[serde(default)]
    output_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    #[default]
    Background,
    Delegation,
    Batch,
}

impl TaskKind {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Background => "background",
            Self::Delegation => "delegation",
            Self::Batch => "batch",
        }
    }

    #[must_use]
    pub fn to_ui_kind(&self) -> UiTaskKind {
        match self {
            Self::Background => UiTaskKind::Background,
            Self::Delegation => UiTaskKind::Delegation,
            Self::Batch => UiTaskKind::Batch,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Stopped,
}

impl TaskStatus {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Stopped => "stopped",
        }
    }

    #[must_use]
    pub fn to_ui_status(&self) -> UiTaskStatus {
        match self {
            Self::Pending => UiTaskStatus::Pending,
            Self::Running => UiTaskStatus::Running,
            Self::Completed => UiTaskStatus::Completed,
            Self::Failed => UiTaskStatus::Failed,
            Self::Stopped => UiTaskStatus::Stopped,
        }
    }
}

fn generate_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("task_{timestamp}_{count}")
}

fn now_timestamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}")
}

#[must_use]
pub fn allocate_task_id() -> String {
    generate_id()
}

#[cfg(test)]
pub(crate) struct TaskTestGuard {
    _guard: parking_lot::MutexGuard<'static, ()>,
}

#[cfg(test)]
impl Drop for TaskTestGuard {
    fn drop(&mut self) {
        reset_task_test_state();
    }
}

#[cfg(test)]
pub(crate) fn test_guard_for_tests() -> TaskTestGuard {
    let guard = TASK_TEST_GUARD.lock();
    reset_task_test_state();
    TaskTestGuard { _guard: guard }
}

#[cfg(test)]
fn reset_task_test_state() {
    TASK_STORE.lock().clear();
    *TASK_OUTPUT_DIR.lock() = None;
    *TASK_LIST_ID.lock() = None;
    *TASK_LIST_BASE_DIR.lock() = None;
    *LEADER_TEAM_NAME.lock() = None;
}

pub fn configure_task_output_dir(path: Option<PathBuf>) -> Result<()> {
    let mut output_dir = TASK_OUTPUT_DIR.lock();
    *output_dir = path;
    Ok(())
}

#[must_use]
pub fn task_snapshots() -> Vec<BackgroundTask> {
    let store = TASK_STORE.lock();
    let mut tasks = store.values().cloned().collect::<Vec<_>>();
    tasks.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
    tasks
}

pub fn load_persisted_tasks(base_dir: &Path) -> Result<Vec<BackgroundTask>> {
    if !base_dir.exists() {
        return Ok(Vec::new());
    }

    let mut tasks = Vec::new();
    for entry in
        fs::read_dir(base_dir).with_context(|| format!("failed to read {}", base_dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let contents = fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let record: PersistedTaskRecord = serde_json::from_str(&contents)
            .with_context(|| format!("failed to parse {}", path.display()))?;
        let output = match &record.output_path {
            Some(output_path) => fs::read_to_string(output_path).unwrap_or_default(),
            None => String::new(),
        };
        tasks.push(BackgroundTask {
            id: record.id,
            parent_task_id: record.parent_task_id,
            depth: record.depth,
            kind: record.kind,
            title: record.title,
            status: record.status,
            summary: record.summary,
            output,
            output_path: record.output_path,
            turns_used: record.turns_used,
            created_at: record.created_at,
            updated_at: record.updated_at,
        });
    }

    tasks.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
    Ok(tasks)
}

pub fn load_persisted_task(base_dir: &Path, task_id: &str) -> Result<Option<BackgroundTask>> {
    Ok(load_persisted_tasks(base_dir)?
        .into_iter()
        .find(|task| task.id == task_id))
}

#[must_use]
pub fn ui_task_snapshots() -> Vec<UiTaskNode> {
    task_snapshots()
        .iter()
        .map(background_task_to_ui_node)
        .collect()
}

pub fn load_persisted_ui_task_snapshots(base_dir: &Path) -> Result<Vec<UiTaskNode>> {
    Ok(load_persisted_tasks(base_dir)?
        .iter()
        .map(background_task_to_ui_node)
        .collect())
}

fn background_task_to_ui_node(task: &BackgroundTask) -> UiTaskNode {
    UiTaskNode {
        id: task.id.clone(),
        parent_task_id: task.parent_task_id.clone(),
        title: task.title.clone(),
        status: task.status.to_ui_status(),
        kind: task.kind.to_ui_kind(),
        depth: task.depth,
        summary: task.summary.clone(),
        turns_used: task.turns_used,
        output_path: task.output_path.clone(),
        created_at: task.created_at.clone(),
        updated_at: task.updated_at.clone(),
    }
}

pub fn create_background_task(title: &str) -> Result<BackgroundTask> {
    let task = BackgroundTask {
        id: generate_id(),
        parent_task_id: None,
        depth: 0,
        kind: TaskKind::Background,
        title: title.to_owned(),
        status: TaskStatus::Pending,
        summary: String::new(),
        output: String::new(),
        output_path: None,
        turns_used: None,
        created_at: now_timestamp(),
        updated_at: now_timestamp(),
    };
    let mut store = TASK_STORE.lock();
    store.insert(task.id.clone(), task.clone());
    drop(store);
    persist_task_if_configured(&task.id)?;
    Ok(task)
}

pub fn start_tracked_task(
    task_id: String,
    title: &str,
    parent_task_id: Option<String>,
    depth: u32,
    kind: TaskKind,
    summary: Option<&str>,
) -> Result<BackgroundTask> {
    let now = now_timestamp();
    let mut store = TASK_STORE.lock();
    let task = store
        .entry(task_id.clone())
        .or_insert_with(|| BackgroundTask {
            id: task_id.clone(),
            parent_task_id: parent_task_id.clone(),
            depth,
            kind: kind.clone(),
            title: title.to_owned(),
            status: TaskStatus::Running,
            summary: summary.unwrap_or_default().to_owned(),
            output: String::new(),
            output_path: None,
            turns_used: None,
            created_at: now.clone(),
            updated_at: now.clone(),
        });
    task.parent_task_id = parent_task_id;
    task.depth = depth;
    task.kind = kind;
    task.title = title.to_owned();
    task.status = TaskStatus::Running;
    if let Some(summary) = summary {
        task.summary = summary.to_owned();
    }
    task.updated_at = now_timestamp();
    persist_existing_task(task)?;
    Ok(task.clone())
}

pub fn update_task_progress(task_id: &str, summary: &str) -> Result<()> {
    let mut store = TASK_STORE.lock();
    let task = store
        .get_mut(task_id)
        .ok_or_else(|| anyhow!("task '{task_id}' not found"))?;
    task.status = TaskStatus::Running;
    task.summary = summary.to_owned();
    task.updated_at = now_timestamp();
    persist_existing_task(task)
}

pub fn mark_task_running(task_id: &str, output: Option<&str>) -> Result<()> {
    let mut store = TASK_STORE.lock();
    let task = store
        .get_mut(task_id)
        .ok_or_else(|| anyhow!("task '{task_id}' not found"))?;
    task.status = TaskStatus::Running;
    if let Some(output) = output {
        task.output = output.to_owned();
    }
    task.updated_at = now_timestamp();
    persist_existing_task(task)
}

pub fn finish_background_task(task_id: &str, status: TaskStatus, output: &str) -> Result<()> {
    finish_tracked_task(task_id, status, None, output, None)
}

pub fn finish_tracked_task(
    task_id: &str,
    status: TaskStatus,
    summary: Option<&str>,
    output: &str,
    turns_used: Option<u32>,
) -> Result<()> {
    let mut store = TASK_STORE.lock();
    let task = store
        .get_mut(task_id)
        .ok_or_else(|| anyhow!("task '{task_id}' not found"))?;
    task.status = status;
    if let Some(summary) = summary {
        task.summary = summary.to_owned();
    }
    task.output = output.to_owned();
    task.turns_used = turns_used;
    task.updated_at = now_timestamp();
    persist_existing_task(task)
}

pub fn task_create(input: &Value) -> Result<String> {
    let subject = input
        .get("subject")
        .and_then(Value::as_str)
        .or_else(|| input.get("title").and_then(Value::as_str))
        .ok_or_else(|| anyhow!("subject is required"))?;
    let description = input
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let active_form = input
        .get("activeForm")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let metadata = input
        .get("metadata")
        .and_then(Value::as_object)
        .map(|value| {
            value
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect::<BTreeMap<_, _>>()
        });

    let task_list_id = current_task_list_id();
    ensure_task_list_dir(&task_list_id)?;
    let _lock = TaskListLockGuard::acquire(&task_list_id)?;
    let task = SharedTask {
        id: next_task_id(&task_list_id)?,
        subject: subject.to_owned(),
        description: description.to_owned(),
        active_form,
        owner: None,
        status: SharedTaskStatus::Pending,
        blocks: Vec::new(),
        blocked_by: Vec::new(),
        metadata,
    };
    write_shared_task(&task_list_id, &task)?;

    Ok(format!(
        "Task #{} created successfully: {}",
        task.id, task.subject
    ))
}

pub fn task_get(input: &Value) -> Result<String> {
    let id = input
        .get("taskId")
        .and_then(Value::as_str)
        .or_else(|| input.get("id").and_then(Value::as_str))
        .ok_or_else(|| anyhow!("taskId is required"))?;
    let task_list_id = current_task_list_id();
    let Some(task) = read_shared_task(&task_list_id, id)? else {
        return Ok("Task not found".to_owned());
    };

    let mut lines = vec![
        format!("Task #{}: {}", task.id, task.subject),
        format!("Status: {}", task.status.as_str()),
        format!("Description: {}", task.description),
    ];
    if !task.blocked_by.is_empty() {
        lines.push(format!(
            "Blocked by: {}",
            task.blocked_by
                .iter()
                .map(|value| format!("#{value}"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !task.blocks.is_empty() {
        lines.push(format!(
            "Blocks: {}",
            task.blocks
                .iter()
                .map(|value| format!("#{value}"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    Ok(lines.join("\n"))
}

pub fn task_list(_input: &Value) -> Result<String> {
    let task_list_id = current_task_list_id();
    let tasks = list_shared_tasks(&task_list_id)?;
    if tasks.is_empty() {
        return Ok("No tasks found".to_owned());
    }

    let resolved = tasks
        .iter()
        .filter(|task| task.status == SharedTaskStatus::Completed)
        .map(|task| task.id.clone())
        .collect::<std::collections::BTreeSet<_>>();

    Ok(tasks
        .into_iter()
        .filter(|task| {
            task.metadata
                .as_ref()
                .and_then(|meta| meta.get("_internal"))
                .and_then(Value::as_bool)
                != Some(true)
        })
        .map(|task| {
            let owner = task
                .owner
                .as_ref()
                .map(|value| format!(" ({value})"))
                .unwrap_or_default();
            let blocked_by = task
                .blocked_by
                .into_iter()
                .filter(|value| !resolved.contains(value))
                .map(|value| format!("#{value}"))
                .collect::<Vec<_>>();
            let blocked = if blocked_by.is_empty() {
                String::new()
            } else {
                format!(" [blocked by {}]", blocked_by.join(", "))
            };
            format!(
                "#{} [{}] {}{}{}",
                task.id,
                task.status.as_str(),
                task.subject,
                owner,
                blocked
            )
        })
        .collect::<Vec<_>>()
        .join("\n"))
}

pub fn task_update(input: &Value) -> Result<String> {
    let id = input
        .get("taskId")
        .and_then(Value::as_str)
        .or_else(|| input.get("id").and_then(Value::as_str))
        .ok_or_else(|| anyhow!("taskId is required"))?;
    let task_list_id = current_task_list_id();
    let _lock = TaskListLockGuard::acquire(&task_list_id)?;
    let Some(mut task) = read_shared_task(&task_list_id, id)? else {
        return Ok(format!("Task #{id} not found"));
    };

    let mut updated_fields = Vec::new();
    let prior_status = task.status.clone();

    if let Some(subject) = input.get("subject").and_then(Value::as_str)
        && subject != task.subject
    {
        task.subject = subject.to_owned();
        updated_fields.push("subject");
    }
    if let Some(description) = input.get("description").and_then(Value::as_str)
        && description != task.description
    {
        task.description = description.to_owned();
        updated_fields.push("description");
    }
    if input.get("activeForm").is_some() {
        let next = input
            .get("activeForm")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        if next != task.active_form {
            task.active_form = next;
            updated_fields.push("activeForm");
        }
    }
    if input.get("owner").is_some() {
        let next = input
            .get("owner")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        if next != task.owner {
            task.owner = next;
            updated_fields.push("owner");
        }
    } else if input.get("status").and_then(Value::as_str) == Some("in_progress")
        && task.owner.is_none()
        && std::env::var(claude_swarm::constants::ENV_TEAM_NAME)
            .ok()
            .is_some_and(|value| !value.trim().is_empty())
    {
        let next = std::env::var(claude_swarm::constants::ENV_AGENT_NAME)
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        if next.is_some() && next != task.owner {
            task.owner = next;
            updated_fields.push("owner");
        }
    }
    if let Some(status) = input.get("status").and_then(Value::as_str) {
        if status == "deleted" {
            let deleted = delete_shared_task(&task_list_id, id)?;
            return Ok(if deleted {
                format!("Updated task #{id} deleted")
            } else {
                format!("Task #{id} not found")
            });
        }
        let next = SharedTaskStatus::parse(status)?;
        if next != task.status {
            task.status = next;
            updated_fields.push("status");
        }
    }
    if let Some(metadata) = input.get("metadata").and_then(Value::as_object) {
        task.metadata = merge_task_metadata(task.metadata.take(), Some(metadata));
        updated_fields.push("metadata");
    }

    write_shared_task(&task_list_id, &task)?;

    if let Some(values) = input.get("addBlocks").and_then(Value::as_array) {
        let mut changed = false;
        for value in values.iter().filter_map(Value::as_str) {
            changed |= add_task_dependency(&task_list_id, id, value)?;
        }
        if changed {
            updated_fields.push("blocks");
        }
    }

    if let Some(values) = input.get("addBlockedBy").and_then(Value::as_array) {
        let mut changed = false;
        for value in values.iter().filter_map(Value::as_str) {
            changed |= add_task_dependency(&task_list_id, value, id)?;
        }
        if changed {
            updated_fields.push("blockedBy");
        }
    }

    let mut result = if updated_fields.is_empty() {
        format!("Updated task #{id}")
    } else {
        format!("Updated task #{id} {}", updated_fields.join(", "))
    };
    if prior_status != task.status && task.status == SharedTaskStatus::Completed {
        let has_teammate_context = std::env::var(claude_swarm::constants::ENV_TEAM_NAME)
            .ok()
            .is_some_and(|value| !value.trim().is_empty());
        if has_teammate_context {
            result.push_str(
                "\n\nTask completed. Call TaskList now to find your next available task or see if your work unblocked others.",
            );
        }
    }
    Ok(result)
}

/// Get the output of a background task by ID.
pub fn task_output(input: &Value) -> Result<String> {
    let task_id = input["task_id"]
        .as_str()
        .ok_or_else(|| anyhow!("task_id is required"))?;

    let store = TASK_STORE.lock();

    let task = store
        .get(task_id)
        .ok_or_else(|| anyhow!("task '{task_id}' not found"))?;

    let status = match task.status {
        TaskStatus::Pending => "pending",
        TaskStatus::Running => "running",
        TaskStatus::Completed => "completed",
        TaskStatus::Stopped => "stopped",
        TaskStatus::Failed => "failed",
    };

    let mut result = format!(
        "Task ID: {}\nStatus: {}\nTitle: {}",
        task.id, status, task.title
    );

    if let Some(ref output_path) = task.output_path {
        let path = PathBuf::from(output_path);
        if path.exists()
            && let Ok(content) = std::fs::read_to_string(&path)
        {
            result.push_str(&format!("\n\nOutput:\n{content}"));
        }
    }

    Ok(result)
}

/// Stop a running background task by ID.
pub fn task_stop(input: &Value) -> Result<String> {
    let task_id = input["task_id"]
        .as_str()
        .ok_or_else(|| anyhow!("task_id is required"))?;

    let output_dir = TASK_OUTPUT_DIR.lock().clone();

    let mut store = TASK_STORE.lock();

    let task = store
        .get_mut(task_id)
        .ok_or_else(|| anyhow!("task '{task_id}' not found"))?;

    if matches!(
        task.status,
        TaskStatus::Stopped | TaskStatus::Completed | TaskStatus::Failed
    ) {
        return Ok(format!(
            "Task '{}' is already {} (no action taken).",
            task_id,
            match task.status {
                TaskStatus::Stopped => "stopped",
                TaskStatus::Completed => "completed",
                TaskStatus::Failed => "failed",
                _ => "finished",
            }
        ));
    }

    task.status = TaskStatus::Stopped;
    task.updated_at = now_timestamp();
    if task.summary.trim().is_empty() {
        task.summary = "Stopped by user".to_owned();
    }

    if let Some(output_dir) = output_dir.as_ref() {
        let persisted_path = task_output::persist_task(output_dir, task)?;
        task.output_path = persisted_path.map(|path| path.display().to_string());
    }

    Ok(format!("Task '{}' stopped successfully.", task_id))
}

/// Stop any active tracked tasks and clear the in-memory task store.
///
/// This is used during session reset flows such as `/clear` so stale task
/// state does not leak into the next session. Pending/running tasks are first
/// marked `stopped` and re-persisted into the current task artifact directory.
///
/// # Errors
/// Returns an error if the task store is poisoned or persisted task metadata
/// cannot be updated.
pub fn stop_and_clear_tracked_tasks(stop_reason: &str) -> Result<Vec<BackgroundTask>> {
    let output_dir = TASK_OUTPUT_DIR.lock().clone();

    let mut store = TASK_STORE.lock();

    let mut snapshots = Vec::with_capacity(store.len());
    for task in store.values_mut() {
        if matches!(task.status, TaskStatus::Pending | TaskStatus::Running) {
            task.status = TaskStatus::Stopped;
            task.updated_at = now_timestamp();
            if task.summary.trim().is_empty() {
                task.summary = stop_reason.to_owned();
            } else if !task.summary.contains(stop_reason) {
                task.summary = format!("{} ({stop_reason})", task.summary);
            }

            if let Some(output_dir) = output_dir.as_ref() {
                let persisted_path = task_output::persist_task(output_dir, task)?;
                task.output_path = persisted_path.map(|path| path.display().to_string());
            }
        }

        snapshots.push(task.clone());
    }

    store.clear();
    Ok(snapshots)
}

fn persist_task_if_configured(task_id: &str) -> Result<()> {
    let output_dir = TASK_OUTPUT_DIR.lock().clone();
    let Some(output_dir) = output_dir else {
        return Ok(());
    };

    let mut store = TASK_STORE.lock();
    let task = store
        .get_mut(task_id)
        .ok_or_else(|| anyhow!("task '{task_id}' not found"))?;
    let persisted_path = task_output::persist_task(&output_dir, task)?;
    task.output_path = persisted_path.map(|path| path.display().to_string());
    Ok(())
}

fn persist_existing_task(task: &mut BackgroundTask) -> Result<()> {
    let output_dir = TASK_OUTPUT_DIR.lock().clone();
    let Some(output_dir) = output_dir else {
        return Ok(());
    };

    let persisted_path = task_output::persist_task(&output_dir, task)?;
    task.output_path = persisted_path.map(|path| path.display().to_string());
    Ok(())
}

pub fn configure_task_list_context(
    task_list_id: Option<String>,
    base_dir: Option<PathBuf>,
) -> Result<()> {
    let mut configured_task_list_id = TASK_LIST_ID.lock();
    *configured_task_list_id = task_list_id
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    drop(configured_task_list_id);

    let mut configured_base_dir = TASK_LIST_BASE_DIR.lock();
    *configured_base_dir = base_dir;
    Ok(())
}

pub fn configure_task_list_base_dir(base_dir: Option<PathBuf>) -> Result<()> {
    let mut configured_base_dir = TASK_LIST_BASE_DIR.lock();
    *configured_base_dir = base_dir;
    Ok(())
}

pub fn set_leader_team_name(team_name: Option<String>) -> Result<()> {
    let mut current = LEADER_TEAM_NAME.lock();
    *current = team_name
        .map(|value| sanitize_task_path_component(value.trim()))
        .filter(|value| !value.is_empty());
    Ok(())
}

pub fn leader_team_name() -> Result<Option<String>> {
    Ok(LEADER_TEAM_NAME.lock().clone())
}

pub fn clear_leader_team_name() -> Result<()> {
    set_leader_team_name(None)
}

#[must_use]
pub fn task_list_base_dir() -> PathBuf {
    TASK_LIST_BASE_DIR
        .lock()
        .clone()
        .unwrap_or_else(|| claude_swarm::team_helpers::claude_config_home_dir().join("tasks"))
}

#[must_use]
pub fn task_list_dir(task_list_id: &str) -> PathBuf {
    task_list_base_dir().join(sanitize_task_path_component(task_list_id))
}

#[must_use]
pub fn current_task_list_id() -> String {
    if let Ok(value) = std::env::var("CLAUDE_CODE_TASK_LIST_ID") {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return trimmed.to_owned();
        }
    }

    if let Ok(value) = std::env::var(claude_swarm::constants::ENV_TEAM_NAME) {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return trimmed.to_owned();
        }
    }

    if let Some(value) = TASK_LIST_ID.lock().as_ref() {
        return value.clone();
    }

    if let Some(value) = LEADER_TEAM_NAME.lock().as_ref() {
        return value.clone();
    }

    "default".to_owned()
}

pub fn ensure_task_list_dir(task_list_id: &str) -> Result<()> {
    let dir = task_list_dir(task_list_id);
    fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))
}

pub fn reset_task_list(task_list_id: &str) -> Result<()> {
    ensure_task_list_dir(task_list_id)?;
    let _lock = TaskListLockGuard::acquire(task_list_id)?;
    let dir = task_list_dir(task_list_id);
    let highest =
        current_high_water_mark(task_list_id)?.max(find_highest_task_id_from_files(task_list_id)?);
    if highest > 0 {
        write_high_water_mark(task_list_id, highest)?;
    }

    for entry in fs::read_dir(&dir).with_context(|| format!("failed to read {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("json") {
            let _ = fs::remove_file(&path);
        }
    }

    Ok(())
}

fn sanitize_task_path_component(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect()
}

impl TaskListLockGuard {
    fn acquire(task_list_id: &str) -> Result<Self> {
        ensure_task_list_dir(task_list_id)?;
        let lock_path = task_list_dir(task_list_id).join(TASK_LIST_LOCK_FILE);
        for _ in 0..100 {
            match OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&lock_path)
            {
                Ok(_) => return Ok(Self { path: lock_path }),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("failed to lock {}", lock_path.display()));
                }
            }
        }

        Err(anyhow!(
            "timed out waiting for task list lock {}",
            lock_path.display()
        ))
    }
}

fn high_water_mark_path(task_list_id: &str) -> PathBuf {
    task_list_dir(task_list_id).join(TASK_LIST_HIGH_WATER_MARK_FILE)
}

fn current_high_water_mark(task_list_id: &str) -> Result<u64> {
    let path = high_water_mark_path(task_list_id);
    match fs::read_to_string(&path) {
        Ok(contents) => Ok(contents.trim().parse::<u64>().unwrap_or(0)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(error) => Err(error).with_context(|| format!("failed to read {}", path.display())),
    }
}

fn write_high_water_mark(task_list_id: &str, value: u64) -> Result<()> {
    let path = high_water_mark_path(task_list_id);
    fs::write(&path, value.to_string())
        .with_context(|| format!("failed to write {}", path.display()))
}

fn task_path(task_list_id: &str, task_id: &str) -> PathBuf {
    task_list_dir(task_list_id).join(format!("{}.json", sanitize_task_path_component(task_id)))
}

fn find_highest_task_id_from_files(task_list_id: &str) -> Result<u64> {
    let dir = task_list_dir(task_list_id);
    if !dir.exists() {
        return Ok(0);
    }

    let mut highest = 0;
    for entry in fs::read_dir(&dir).with_context(|| format!("failed to read {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        if let Ok(value) = stem.parse::<u64>() {
            highest = highest.max(value);
        }
    }

    Ok(highest)
}

fn next_task_id(task_list_id: &str) -> Result<String> {
    let next = current_high_water_mark(task_list_id)?
        .max(find_highest_task_id_from_files(task_list_id)?)
        + 1;
    write_high_water_mark(task_list_id, next)?;
    Ok(next.to_string())
}

fn read_shared_task(task_list_id: &str, task_id: &str) -> Result<Option<SharedTask>> {
    let path = task_path(task_list_id, task_id);
    let contents = match fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", path.display()));
        }
    };
    let task = serde_json::from_str::<SharedTask>(&contents)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    Ok(Some(task))
}

fn write_shared_task(task_list_id: &str, task: &SharedTask) -> Result<()> {
    ensure_task_list_dir(task_list_id)?;
    let path = task_path(task_list_id, &task.id);
    let contents = serde_json::to_vec_pretty(task)?;
    fs::write(&path, contents).with_context(|| format!("failed to write {}", path.display()))
}

fn list_shared_tasks(task_list_id: &str) -> Result<Vec<SharedTask>> {
    let dir = task_list_dir(task_list_id);
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut tasks = Vec::new();
    for entry in fs::read_dir(&dir).with_context(|| format!("failed to read {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let contents = fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        if let Ok(task) = serde_json::from_str::<SharedTask>(&contents) {
            tasks.push(task);
        }
    }

    tasks.sort_by(|left, right| {
        let left_num = left.id.parse::<u64>().unwrap_or(u64::MAX);
        let right_num = right.id.parse::<u64>().unwrap_or(u64::MAX);
        left_num
            .cmp(&right_num)
            .then_with(|| left.id.cmp(&right.id))
    });
    Ok(tasks)
}

fn merge_task_metadata(
    existing: Option<BTreeMap<String, Value>>,
    update: Option<&serde_json::Map<String, Value>>,
) -> Option<BTreeMap<String, Value>> {
    let Some(update) = update else {
        return existing;
    };

    let mut merged = existing.unwrap_or_default();
    for (key, value) in update {
        if value.is_null() {
            merged.remove(key);
        } else {
            merged.insert(key.clone(), value.clone());
        }
    }
    if merged.is_empty() {
        None
    } else {
        Some(merged)
    }
}

fn add_task_dependency(task_list_id: &str, from_task_id: &str, to_task_id: &str) -> Result<bool> {
    let Some(mut from_task) = read_shared_task(task_list_id, from_task_id)? else {
        return Ok(false);
    };
    let Some(mut to_task) = read_shared_task(task_list_id, to_task_id)? else {
        return Ok(false);
    };

    if !from_task.blocks.iter().any(|value| value == to_task_id) {
        from_task.blocks.push(to_task_id.to_owned());
        from_task.blocks.sort();
        from_task.blocks.dedup();
        write_shared_task(task_list_id, &from_task)?;
    }

    if !to_task.blocked_by.iter().any(|value| value == from_task_id) {
        to_task.blocked_by.push(from_task_id.to_owned());
        to_task.blocked_by.sort();
        to_task.blocked_by.dedup();
        write_shared_task(task_list_id, &to_task)?;
    }

    Ok(true)
}

fn delete_shared_task(task_list_id: &str, task_id: &str) -> Result<bool> {
    let path = task_path(task_list_id, task_id);
    match fs::remove_file(&path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to delete {}", path.display()));
        }
    }

    if let Ok(value) = task_id.parse::<u64>() {
        let current = current_high_water_mark(task_list_id)?;
        if value > current {
            write_high_water_mark(task_list_id, value)?;
        }
    }

    for mut task in list_shared_tasks(task_list_id)? {
        let original_blocks = task.blocks.len();
        let original_blocked_by = task.blocked_by.len();
        task.blocks.retain(|value| value != task_id);
        task.blocked_by.retain(|value| value != task_id);
        if task.blocks.len() != original_blocks || task.blocked_by.len() != original_blocked_by {
            write_shared_task(task_list_id, &task)?;
        }
    }

    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    fn test_guard() -> super::TaskTestGuard {
        super::test_guard_for_tests()
    }

    fn configure_shared_task_list(task_list_id: &str) -> TempDir {
        let tempdir = tempfile::tempdir().expect("tempdir");
        configure_task_list_context(
            Some(task_list_id.to_owned()),
            Some(tempdir.path().join("tasks")),
        )
        .expect("configure task list context");
        tempdir
    }

    #[test]
    fn task_create_and_get_work() {
        let _guard = test_guard();
        let _tempdir = configure_shared_task_list("task-create-and-get");
        let create_result = task_create(&json!({
            "subject": "Test task",
            "description": "Do the test work"
        }));
        assert!(create_result.is_ok(), "create failed: {:?}", create_result);

        assert_eq!(
            create_result.expect("create should work"),
            "Task #1 created successfully: Test task"
        );

        let get_result = task_get(&json!({"taskId": "1"}));
        assert!(get_result.is_ok(), "get failed: {:?}", get_result);

        let get_str = get_result.expect("get should work");
        assert!(get_str.contains("Task #1: Test task"));
        assert!(get_str.contains("Status: pending"));
        assert!(get_str.contains("Description: Do the test work"));

        let task = read_shared_task("task-create-and-get", "1")
            .expect("read shared task")
            .expect("task should exist");
        assert_eq!(task.subject, "Test task");
        assert_eq!(task.description, "Do the test work");
        assert_eq!(task.status, SharedTaskStatus::Pending);
    }

    #[test]
    fn task_update_changes_status_and_owner() {
        let _guard = test_guard();
        let _tempdir = configure_shared_task_list("task-update");
        task_create(&json!({
            "subject": "Update test",
            "description": "Change the state"
        }))
        .expect("create should work");

        let update_result = task_update(&json!({
            "taskId": "1",
            "status": "in_progress",
            "owner": "worker-1"
        }));
        assert!(update_result.is_ok(), "update failed: {:?}", update_result);

        let get_str = task_get(&json!({"taskId": "1"})).expect("get should work");
        assert!(get_str.contains("Status: in_progress"));

        let task = read_shared_task("task-update", "1")
            .expect("read shared task")
            .expect("task should exist");
        assert_eq!(task.status, SharedTaskStatus::InProgress);
        assert_eq!(task.owner.as_deref(), Some("worker-1"));
    }

    #[test]
    fn tracked_task_records_tree_metadata() {
        let _guard = test_guard();
        let task = start_tracked_task(
            "delegation-root".to_owned(),
            "Fix delegation",
            Some("parent-1".to_owned()),
            2,
            TaskKind::Delegation,
            Some("started"),
        )
        .expect("tracked task");

        assert_eq!(task.parent_task_id.as_deref(), Some("parent-1"));
        assert_eq!(task.depth, 2);
        assert_eq!(task.kind.as_str(), "delegation");
        assert_eq!(task.summary, "started");
    }

    #[test]
    fn task_list_filters_internal_tasks_and_resolved_blockers() {
        let _guard = test_guard();
        let _tempdir = configure_shared_task_list("task-list");
        task_create(&json!({"subject": "Compile", "description": "Compile the project"}))
            .expect("create first task");
        task_create(&json!({"subject": "Test", "description": "Run tests"}))
            .expect("create second task");
        task_create(&json!({
            "subject": "Internal",
            "description": "Hidden task",
            "metadata": {"_internal": true}
        }))
        .expect("create internal task");
        task_update(&json!({"taskId": "2", "addBlockedBy": ["1"]})).expect("add blocker");
        task_update(&json!({"taskId": "1", "status": "completed"})).expect("complete blocker");

        let list_result = task_list(&json!({}));
        assert!(list_result.is_ok(), "list failed: {:?}", list_result);

        let list_str = list_result.expect("list should work");
        assert!(list_str.contains("#1 [completed] Compile"));
        assert!(list_str.contains("#2 [pending] Test"));
        assert!(!list_str.contains("Internal"));
        assert!(!list_str.contains("[blocked by #1]"));
    }

    #[test]
    fn task_get_missing_returns_task_not_found() {
        let _guard = test_guard();
        let _tempdir = configure_shared_task_list("task-missing");
        let result = task_get(&json!({"taskId": "nonexistent"})).expect("task_get should work");
        assert_eq!(result, "Task not found");
    }

    #[test]
    fn task_update_deleted_removes_file_and_dependencies() {
        let _guard = test_guard();
        let _tempdir = configure_shared_task_list("task-delete");
        task_create(&json!({"subject": "Blocker", "description": "Do blocker work"}))
            .expect("create blocker");
        task_create(&json!({"subject": "Blocked", "description": "Do blocked work"}))
            .expect("create blocked");
        task_update(&json!({"taskId": "1", "addBlocks": ["2"]})).expect("add dependency");

        let delete_result =
            task_update(&json!({"taskId": "1", "status": "deleted"})).expect("delete task");
        assert_eq!(delete_result, "Updated task #1 deleted");
        assert!(
            read_shared_task("task-delete", "1")
                .expect("read deleted task")
                .is_none()
        );

        let blocked = read_shared_task("task-delete", "2")
            .expect("read blocked task")
            .expect("blocked task should exist");
        assert!(blocked.blocked_by.is_empty());
        assert!(blocked.blocks.is_empty());
    }

    #[test]
    fn ui_task_snapshot_exports_task_tree_fields() {
        let _guard = test_guard();
        let task_id = allocate_task_id();
        start_tracked_task(
            task_id.clone(),
            "Snapshot task",
            Some("parent-x".to_owned()),
            1,
            TaskKind::Delegation,
            Some("working"),
        )
        .expect("tracked task");

        let tasks = ui_task_snapshots();
        let task = tasks
            .into_iter()
            .find(|task| task.id == task_id)
            .expect("snapshot should contain task");
        assert_eq!(task.parent_task_id.as_deref(), Some("parent-x"));
        assert_eq!(task.depth, 1);
    }

    #[test]
    fn configure_output_dir_persists_task_output() {
        let _guard = test_guard();
        let tempdir = tempfile::tempdir().expect("tempdir");
        configure_task_output_dir(Some(tempdir.path().to_path_buf())).expect("configure output");
        let task = create_background_task("Persist output test").expect("create background task");
        finish_background_task(&task.id, TaskStatus::Completed, "done")
            .expect("finish background task");

        let persisted = load_persisted_task(tempdir.path(), &task.id)
            .expect("load persisted task")
            .expect("persisted task should exist");
        assert_eq!(persisted.output, "done");
        assert!(persisted.output_path.is_some());
        assert!(tempdir.path().join(format!("{}.json", task.id)).exists());
    }

    #[test]
    fn load_persisted_tasks_reads_metadata_and_output() {
        let _guard = test_guard();
        let tempdir = tempfile::tempdir().expect("tempdir");
        let task_id = allocate_task_id();
        crate::task_output::persist_task(
            tempdir.path(),
            &BackgroundTask {
                id: task_id.clone(),
                parent_task_id: None,
                depth: 0,
                kind: TaskKind::Delegation,
                title: "Persisted task".to_owned(),
                status: TaskStatus::Completed,
                summary: "done".to_owned(),
                output: "captured output".to_owned(),
                output_path: None,
                turns_used: Some(2),
                created_at: "1".to_owned(),
                updated_at: "2".to_owned(),
            },
        )
        .expect("persist task");

        let loaded = load_persisted_tasks(tempdir.path()).expect("load persisted tasks");
        let loaded_task = loaded
            .into_iter()
            .find(|candidate| candidate.id == task_id)
            .expect("persisted task should exist");
        assert_eq!(loaded_task.summary, "done");
        assert_eq!(loaded_task.output, "captured output");
    }

    #[test]
    fn load_persisted_ui_task_snapshots_projects_task_tree() {
        let _guard = test_guard();
        let tempdir = tempfile::tempdir().expect("tempdir");
        let task_id = allocate_task_id();
        crate::task_output::persist_task(
            tempdir.path(),
            &BackgroundTask {
                id: task_id.clone(),
                parent_task_id: Some("parent-task".to_owned()),
                depth: 2,
                kind: TaskKind::Batch,
                title: "UI task".to_owned(),
                status: TaskStatus::Completed,
                summary: "done".to_owned(),
                output: "batch output".to_owned(),
                output_path: None,
                turns_used: Some(4),
                created_at: "1".to_owned(),
                updated_at: "2".to_owned(),
            },
        )
        .expect("persist task");

        let snapshots =
            load_persisted_ui_task_snapshots(tempdir.path()).expect("load persisted ui snapshots");
        let snapshot = snapshots
            .into_iter()
            .find(|candidate| candidate.id == task_id)
            .expect("persisted UI task should exist");
        assert_eq!(snapshot.parent_task_id.as_deref(), Some("parent-task"));
        assert_eq!(snapshot.depth, 2);
        assert_eq!(snapshot.summary, "done");
        assert_eq!(snapshot.turns_used, Some(4));
    }

    #[test]
    fn stop_and_clear_tracked_tasks_stops_active_tasks_and_empties_store() {
        let _guard = test_guard();
        let tempdir = tempfile::tempdir().expect("tempdir");
        configure_task_output_dir(Some(tempdir.path().to_path_buf())).expect("configure output");

        let pending = create_background_task("Pending task").expect("create task");
        let pending_id = pending.id.clone();

        let completed_id = allocate_task_id();
        start_tracked_task(
            completed_id.clone(),
            "Completed task",
            None,
            0,
            TaskKind::Delegation,
            Some("working"),
        )
        .expect("start tracked task");
        finish_tracked_task(
            &completed_id,
            TaskStatus::Completed,
            Some("done"),
            "final output",
            Some(1),
        )
        .expect("finish tracked task");

        let drained =
            stop_and_clear_tracked_tasks("stopped by session clear").expect("clear tracked tasks");
        assert_eq!(drained.len(), 2);
        assert!(task_snapshots().is_empty(), "task store should be empty");

        let pending_persisted = load_persisted_task(tempdir.path(), &pending_id)
            .expect("load persisted pending task")
            .expect("pending task should persist");
        assert!(matches!(pending_persisted.status, TaskStatus::Stopped));
        assert!(
            pending_persisted
                .summary
                .contains("stopped by session clear")
        );

        let completed_persisted = load_persisted_task(tempdir.path(), &completed_id)
            .expect("load persisted completed task")
            .expect("completed task should persist");
        assert!(matches!(completed_persisted.status, TaskStatus::Completed));
        assert_eq!(completed_persisted.summary, "done");
    }
}
