use claude_runtime::file_ops;
use claude_runtime::bash;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;

pub mod runtime;
pub mod tasks;

pub use tasks::{
    get_shared_task_manager, NotebookEditTool, SleepTool, LspTool, WebBrowserTool, MonitorTool,
    SyntheticOutputTool, TaskType, TaskStatus,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
    pub is_read_only: bool,
}

#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    fn spec(&self) -> &ToolSpec;

    async fn execute(&self, input: serde_json::Value, ctx: &ToolContext) -> ToolResult;
}

#[derive(Debug, Clone)]
pub struct ToolContext {
    pub cwd: PathBuf,
}

#[derive(Debug, Clone)]
pub struct ToolResult {
    pub content: String,
    pub is_error: bool,
}

impl ToolResult {
    pub fn ok(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: false,
        }
    }

    pub fn error(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: true,
        }
    }
}

impl From<anyhow::Result<String>> for ToolResult {
    fn from(result: anyhow::Result<String>) -> Self {
        match result {
            Ok(content) => Self::ok(content),
            Err(e) => Self::error(format!("{e}")),
        }
    }
}

impl From<anyhow::Result<()>> for ToolResult {
    fn from(result: anyhow::Result<()>) -> Self {
        match result {
            Ok(()) => Self::ok("Done"),
            Err(e) => Self::error(format!("{e}")),
        }
    }
}

pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    pub fn register<T: Tool + 'static>(&mut self, tool: T) {
        let spec = tool.spec().name.clone();
        self.tools.insert(spec, Box::new(tool));
    }

    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools.get(name).map(|t| t.as_ref())
    }

    pub fn list(&self) -> Vec<&ToolSpec> {
        self.tools.values().map(|t| t.spec()).collect()
    }

    pub fn all_specs_json(&self) -> Vec<serde_json::Value> {
        self.tools
            .values()
            .map(|t| {
                json!({
                    "name": t.spec().name,
                    "description": t.spec().description,
                    "input_schema": t.spec().input_schema,
                })
            })
            .collect()
    }

    pub async fn execute(
        &self,
        name: &str,
        input: serde_json::Value,
        ctx: &ToolContext,
    ) -> ToolResult {
        match self.tools.get(name) {
            Some(tool) => tool.execute(input, ctx).await,
            None => ToolResult::error(format!("Unknown tool: {name}")),
        }
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

pub fn create_default_tools(cwd: PathBuf) -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(BashTool::new(cwd.clone()));
    registry.register(FileReadTool);
    registry.register(FileWriteTool);
    registry.register(FileEditTool);
    registry.register(GlobTool);
    registry.register(GrepTool);
    registry.register(TodoWriteTool);
    registry.register(WebFetchTool);
    registry.register(WebSearchTool);
    registry.register(AgentTool);
    registry.register(SkillTool);
    registry.register(AskUserQuestionTool);
    registry.register(NotebookEditTool);
    registry.register(SleepTool);
    registry.register(LspTool);
    registry.register(EnterPlanModeTool);
    registry.register(ExitPlanModeTool);
    registry.register(ListMcpResourcesTool);
    registry.register(ReadMcpResourceTool);
    registry.register(ToolSearchTool);
    registry.register(SyntheticOutputTool);
    registry.register(MonitorTool);
    registry.register(WebBrowserTool);
    registry.register(PowerShellTool);
    registry.register(ConfigTool);
    registry.register(BriefTool);
    registry.register(CronCreateTool);
    registry.register(CronDeleteTool);
    registry.register(CronListTool);
    registry.register(McpAuthTool);
    registry.register(SendMessageTool);
    registry.register(EnterWorktreeTool);
    registry.register(ExitWorktreeTool);
    registry.register(TeamCreateTool);
    registry.register(TeamDeleteTool);
    registry
}

pub fn create_tools_with_tasks(cwd: PathBuf) -> (ToolRegistry, Arc<tasks::TaskManager>) {
    let mut registry = ToolRegistry::new();
    let task_manager = Arc::new(tasks::TaskManager::new());

    registry.register(BashTool::new(cwd.clone()));
    registry.register(FileReadTool);
    registry.register(FileWriteTool);
    registry.register(FileEditTool);
    registry.register(GlobTool);
    registry.register(GrepTool);
    registry.register(TodoWriteTool);
    registry.register(WebFetchTool);
    registry.register(WebSearchTool);
    registry.register(AgentTool);
    registry.register(SkillTool);
    registry.register(AskUserQuestionTool);
    registry.register(NotebookEditTool);
    registry.register(SleepTool);
    registry.register(LspTool);
    registry.register(EnterPlanModeTool);
    registry.register(ExitPlanModeTool);
    registry.register(ListMcpResourcesTool);
    registry.register(ReadMcpResourceTool);
    registry.register(ToolSearchTool);
    registry.register(SyntheticOutputTool);
    registry.register(MonitorTool);
    registry.register(WebBrowserTool);
    registry.register(PowerShellTool);
    registry.register(ConfigTool);
    registry.register(BriefTool);
    registry.register(CronCreateTool);
    registry.register(CronDeleteTool);
    registry.register(CronListTool);
    registry.register(McpAuthTool);
    registry.register(SendMessageTool);
    registry.register(EnterWorktreeTool);
    registry.register(ExitWorktreeTool);
    registry.register(TeamCreateTool);
    registry.register(TeamDeleteTool);
    registry.register(tasks::TaskOutputTool::new(task_manager.clone()));
    registry.register(tasks::TaskStopTool::new(task_manager.clone()));

    (registry, task_manager)
}

