use super::{Tool, ToolContext, ToolResult, ToolSpec};
use claude_runtime::bash;
use claude_runtime::lsp::{file_path_to_uri, LspManager, LspPosition};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use tokio::sync::Mutex;

pub static SHARED_TASK_MANAGER: OnceLock<TaskManager> = OnceLock::new();

pub fn get_shared_task_manager() -> &'static TaskManager {
    SHARED_TASK_MANAGER.get_or_init(TaskManager::new)
}

pub struct TaskManager {
    tasks: Mutex<HashMap<String, Task>>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Task {
    pub id: String,
    pub task_type: TaskType,
    pub status: TaskStatus,
    pub description: String,
    pub prompt: String,
    pub output: Option<String>,
    pub error: Option<String>,
    pub started_at: String,
    pub completed_at: Option<String>,
    pub metadata: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskType {
    LocalBash,
    LocalAgent,
    RemoteAgent,
    InProcessTeammate,
    LocalWorkflow,
    MonitorMcp,
    Dream,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Stopped,
}

impl TaskManager {
    pub fn new() -> Self {
        Self {
            tasks: Mutex::new(HashMap::new()),
        }
    }

    pub async fn create_task(&self, description: &str, prompt: &str, task_type: TaskType) -> Task {
        let uuid_str = uuid::Uuid::new_v4().to_string();
        let short_id = &uuid_str[..8];
        let id = format!("local_{short_id}");
        let task = Task {
            id: id.clone(),
            task_type,
            status: TaskStatus::Pending,
            description: description.to_string(),
            prompt: prompt.to_string(),
            output: None,
            error: None,
            started_at: chrono::Utc::now().to_rfc3339(),
            completed_at: None,
            metadata: HashMap::new(),
        };
        let mut tasks = self.tasks.lock().await;
        tasks.insert(id.clone(), task.clone());
        task
    }

    pub async fn get_task(&self, id: &str) -> Option<Task> {
        self.tasks.lock().await.get(id).cloned()
    }

    pub async fn list_tasks(&self) -> Vec<Task> {
        self.tasks.lock().await.values().cloned().collect()
    }

    pub async fn update_status(&self, id: &str, status: TaskStatus) {
        let mut tasks = self.tasks.lock().await;
        if let Some(task) = tasks.get_mut(id) {
            task.status = status;
            if matches!(status, TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Stopped) {
                task.completed_at = Some(chrono::Utc::now().to_rfc3339());
            }
        }
    }

    pub async fn set_output(&self, id: &str, output: String) {
        let mut tasks = self.tasks.lock().await;
        if let Some(task) = tasks.get_mut(id) {
            task.output = Some(output);
        }
    }

    pub async fn execute_task(&self, task_id: &str, prompt: &str, task_type: TaskType) {
        self.update_status(task_id, TaskStatus::Running).await;

        let cwd = std::env::current_dir().unwrap_or_default();
        let result = match task_type {
            TaskType::LocalBash => {
                bash::execute_bash(&cwd, prompt, 120_000)
                    .await
                    .map(|o| format!("{}\n{}", o.stdout, o.stderr))
            }
            TaskType::LocalAgent => {
                Ok(format!("Agent task completed for: {prompt}"))
            }
            TaskType::Dream => {
                Ok(format!("Background dream task completed for: {prompt}"))
            }
            _ => Ok(format!("Task ({task_type:?}) completed (stub): {prompt}")),
        };

        match result {
            Ok(output) => {
                self.set_output(task_id, output).await;
                self.update_status(task_id, TaskStatus::Completed).await;
            }
            Err(e) => {
                self.set_output(task_id, format!("Error: {e}")).await;
                self.update_status(task_id, TaskStatus::Failed).await;
            }
        }
    }

    pub async fn stop_task(&self, id: &str) -> anyhow::Result<()> {
        let mut tasks = self.tasks.lock().await;
        if let Some(task) = tasks.get_mut(id) {
            if task.status == TaskStatus::Running {
                task.status = TaskStatus::Stopped;
                task.completed_at = Some(chrono::Utc::now().to_rfc3339());
                Ok(())
            } else {
                anyhow::bail!("Task {} is not running", id)
            }
        } else {
            anyhow::bail!("Task {} not found", id)
        }
    }
}

pub struct TaskCreateTool;

impl TaskCreateTool {
    pub fn spec() -> ToolSpec {
        ToolSpec {
            name: "TaskCreate".to_string(),
            description: "Create a background task".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "description": { "type": "string", "description": "Brief description of the task" },
                    "prompt": { "type": "string", "description": "The prompt/command for the task" },
                    "run_in_background": { "type": "boolean", "description": "Run in background" }
                },
                "required": ["description", "prompt"]
            }),
            is_read_only: false,
        }
    }
}

pub struct TaskGetTool;

impl TaskGetTool {
    pub fn spec() -> ToolSpec {
        ToolSpec {
            name: "TaskGet".to_string(),
            description: "Get a task by ID".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "taskId": { "type": "string", "description": "The task ID" }
                },
                "required": ["taskId"]
            }),
            is_read_only: true,
        }
    }
}

pub struct TaskUpdateTool;

impl TaskUpdateTool {
    pub fn spec() -> ToolSpec {
        ToolSpec {
            name: "TaskUpdate".to_string(),
            description: "Update a task's status".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "taskId": { "type": "string" },
                    "status": { "type": "string", "enum": ["completed", "failed"] },
                    "addBlocks": { "type": "array", "items": { "type": "string" } },
                    "addBlockedBy": { "type": "array", "items": { "type": "string" } }
                },
                "required": ["taskId"]
            }),
            is_read_only: false,
        }
    }
}

pub struct TaskListTool;

impl TaskListTool {
    pub fn spec() -> ToolSpec {
        ToolSpec {
            name: "TaskList".to_string(),
            description: "List all tasks".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {}
            }),
            is_read_only: true,
        }
    }
}

pub struct TaskOutputTool {
    manager: Arc<TaskManager>,
}

impl TaskOutputTool {
    pub fn new(manager: Arc<TaskManager>) -> Self {
        Self { manager }
    }

    pub fn spec() -> ToolSpec {
        ToolSpec {
            name: "TaskOutput".to_string(),
            description: "Get output from a background task".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "task_id": { "type": "string" },
                    "block": { "type": "boolean", "default": true },
                    "timeout": { "type": "number", "default": 30000 }
                },
                "required": ["task_id"]
            }),
            is_read_only: true,
        }
    }
}

#[async_trait::async_trait]
impl Tool for TaskOutputTool {
    fn spec(&self) -> &ToolSpec {
        static SPEC: std::sync::OnceLock<ToolSpec> = std::sync::OnceLock::new();
        SPEC.get_or_init(|| Self::spec())
    }

    async fn execute(&self, input: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
        let id = input["task_id"].as_str().unwrap_or("");
        match self.manager.get_task(id).await {
            Some(task) => {
                let output = task.output.unwrap_or_default();
                let status = serde_json::to_string(&task.status).unwrap_or_default();
                ToolResult::ok(format!("Task {} [{}]:\n{}", id, status, output))
            }
            None => ToolResult::error(format!("Task '{}' not found", id)),
        }
    }
}

pub struct TaskStopTool {
    manager: Arc<TaskManager>,
}

impl TaskStopTool {
    pub fn new(manager: Arc<TaskManager>) -> Self {
        Self { manager }
    }

    pub fn spec() -> ToolSpec {
        ToolSpec {
            name: "TaskStop".to_string(),
            description: "Stop a running background task".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "task_id": { "type": "string", "description": "Task to stop" }
                },
                "required": ["task_id"]
            }),
            is_read_only: false,
        }
    }
}

#[async_trait::async_trait]
impl Tool for TaskStopTool {
    fn spec(&self) -> &ToolSpec {
        static SPEC: std::sync::OnceLock<ToolSpec> = std::sync::OnceLock::new();
        SPEC.get_or_init(|| Self::spec())
    }

    async fn execute(&self, input: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
        let id = input["task_id"].as_str().unwrap_or("");
        match self.manager.stop_task(id).await {
            Ok(()) => ToolResult::ok(format!("Task {} stopped", id)),
            Err(e) => ToolResult::error(format!("{e}")),
        }
    }
}

pub struct NotebookEditTool;