// ---- Tool Implementations ----

pub struct BashTool {
    cwd: PathBuf,
}

impl BashTool {
    pub fn new(cwd: PathBuf) -> Self {
        Self { cwd }
    }
}

#[async_trait::async_trait]
impl Tool for BashTool {
    fn spec(&self) -> &ToolSpec {
        static SPEC: std::sync::OnceLock<ToolSpec> = std::sync::OnceLock::new();
        SPEC.get_or_init(|| ToolSpec {
            name: "BashTool".to_string(),
            description: "Execute a shell command".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "The command to execute" },
                    "timeout": { "type": "number", "description": "Timeout in ms (default 120000)" }
                },
                "required": ["command"]
            }),
            is_read_only: false,
        })
    }

    async fn execute(&self, input: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
        let command = input["command"].as_str().unwrap_or("");
        let timeout = input["timeout"].as_u64().unwrap_or(120_000);
        let result = bash::execute_bash(&self.cwd, command, timeout).await;
        match result {
            Ok(output) => {
                let mut content = output.stdout;
                if !output.stderr.is_empty() {
                    content.push_str("\n");
                    content.push_str(&output.stderr);
                }
                ToolResult::ok(content)
            }
            Err(e) => ToolResult::error(format!("{e}")),
        }
    }
}

pub struct FileReadTool;

#[async_trait::async_trait]
impl Tool for FileReadTool {
    fn spec(&self) -> &ToolSpec {
        static SPEC: std::sync::OnceLock<ToolSpec> = std::sync::OnceLock::new();
        SPEC.get_or_init(|| ToolSpec {
            name: "ReadFile".to_string(),
            description: "Read a file from the filesystem".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "file_path": { "type": "string", "description": "Absolute path to the file" },
                    "offset": { "type": "number", "description": "Line number to start from (1-indexed)" },
                    "limit": { "type": "number", "description": "Max lines to read" }
                },
                "required": ["file_path"]
            }),
            is_read_only: true,
        })
    }

    async fn execute(&self, input: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
        let path = input["file_path"].as_str().unwrap_or("");
        let path = std::path::Path::new(path);
        let offset = input["offset"].as_u64().unwrap_or(0);
        let limit = input["limit"].as_u64().unwrap_or(2000);

        if offset > 0 {
            file_ops::read_file_lines(path, offset as u32, limit as u32).into()
        } else {
            file_ops::read_file(path).into()
        }
    }
}

pub struct FileWriteTool;

#[async_trait::async_trait]
impl Tool for FileWriteTool {
    fn spec(&self) -> &ToolSpec {
        static SPEC: std::sync::OnceLock<ToolSpec> = std::sync::OnceLock::new();
        SPEC.get_or_init(|| ToolSpec {
            name: "WriteFile".to_string(),
            description: "Write content to a file".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "file_path": { "type": "string", "description": "Absolute path" },
                    "content": { "type": "string", "description": "Content to write" }
                },
                "required": ["file_path", "content"]
            }),
            is_read_only: false,
        })
    }

    async fn execute(&self, input: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
        let path = input["file_path"].as_str().unwrap_or("");
        let content = input["content"].as_str().unwrap_or("");
        file_ops::write_file(std::path::Path::new(path), content).into()
    }
}

pub struct FileEditTool;

#[async_trait::async_trait]
impl Tool for FileEditTool {
    fn spec(&self) -> &ToolSpec {
        static SPEC: std::sync::OnceLock<ToolSpec> = std::sync::OnceLock::new();
        SPEC.get_or_init(|| ToolSpec {
            name: "FileEditTool".to_string(),
            description: "Edit a file by replacing text".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "file_path": { "type": "string", "description": "Absolute path" },
                    "old_string": { "type": "string", "description": "Text to find" },
                    "new_string": { "type": "string", "description": "Replacement text" },
                    "replace_all": { "type": "boolean", "description": "Replace all occurrences" }
                },
                "required": ["file_path", "old_string", "new_string"]
            }),
            is_read_only: false,
        })
    }

    async fn execute(&self, input: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
        let path = input["file_path"].as_str().unwrap_or("");
        let old_text = input["old_string"].as_str().unwrap_or("");
        let new_text = input["new_string"].as_str().unwrap_or("");
        let replace_all = input["replace_all"].as_bool().unwrap_or(false);

        if replace_all {
            file_ops::edit_file_all(std::path::Path::new(path), old_text, new_text).into()
        } else {
            file_ops::edit_file(std::path::Path::new(path), old_text, new_text).into()
        }
    }
}

pub struct GlobTool;

#[async_trait::async_trait]
impl Tool for GlobTool {
    fn spec(&self) -> &ToolSpec {
        static SPEC: std::sync::OnceLock<ToolSpec> = std::sync::OnceLock::new();
        SPEC.get_or_init(|| ToolSpec {
            name: "GlobTool".to_string(),
            description: "Find files matching a glob pattern".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Glob pattern" },
                    "path": { "type": "string", "description": "Directory to search in" }
                },
                "required": ["pattern"]
            }),
            is_read_only: true,
        })
    }

    async fn execute(&self, input: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let pattern = input["pattern"].as_str().unwrap_or("");
        let path = input["path"]
            .as_str()
            .map(std::path::Path::new)
            .unwrap_or(&ctx.cwd);
        match file_ops::glob_search(pattern, path) {
            Ok(entries) => ToolResult::ok(entries.join("\n")),
            Err(e) => ToolResult::error(format!("{e}")),
        }
    }
}