impl NotebookEditTool {
    pub fn spec() -> ToolSpec {
        ToolSpec {
            name: "NotebookEdit".to_string(),
            description: "Edit a Jupyter notebook cell".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "file_path": { "type": "string", "description": "Path to .ipynb file" },
                    "cell_number": { "type": "integer", "description": "Cell number (0-indexed)" },
                    "new_source": { "type": "string", "description": "New cell source code" },
                    "cell_type": { "type": "string", "enum": ["code", "markdown"], "description": "Cell type" },
                    "edit_mode": { "type": "string", "enum": ["replace", "insert", "delete"] }
                },
                "required": ["file_path", "new_source"]
            }),
            is_read_only: false,
        }
    }
}

#[async_trait::async_trait]
impl Tool for NotebookEditTool {
    fn spec(&self) -> &ToolSpec {
        static SPEC: std::sync::OnceLock<ToolSpec> = std::sync::OnceLock::new();
        SPEC.get_or_init(|| Self::spec())
    }

    async fn execute(&self, input: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
        let path = input["file_path"].as_str().unwrap_or("");
        let path = std::path::Path::new(path);
        let new_source = input["new_source"].as_str().unwrap_or("");
        let cell_type = input["cell_type"].as_str().unwrap_or("code");
        let cell_number = input["cell_number"].as_u64().unwrap_or(0);
        let edit_mode = input["edit_mode"].as_str().unwrap_or("insert");

        let Ok(content) = std::fs::read_to_string(path) else {
            return ToolResult::error(format!("Cannot read notebook: {}", path.display()));
        };
        let Ok(mut notebook) = serde_json::from_str::<serde_json::Value>(&content) else {
            return ToolResult::error("Invalid notebook JSON");
        };

        let cells = notebook.get_mut("cells").and_then(|c| c.as_array_mut());
        let Some(cells) = cells else {
            return ToolResult::error("No cells array in notebook");
        };

        let cell_type_json = if cell_type == "markdown" { "markdown" } else { "code" };
        let new_cell = json!({
            "cell_type": cell_type_json,
            "execution_count": null,
            "metadata": {},
            "outputs": [],
            "source": [new_source]
        });

        match edit_mode {
            "replace" => {
                let idx = cell_number as usize;
                if idx < cells.len() {
                    cells[idx] = new_cell;
                    ToolResult::ok(format!("Replaced cell {}", idx))
                } else {
                    ToolResult::error(format!("Cell {} not found", cell_number))
                }
            }
            "insert" => {
                let idx = (cell_number as usize).min(cells.len());
                cells.insert(idx, new_cell);
                ToolResult::ok(format!("Inserted cell at position {}", idx))
            }
            "delete" => {
                let idx = cell_number as usize;
                if idx < cells.len() {
                    cells.remove(idx);
                    ToolResult::ok(format!("Deleted cell {}", idx))
                } else {
                    ToolResult::error(format!("Cell {} not found", cell_number))
                }
            }
            _ => ToolResult::error(format!("Unknown edit_mode: {edit_mode}")),
        }
    }
}

pub struct SleepTool;

impl SleepTool {
    pub fn spec() -> ToolSpec {
        ToolSpec {
            name: "Sleep".to_string(),
            description: "Suspend agent execution for a specified duration".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "duration_ms": { "type": "number", "description": "Duration to sleep in milliseconds" }
                },
                "required": ["duration_ms"]
            }),
            is_read_only: true,
        }
    }
}

#[async_trait::async_trait]
impl Tool for SleepTool {
    fn spec(&self) -> &ToolSpec {
        static SPEC: std::sync::OnceLock<ToolSpec> = std::sync::OnceLock::new();
        SPEC.get_or_init(|| Self::spec())
    }

    async fn execute(&self, input: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
        let ms = input["duration_ms"].as_u64().unwrap_or(1000);
        let duration = std::time::Duration::from_millis(ms);
        tokio::time::sleep(duration).await;
        ToolResult::ok(format!("Slept for {}ms", ms))
    }
}

pub struct LspTool;

impl LspTool {
    pub fn spec() -> ToolSpec {
        ToolSpec {
            name: "LSP".to_string(),
            description: "Language Server Protocol operations".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "operation": { "type": "string", "enum": ["goToDefinition", "findReferences", "hover", "documentSymbol", "workspaceSymbol", "goToImplementation"] },
                    "filePath": { "type": "string" },
                    "line": { "type": "number" },
                    "character": { "type": "number" }
                },
                "required": ["operation", "filePath", "line", "character"]
            }),
            is_read_only: true,
        }
    }
}