pub struct GrepTool;

#[async_trait::async_trait]
impl Tool for GrepTool {
    fn spec(&self) -> &ToolSpec {
        static SPEC: std::sync::OnceLock<ToolSpec> = std::sync::OnceLock::new();
        SPEC.get_or_init(|| ToolSpec {
            name: "GrepTool".to_string(),
            description: "Search file contents using regex".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Regex pattern" },
                    "path": { "type": "string", "description": "Directory to search" },
                    "include": { "type": "string", "description": "File pattern filter" }
                },
                "required": ["pattern"]
            }),
            is_read_only: true,
        })
    }

    async fn execute(&self, input: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let pattern = input["pattern"].as_str().unwrap_or("");
        let path = input["path"]
            .as_str()
            .map(std::path::Path::new)
            .unwrap_or(&ctx.cwd);
        let include = input["include"].as_str();

        match file_ops::grep_search(pattern, path, include) {
            Ok(results) => {
                let lines: Vec<String> = results
                    .iter()
                    .map(|(path, line, text)| format!("{path}:{line}: {text}"))
                    .collect();
                ToolResult::ok(lines.join("\n"))
            }
            Err(e) => ToolResult::error(format!("{e}")),
        }
    }
}

pub struct TodoWriteTool;

#[async_trait::async_trait]
impl Tool for TodoWriteTool {
    fn spec(&self) -> &ToolSpec {
        static SPEC: std::sync::OnceLock<ToolSpec> = std::sync::OnceLock::new();
        SPEC.get_or_init(|| ToolSpec {
            name: "TodoWrite".to_string(),
            description: "Create and manage a task list".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "todos": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "content": { "type": "string" },
                                "status": { "type": "string", "enum": ["pending", "in_progress", "completed"] },
                                "priority": { "type": "string", "enum": ["high", "medium", "low"] }
                            },
                            "required": ["content", "status", "priority"]
                        }
                    }
                },
                "required": ["todos"]
            }),
            is_read_only: false,
        })
    }

    async fn execute(&self, input: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
        let todos = input["todos"].as_array().cloned().unwrap_or_default();
        let mut lines = Vec::new();
        for todo in &todos {
            let content = todo["content"].as_str().unwrap_or("");
            let status = todo["status"].as_str().unwrap_or("pending");
            let priority = todo["priority"].as_str().unwrap_or("medium");
            let icon = match status {
                "completed" => "[x]",
                "in_progress" => "[>]",
                _ => "[ ]",
            };
            lines.push(format!("{} {} ({priority}): {content}", icon, status));
        }
        ToolResult::ok(lines.join("\n"))
    }
}

pub struct WebFetchTool;

#[async_trait::async_trait]
impl Tool for WebFetchTool {
    fn spec(&self) -> &ToolSpec {
        static SPEC: std::sync::OnceLock<ToolSpec> = std::sync::OnceLock::new();
        SPEC.get_or_init(|| ToolSpec {
            name: "WebFetchTool".to_string(),
            description: "Fetch content from a URL".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "URL to fetch" },
                    "format": { "type": "string", "enum": ["text", "markdown"], "description": "Output format" }
                },
                "required": ["url"]
            }),
            is_read_only: true,
        })
    }

    async fn execute(&self, input: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
        let url = input["url"].as_str().unwrap_or("");
        let client = reqwest::Client::new();
        match client.get(url).send().await {
            Ok(response) => match response.text().await {
                Ok(body) => ToolResult::ok(body),
                Err(e) => ToolResult::error(format!("Failed to read response: {e}")),
            },
            Err(e) => ToolResult::error(format!("Failed to fetch URL: {e}")),
        }
    }
}

pub struct WebSearchTool;

#[async_trait::async_trait]
impl Tool for WebSearchTool {
    fn spec(&self) -> &ToolSpec {
        static SPEC: std::sync::OnceLock<ToolSpec> = std::sync::OnceLock::new();
        SPEC.get_or_init(|| ToolSpec {
            name: "WebSearchTool".to_string(),
            description: "Search the web for information".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Search query" }
                },
                "required": ["query"]
            }),
            is_read_only: true,
        })
    }

    async fn execute(&self, input: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
        let query = input["query"].as_str().unwrap_or("");
        ToolResult::ok(format!(
            "Web search not yet implemented. Query: {query}"
        ))
    }
}

pub struct AgentTool;

#[async_trait::async_trait]
impl Tool for AgentTool {
    fn spec(&self) -> &ToolSpec {
        static SPEC: std::sync::OnceLock<ToolSpec> = std::sync::OnceLock::new();
        SPEC.get_or_init(|| ToolSpec {
            name: "Agent".to_string(),
            description: "Spawn a sub-agent for complex multi-step tasks".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "description": { "type": "string" },
                    "prompt": { "type": "string" },
                    "subagent_type": { "type": "string" }
                },
                "required": ["description", "prompt", "subagent_type"]
            }),
            is_read_only: false,
        })
    }

    async fn execute(&self, input: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
        let description = input["description"].as_str().unwrap_or("");
        let prompt = input["prompt"].as_str().unwrap_or("");
        let agent_type = input["subagent_type"].as_str().unwrap_or("general");

        let task_type = match agent_type {
            "explore" | "code" | "ask" => TaskType::LocalAgent,
            "dream" => TaskType::Dream,
            _ => TaskType::LocalBash,
        };

        let manager = get_shared_task_manager();
        let task = manager.create_task(description, prompt, task_type).await;

        let task_id = task.id.clone();
        let task_prompt = prompt.to_string();

        tokio::spawn(async move {
            let mgr = get_shared_task_manager();
            mgr.execute_task(&task_id, &task_prompt, task_type).await;
        });

        ToolResult::ok(format!(
            "Task {} created: {}\nType: {agent_type}\nUse TaskOutput to retrieve results.",
            task.id, description
        ))
    }
}

pub struct SkillTool;

#[async_trait::async_trait]
impl Tool for SkillTool {
    fn spec(&self) -> &ToolSpec {
        static SPEC: std::sync::OnceLock<ToolSpec> = std::sync::OnceLock::new();
        SPEC.get_or_init(|| ToolSpec {
            name: "Skill".to_string(),
            description: "Load and use a skill".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Skill name" }
                },
                "required": ["name"]
            }),
            is_read_only: true,
        })
    }

    async fn execute(&self, input: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
        let name = input["name"].as_str().unwrap_or("");
        ToolResult::ok(format!("Skill '{name}' not yet implemented"))
    }
}

pub struct AskUserQuestionTool;

#[async_trait::async_trait]
impl Tool for AskUserQuestionTool {
    fn spec(&self) -> &ToolSpec {
        static SPEC: std::sync::OnceLock<ToolSpec> = std::sync::OnceLock::new();
        SPEC.get_or_init(|| ToolSpec {
            name: "AskUserQuestion".to_string(),
            description: "Ask the user a question and get their response".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "questions": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "question": { "type": "string" },
                                "header": { "type": "string" }
                            },
                            "required": ["question"]
                        }
                    }
                },
                "required": ["questions"]
            }),
            is_read_only: true,
        })
    }

    async fn execute(&self, input: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
        let questions = input["questions"].as_array().cloned().unwrap_or_default();
        let lines: Vec<String> = questions
            .iter()
            .map(|q| q["question"].as_str().unwrap_or("").to_string())
            .collect();
        ToolResult::ok(format!(
            "Questions pending user response:\n{}",
            lines.join("\n")
        ))
    }
}

pub struct EnterPlanModeTool;

#[async_trait::async_trait]
impl Tool for EnterPlanModeTool {
    fn spec(&self) -> &ToolSpec {
        static SPEC: std::sync::OnceLock<ToolSpec> = std::sync::OnceLock::new();
        SPEC.get_or_init(|| ToolSpec {
            name: "EnterPlanMode".to_string(),
            description: "Switch to plan mode for complex tasks".to_string(),
            input_schema: json!({ "type": "object", "properties": {} }),
            is_read_only: true,
        })
    }
    async fn execute(&self, _input: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
        ToolResult::ok("Entered plan mode. Use ExitPlanMode when ready to implement.".to_string())
    }
}

pub struct ExitPlanModeTool;

#[async_trait::async_trait]
impl Tool for ExitPlanModeTool {
    fn spec(&self) -> &ToolSpec {
        static SPEC: std::sync::OnceLock<ToolSpec> = std::sync::OnceLock::new();
        SPEC.get_or_init(|| ToolSpec {
            name: "ExitPlanMode".to_string(),
            description: "Exit plan mode and start implementing".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "allowedPrompts": {
                        "type": "array",
                        "items": { "type": "object", "properties": { "tool": { "type": "string" }, "prompt": { "type": "string" } } }
                    }
                }
            }),
            is_read_only: false,
        })
    }
    async fn execute(&self, _input: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
        ToolResult::ok("Exited plan mode. Starting implementation.".to_string())
    }
}

pub struct ListMcpResourcesTool;

#[async_trait::async_trait]
impl Tool for ListMcpResourcesTool {
    fn spec(&self) -> &ToolSpec {
        static SPEC: std::sync::OnceLock<ToolSpec> = std::sync::OnceLock::new();
        SPEC.get_or_init(|| ToolSpec {
            name: "ListMcpResources".to_string(),
            description: "List resources from connected MCP servers".to_string(),
            input_schema: json!({ "type": "object", "properties": { "server": { "type": "string" } } }),
            is_read_only: true,
        })
    }
    async fn execute(&self, _input: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
        ToolResult::ok("No MCP servers connected. Use /mcp to configure servers.".to_string())
    }
}

pub struct ReadMcpResourceTool;

#[async_trait::async_trait]
impl Tool for ReadMcpResourceTool {
    fn spec(&self) -> &ToolSpec {
        static SPEC: std::sync::OnceLock<ToolSpec> = std::sync::OnceLock::new();
        SPEC.get_or_init(|| ToolSpec {
            name: "ReadMcpResource".to_string(),
            description: "Read a specific MCP resource by URI".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": { "server": { "type": "string" }, "uri": { "type": "string" } },
                "required": ["server", "uri"]
            }),
            is_read_only: true,
        })
    }
    async fn execute(&self, _input: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
        ToolResult::error("No MCP servers connected".to_string())
    }
}

pub struct ToolSearchTool;