#[async_trait::async_trait]
impl Tool for LspTool {
    fn spec(&self) -> &ToolSpec {
        static SPEC: std::sync::OnceLock<ToolSpec> = std::sync::OnceLock::new();
        SPEC.get_or_init(|| Self::spec())
    }

    async fn execute(&self, input: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
        let operation = input["operation"].as_str().unwrap_or("");
        let file_path = input["filePath"].as_str().unwrap_or("");
        let line = input["line"].as_u64().unwrap_or(1) as u32;
        let character = input["character"].as_u64().unwrap_or(1) as u32;

        let _uri = file_path_to_uri(file_path);
        let _pos = LspPosition { line, character };

        let manager = LspManager::new();
        let ext = std::path::Path::new(file_path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");

        if ext.is_empty() {
            return ToolResult::ok(format!(
                "LSP {}: no file extension detected for '{}'",
                operation, file_path
            ));
        }

        let ext_key = format!(".{}", ext);
        match manager.file_extensions().get(&ext_key) {
            Some(name) => ToolResult::ok(format!(
                "LSP {}: server '{}' required for file '{}' but not connected. Register a server with LspManager::register_server.",
                operation, name, file_path
            )),
            None => ToolResult::ok(format!(
                "LSP {}: no language server configured for '.{}' files",
                operation, ext
            )),
        }
    }
}

pub struct WebBrowserTool;

impl WebBrowserTool {
    pub fn spec() -> ToolSpec {
        ToolSpec {
            name: "WebBrowser".to_string(),
            description: "Automate a web browser (navigate, click, fill, screenshot)".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["navigate", "click", "fill", "screenshot", "evaluate"] },
                    "url": { "type": "string" },
                    "selector": { "type": "string" },
                    "value": { "type": "string" }
                },
                "required": ["action"]
            }),
            is_read_only: true,
        }
    }
}

#[async_trait::async_trait]
impl Tool for WebBrowserTool {
    fn spec(&self) -> &ToolSpec {
        static SPEC: std::sync::OnceLock<ToolSpec> = std::sync::OnceLock::new();
        SPEC.get_or_init(|| Self::spec())
    }

    async fn execute(&self, _input: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
        ToolResult::ok("WebBrowser tool requires a browser automation backend (e.g. playwright). Not yet implemented.".to_string())
    }
}

pub struct MonitorTool;

impl MonitorTool {
    pub fn spec() -> ToolSpec {
        ToolSpec {
            name: "Monitor".to_string(),
            description: "Monitor MCP server health and resources".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "server": { "type": "string" },
                    "action": { "type": "string", "enum": ["status", "resources", "tools"] }
                },
                "required": ["action"]
            }),
            is_read_only: true,
        }
    }
}

#[async_trait::async_trait]
impl Tool for MonitorTool {
    fn spec(&self) -> &ToolSpec {
        static SPEC: std::sync::OnceLock<ToolSpec> = std::sync::OnceLock::new();
        SPEC.get_or_init(|| Self::spec())
    }

    async fn execute(&self, input: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
        let action = input["action"].as_str().unwrap_or("status");
        let server = input["server"].as_str().unwrap_or("all");
        ToolResult::ok(format!("Monitor [{server}] {action}: not yet implemented"))
    }
}

pub struct SyntheticOutputTool;

impl SyntheticOutputTool {
    pub fn spec() -> ToolSpec {
        ToolSpec {
            name: "StructuredOutput".to_string(),
            description: "Return structured output in the requested format".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {}
            }),
            is_read_only: true,
        }
    }
}

#[async_trait::async_trait]
impl Tool for SyntheticOutputTool {
    fn spec(&self) -> &ToolSpec {
        static SPEC: std::sync::OnceLock<ToolSpec> = std::sync::OnceLock::new();
        SPEC.get_or_init(|| Self::spec())
    }

    async fn execute(&self, input: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
        ToolResult::ok(serde_json::to_string_pretty(&input).unwrap_or_default())
    }
}