#[async_trait::async_trait]
impl Tool for ToolSearchTool {
    fn spec(&self) -> &ToolSpec {
        static SPEC: std::sync::OnceLock<ToolSpec> = std::sync::OnceLock::new();
        SPEC.get_or_init(|| ToolSpec {
            name: "ToolSearch".to_string(),
            description: "Search for available tools by keyword".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": { "query": { "type": "string" }, "max_results": { "type": "number", "default": 5 } },
                "required": ["query"]
            }),
            is_read_only: true,
        })
    }
    async fn execute(&self, input: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
        let query = input["query"].as_str().unwrap_or("");
        let builtins = vec![
            "Bash", "Read", "Write", "Edit", "Glob", "Grep", "TodoWrite",
            "WebFetch", "WebSearch", "Agent", "Skill", "AskUserQuestion",
            "NotebookEdit", "Sleep", "LSP", "EnterPlanMode", "ExitPlanMode",
            "ListMcpResources", "ReadMcpResource", "ToolSearch",
            "StructuredOutput", "Monitor", "WebBrowser",
            "TaskCreate", "TaskGet", "TaskUpdate", "TaskList", "TaskOutput", "TaskStop",
            "PowerShell", "Config", "Brief",
            "CronCreate", "CronDelete", "CronList",
            "McpAuth", "SendMessage",
            "EnterWorktree", "ExitWorktree",
            "TeamCreate", "TeamDelete",
        ];
        let matches: Vec<&str> = builtins.iter().copied().filter(|t| {
            t.to_lowercase().contains(&query.to_lowercase())
        }).collect();
        if matches.is_empty() {
            ToolResult::ok(format!("No tools found matching '{query}'. Available: {}", builtins.join(", ")))
        } else {
            ToolResult::ok(format!("Found tools: {}", matches.join(", ")))
        }
    }
}

// ---- Shared State Helpers ----

fn config_store() -> &'static std::sync::Mutex<HashMap<String, String>> {
    static STORE: std::sync::OnceLock<std::sync::Mutex<HashMap<String, String>>> =
        std::sync::OnceLock::new();
    STORE.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

fn brief_mode() -> &'static std::sync::Mutex<bool> {
    static BRIEF: std::sync::OnceLock<std::sync::Mutex<bool>> = std::sync::OnceLock::new();
    BRIEF.get_or_init(|| std::sync::Mutex::new(false))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CronJob {
    id: String,
    description: String,
    prompt: String,
    schedule: String,
    enabled: bool,
    created_at: String,
}

fn cron_store() -> &'static std::sync::Mutex<Vec<CronJob>> {
    static STORE: std::sync::OnceLock<std::sync::Mutex<Vec<CronJob>>> = std::sync::OnceLock::new();
    STORE.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SentMessage {
    recipient: String,
    message: String,
    sent_at: String,
}

fn message_store() -> &'static std::sync::Mutex<Vec<SentMessage>> {
    static STORE: std::sync::OnceLock<std::sync::Mutex<Vec<SentMessage>>> = std::sync::OnceLock::new();
    STORE.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TeamMember {
    name: String,
    role: String,
    instructions: String,
}

fn team_store() -> &'static std::sync::Mutex<Vec<TeamMember>> {
    static STORE: std::sync::OnceLock<std::sync::Mutex<Vec<TeamMember>>> = std::sync::OnceLock::new();
    STORE.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

// ---- PowerShellTool ----

pub struct PowerShellTool;

#[async_trait::async_trait]
impl Tool for PowerShellTool {
    fn spec(&self) -> &ToolSpec {
        static SPEC: std::sync::OnceLock<ToolSpec> = std::sync::OnceLock::new();
        SPEC.get_or_init(|| ToolSpec {
            name: "PowerShell".to_string(),
            description: "Execute a PowerShell command on Windows".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "PowerShell command to execute" },
                    "timeout": { "type": "number", "description": "Timeout in ms (default 120000)" }
                },
                "required": ["command"]
            }),
            is_read_only: false,
        })
    }

    async fn execute(&self, input: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
        let command = input["command"].as_str().unwrap_or("");
        let timeout_ms = input["timeout"].as_u64().unwrap_or(120_000);

        let child = match tokio::process::Command::new("powershell")
            .args(["-NoProfile", "-Command", command])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => return ToolResult::error(format!("Failed to spawn PowerShell: {e}")),
        };

        let timeout = tokio::time::Duration::from_millis(timeout_ms);
        match tokio::time::timeout(timeout, child.wait_with_output()).await {
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                let mut content = stdout;
                if !stderr.is_empty() {
                    content.push_str("\n");
                    content.push_str(&stderr);
                }
                ToolResult::ok(content)
            }
            Ok(Err(e)) => ToolResult::error(format!("PowerShell error: {e}")),
            Err(_) => ToolResult::error(format!(
                "PowerShell command timed out after {timeout_ms}ms"
            )),
        }
    }
}

// ---- ConfigTool ----

pub struct ConfigTool;

#[async_trait::async_trait]
impl Tool for ConfigTool {
    fn spec(&self) -> &ToolSpec {
        static SPEC: std::sync::OnceLock<ToolSpec> = std::sync::OnceLock::new();
        SPEC.get_or_init(|| ToolSpec {
            name: "Config".to_string(),
            description: "Read/write runtime configuration".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["get", "set", "list"], "description": "Action to perform" },
                    "key": { "type": "string", "description": "Configuration key" },
                    "value": { "type": "string", "description": "Configuration value" }
                },
                "required": ["action"]
            }),
            is_read_only: false,
        })
    }

    async fn execute(&self, input: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
        let action = input["action"].as_str().unwrap_or("");
        let store = config_store();
        let mut config = store.lock().unwrap();

        match action {
            "get" => {
                let key = input["key"].as_str().unwrap_or("");
                if key.is_empty() {
                    return ToolResult::error("Key is required for get action".to_string());
                }
                match config.get(key) {
                    Some(value) => ToolResult::ok(format!("{key} = {value}")),
                    None => ToolResult::error(format!("Key '{key}' not found")),
                }
            }
            "set" => {
                let key = input["key"].as_str().unwrap_or("");
                let value = input["value"].as_str().unwrap_or("");
                if key.is_empty() {
                    return ToolResult::error("Key is required for set action".to_string());
                }
                config.insert(key.to_string(), value.to_string());
                ToolResult::ok(format!("Set {key} = {value}"))
            }
            "list" => {
                if config.is_empty() {
                    ToolResult::ok("No configuration keys set".to_string())
                } else {
                    let lines: Vec<String> = config
                        .iter()
                        .map(|(k, v)| format!("{k} = {v}"))
                        .collect();
                    ToolResult::ok(lines.join("\n"))
                }
            }
            _ => ToolResult::error(format!(
                "Unknown action: {action}. Use 'get', 'set', or 'list'"
            )),
        }
    }
}

// ---- BriefTool ----

pub struct BriefTool;

#[async_trait::async_trait]
impl Tool for BriefTool {
    fn spec(&self) -> &ToolSpec {
        static SPEC: std::sync::OnceLock<ToolSpec> = std::sync::OnceLock::new();
        SPEC.get_or_init(|| ToolSpec {
            name: "Brief".to_string(),
            description: "Toggle brief mode for concise output".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "enabled": { "type": "boolean", "description": "Enable or disable brief mode" }
                },
                "required": ["enabled"]
            }),
            is_read_only: false,
        })
    }

    async fn execute(&self, input: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
        let enabled = input["enabled"].as_bool().unwrap_or(false);
        let mode = brief_mode();
        let mut current = mode.lock().unwrap();
        *current = enabled;
        ToolResult::ok(if enabled {
            "Brief mode enabled".to_string()
        } else {
            "Brief mode disabled".to_string()
        })
    }
}

// ---- CronCreateTool ----

pub struct CronCreateTool;

#[async_trait::async_trait]
impl Tool for CronCreateTool {
    fn spec(&self) -> &ToolSpec {
        static SPEC: std::sync::OnceLock<ToolSpec> = std::sync::OnceLock::new();
        SPEC.get_or_init(|| ToolSpec {
            name: "CronCreate".to_string(),
            description: "Create a scheduled/recurring task".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "description": { "type": "string", "description": "Task description" },
                    "prompt": { "type": "string", "description": "Prompt to execute on schedule" },
                    "schedule": { "type": "string", "description": "Schedule (e.g. 'every 5m', 'hourly', 'daily')" },
                    "run_in_background": { "type": "boolean", "description": "Run in background" }
                },
                "required": ["description", "prompt", "schedule"]
            }),
            is_read_only: false,
        })
    }

    async fn execute(&self, input: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
        let description = input["description"].as_str().unwrap_or("");
        let prompt = input["prompt"].as_str().unwrap_or("");
        let schedule = input["schedule"].as_str().unwrap_or("");
        if description.is_empty() || prompt.is_empty() || schedule.is_empty() {
            return ToolResult::error(
                "description, prompt, and schedule are required".to_string(),
            );
        }

        let id = uuid::Uuid::new_v4().to_string()[..8].to_string();
        let job = CronJob {
            id: id.clone(),
            description: description.to_string(),
            prompt: prompt.to_string(),
            schedule: schedule.to_string(),
            enabled: true,
            created_at: chrono::Utc::now().to_rfc3339(),
        };

        let store = cron_store();
        let mut jobs = store.lock().unwrap();
        jobs.push(job);

        ToolResult::ok(format!(
            "Created cron job '{id}': {description}\nSchedule: {schedule}\nPrompt: {prompt}"
        ))
    }
}

// ---- CronDeleteTool ----

pub struct CronDeleteTool;

#[async_trait::async_trait]
impl Tool for CronDeleteTool {
    fn spec(&self) -> &ToolSpec {
        static SPEC: std::sync::OnceLock<ToolSpec> = std::sync::OnceLock::new();
        SPEC.get_or_init(|| ToolSpec {
            name: "CronDelete".to_string(),
            description: "Delete a scheduled task".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Cron job ID to delete" }
                },
                "required": ["id"]
            }),
            is_read_only: false,
        })
    }

    async fn execute(&self, input: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
        let id = input["id"].as_str().unwrap_or("");
        if id.is_empty() {
            return ToolResult::error("id is required".to_string());
        }

        let store = cron_store();
        let mut jobs = store.lock().unwrap();
        let before = jobs.len();
        jobs.retain(|j| j.id != id);
        let removed = before - jobs.len();

        if removed > 0 {
            ToolResult::ok(format!("Deleted cron job '{id}'"))
        } else {
            ToolResult::error(format!("Cron job '{id}' not found"))
        }
    }
}

// ---- CronListTool ----

pub struct CronListTool;

#[async_trait::async_trait]
impl Tool for CronListTool {
    fn spec(&self) -> &ToolSpec {
        static SPEC: std::sync::OnceLock<ToolSpec> = std::sync::OnceLock::new();
        SPEC.get_or_init(|| ToolSpec {
            name: "CronList".to_string(),
            description: "List all scheduled tasks".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {}
            }),
            is_read_only: true,
        })
    }

    async fn execute(&self, _input: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
        let store = cron_store();
        let jobs = store.lock().unwrap();

        if jobs.is_empty() {
            return ToolResult::ok("No scheduled tasks".to_string());
        }

        let lines: Vec<String> = jobs
            .iter()
            .map(|j| {
                let status = if j.enabled { "enabled" } else { "disabled" };
                format!(
                    "[{}] {} ({}) - {} [{}]",
                    j.id, j.description, j.schedule, status, j.created_at
                )
            })
            .collect();
        ToolResult::ok(format!(
            "Scheduled tasks ({}):\n{}",
            jobs.len(),
            lines.join("\n")
        ))
    }
}

// ---- McpAuthTool ----

pub struct McpAuthTool;

#[async_trait::async_trait]
impl Tool for McpAuthTool {
    fn spec(&self) -> &ToolSpec {
        static SPEC: std::sync::OnceLock<ToolSpec> = std::sync::OnceLock::new();
        SPEC.get_or_init(|| ToolSpec {
            name: "McpAuth".to_string(),
            description: "Authenticate with an MCP server".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "server": { "type": "string", "description": "MCP server name" },
                    "action": { "type": "string", "enum": ["login", "logout", "status"], "description": "Auth action" }
                },
                "required": ["server", "action"]
            }),
            is_read_only: false,
        })
    }

    async fn execute(&self, input: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
        let server = input["server"].as_str().unwrap_or("");
        let action = input["action"].as_str().unwrap_or("");

        match action {
            "login" => ToolResult::ok(format!(
                "Authentication initiated for MCP server '{server}'. OAuth flow not yet implemented."
            )),
            "logout" => ToolResult::ok(format!(
                "Logged out from MCP server '{server}'."
            )),
            "status" => ToolResult::ok(format!(
                "Auth status for '{server}': not authenticated. MCP auth is not yet fully implemented."
            )),
            _ => ToolResult::error(format!(
                "Unknown action: {action}. Use 'login', 'logout', or 'status'"
            )),
        }
    }
}

// ---- SendMessageTool ----

pub struct SendMessageTool;

#[async_trait::async_trait]
impl Tool for SendMessageTool {
    fn spec(&self) -> &ToolSpec {
        static SPEC: std::sync::OnceLock<ToolSpec> = std::sync::OnceLock::new();
        SPEC.get_or_init(|| ToolSpec {
            name: "SendMessage".to_string(),
            description: "Send a message to a team member or agent".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "recipient": { "type": "string", "description": "Message recipient" },
                    "message": { "type": "string", "description": "Message content" }
                },
                "required": ["recipient", "message"]
            }),
            is_read_only: false,
        })
    }

    async fn execute(&self, input: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
        let recipient = input["recipient"].as_str().unwrap_or("");
        let message = input["message"].as_str().unwrap_or("");

        if recipient.is_empty() || message.is_empty() {
            return ToolResult::error("recipient and message are required".to_string());
        }

        let msg = SentMessage {
            recipient: recipient.to_string(),
            message: message.to_string(),
            sent_at: chrono::Utc::now().to_rfc3339(),
        };

        let store = message_store();
        let mut messages = store.lock().unwrap();
        messages.push(msg);

        ToolResult::ok(format!("Message sent to '{recipient}'"))
    }
}

// ---- EnterWorktreeTool ----

pub struct EnterWorktreeTool;

#[async_trait::async_trait]
impl Tool for EnterWorktreeTool {
    fn spec(&self) -> &ToolSpec {
        static SPEC: std::sync::OnceLock<ToolSpec> = std::sync::OnceLock::new();
        SPEC.get_or_init(|| ToolSpec {
            name: "EnterWorktree".to_string(),
            description: "Enter or create a git worktree".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Worktree path (optional; lists current worktrees if omitted)" }
                },
                "required": []
            }),
            is_read_only: false,
        })
    }

    async fn execute(&self, input: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let path = input["path"].as_str();

        let (cmd, timeout) = if let Some(path) = path {
            (format!("git worktree add {path}"), 30_000)
        } else {
            ("git worktree list".to_string(), 10_000)
        };

        match bash::execute_bash(&ctx.cwd, &cmd, timeout).await {
            Ok(output) => {
                let mut content = output.stdout;
                if !output.stderr.is_empty() {
                    content.push_str("\n");
                    content.push_str(&output.stderr);
                }
                ToolResult::ok(content)
            }
            Err(e) => ToolResult::error(format!("{e}")),
        }
    }
}

// ---- ExitWorktreeTool ----

pub struct ExitWorktreeTool;

#[async_trait::async_trait]
impl Tool for ExitWorktreeTool {
    fn spec(&self) -> &ToolSpec {
        static SPEC: std::sync::OnceLock<ToolSpec> = std::sync::OnceLock::new();
        SPEC.get_or_init(|| ToolSpec {
            name: "ExitWorktree".to_string(),
            description: "Exit the current git worktree".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {}
            }),
            is_read_only: false,
        })
    }

    async fn execute(&self, _input: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        match bash::execute_bash(&ctx.cwd, "git worktree list", 10_000).await {
            Ok(list_output) => {
                let worktree_count = list_output
                    .stdout
                    .lines()
                    .filter(|l| !l.trim().is_empty())
                    .count();

                if worktree_count <= 1 {
                    return ToolResult::ok("No additional worktrees to exit.".to_string());
                }

                ToolResult::ok(format!(
                    "Current worktrees:\n{}\n\nTo remove a worktree, cd to a different worktree first, then run:\n  git worktree remove <path>",
                    list_output.stdout.trim()
                ))
            }
            Err(e) => ToolResult::error(format!("{e}")),
        }
    }
}

// ---- TeamCreateTool ----

pub struct TeamCreateTool;

#[async_trait::async_trait]
impl Tool for TeamCreateTool {
    fn spec(&self) -> &ToolSpec {
        static SPEC: std::sync::OnceLock<ToolSpec> = std::sync::OnceLock::new();
        SPEC.get_or_init(|| ToolSpec {
            name: "TeamCreate".to_string(),
            description: "Create a team member configuration".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Team member name" },
                    "role": { "type": "string", "description": "Team member role" },
                    "instructions": { "type": "string", "description": "Instructions for the team member" }
                },
                "required": ["name", "role", "instructions"]
            }),
            is_read_only: false,
        })
    }

    async fn execute(&self, input: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
        let name = input["name"].as_str().unwrap_or("");
        let role = input["role"].as_str().unwrap_or("");
        let instructions = input["instructions"].as_str().unwrap_or("");

        if name.is_empty() || role.is_empty() || instructions.is_empty() {
            return ToolResult::error(
                "name, role, and instructions are required".to_string(),
            );
        }

        let store = team_store();
        let mut members = store.lock().unwrap();

        if members.iter().any(|m| m.name == name) {
            return ToolResult::error(format!("Team member '{name}' already exists"));
        }

        members.push(TeamMember {
            name: name.to_string(),
            role: role.to_string(),
            instructions: instructions.to_string(),
        });

        ToolResult::ok(format!("Created team member '{name}' with role '{role}'"))
    }
}

// ---- TeamDeleteTool ----

pub struct TeamDeleteTool;

#[async_trait::async_trait]
impl Tool for TeamDeleteTool {
    fn spec(&self) -> &ToolSpec {
        static SPEC: std::sync::OnceLock<ToolSpec> = std::sync::OnceLock::new();
        SPEC.get_or_init(|| ToolSpec {
            name: "TeamDelete".to_string(),
            description: "Delete a team member".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Team member name to delete" }
                },
                "required": ["name"]
            }),
            is_read_only: false,
        })
    }

    async fn execute(&self, input: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
        let name = input["name"].as_str().unwrap_or("");
        if name.is_empty() {
            return ToolResult::error("name is required".to_string());
        }

        let store = team_store();
        let mut members = store.lock().unwrap();
        let before = members.len();
        members.retain(|m| m.name != name);
        let removed = before - members.len();

        if removed > 0 {
            ToolResult::ok(format!("Deleted team member '{name}'"))
        } else {
            ToolResult::error(format!("Team member '{name}' not found"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockTool;

    #[async_trait::async_trait]
    impl Tool for MockTool {
        fn spec(&self) -> &ToolSpec {
            static SPEC: std::sync::OnceLock<ToolSpec> = std::sync::OnceLock::new();
            SPEC.get_or_init(|| ToolSpec {
                name: "MockTool".to_string(),
                description: "A mock tool for testing".to_string(),
                input_schema: json!({"type": "object", "properties": {}}),
                is_read_only: true,
            })
        }

        async fn execute(&self, _input: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
            ToolResult::ok("mock result")
        }
    }

    #[tokio::test]
    async fn test_tool_registry_register_and_get() {
        let mut registry = ToolRegistry::new();
        registry.register(MockTool);

        let tool = registry.get("MockTool");
        assert!(tool.is_some());
        assert_eq!(tool.unwrap().spec().name, "MockTool");

        let specs = registry.list();
        assert_eq!(specs.len(), 1);
    }

    #[tokio::test]
    async fn test_tool_registry_execute_unknown() {
        let registry = ToolRegistry::new();
        let ctx = ToolContext {
            cwd: std::path::PathBuf::from("/tmp"),
        };
        let result = registry.execute("NonexistentTool", json!({}), &ctx).await;
        assert!(result.is_error);
        assert!(result.content.contains("Unknown tool"));
    }

    #[test]
    fn test_tool_result_ok_and_error() {
        let ok = ToolResult::ok("success");
        assert!(!ok.is_error);
        assert_eq!(ok.content, "success");

        let err = ToolResult::error("something went wrong");
        assert!(err.is_error);
        assert_eq!(err.content, "something went wrong");
    }

    #[tokio::test]
    async fn test_tool_search() {
        let mut registry = ToolRegistry::new();
        registry.register(ToolSearchTool);

        let ctx = ToolContext {
            cwd: std::path::PathBuf::from("/tmp"),
        };
        let result = registry
            .execute("ToolSearch", json!({"query": "glob"}), &ctx)
            .await;
        assert!(!result.is_error);
        assert!(result.content.contains("Glob"));
    }

    #[tokio::test]
    async fn test_todo_write() {
        let mut registry = ToolRegistry::new();
        registry.register(TodoWriteTool);

        let ctx = ToolContext {
            cwd: std::path::PathBuf::from("/tmp"),
        };
        let input = json!({
            "todos": [
                {"content": "Write tests", "status": "pending", "priority": "high"},
                {"content": "Fix bug", "status": "in_progress", "priority": "medium"},
                {"content": "Deploy", "status": "completed", "priority": "low"}
            ]
        });
        let result = registry.execute("TodoWrite", input, &ctx).await;
        assert!(!result.is_error);
        assert!(result.content.contains("Write tests"));
        assert!(result.content.contains("high"));
        assert!(result.content.contains("[x]"));
        assert!(result.content.contains("[>]"));
        assert!(result.content.contains("[ ]"));
    }
}
