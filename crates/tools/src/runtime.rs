use super::ToolSpec;
use serde_json::json;

pub struct TaskCreateTool;

impl TaskCreateTool {
    pub fn spec() -> ToolSpec {
        ToolSpec {
            name: "TaskCreate".to_string(),
            description: "Create a background task".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "description": { "type": "string" },
                    "prompt": { "type": "string" },
                    "subagent_type": { "type": "string" }
                },
                "required": ["description", "prompt"]
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
            description: "List running tasks".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {}
            }),
            is_read_only: true,
        }
    }
}

pub struct TaskStopTool;

impl TaskStopTool {
    pub fn spec() -> ToolSpec {
        ToolSpec {
            name: "TaskStop".to_string(),
            description: "Stop a running task".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "task_id": { "type": "string" }
                },
                "required": ["task_id"]
            }),
            is_read_only: false,
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
                    "file_path": { "type": "string" },
                    "cell_number": { "type": "integer" },
                    "new_content": { "type": "string" },
                    "cell_type": { "type": "string", "enum": ["code", "markdown"] }
                },
                "required": ["file_path", "cell_number", "new_content"]
            }),
            is_read_only: false,
        }
    }
}

pub struct ToolSearchTool;

impl ToolSearchTool {
    pub fn spec() -> ToolSpec {
        ToolSpec {
            name: "ToolSearch".to_string(),
            description: "Search for available tools".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" }
                },
                "required": ["query"]
            }),
            is_read_only: true,
        }
    }
}

pub struct EnterPlanModeTool;

impl EnterPlanModeTool {
    pub fn spec() -> ToolSpec {
        ToolSpec {
            name: "EnterPlanMode".to_string(),
            description: "Switch to plan mode".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {}
            }),
            is_read_only: true,
        }
    }
}

pub struct ExitPlanModeTool;

impl ExitPlanModeTool {
    pub fn spec() -> ToolSpec {
        ToolSpec {
            name: "ExitPlanMode".to_string(),
            description: "Exit plan mode".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {}
            }),
            is_read_only: true,
        }
    }
}

pub struct ListMcpResourcesTool;

impl ListMcpResourcesTool {
    pub fn spec() -> ToolSpec {
        ToolSpec {
            name: "ListMcpResources".to_string(),
            description: "List available MCP resources".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {}
            }),
            is_read_only: true,
        }
    }
}

pub struct ReadMcpResourceTool;

impl ReadMcpResourceTool {
    pub fn spec() -> ToolSpec {
        ToolSpec {
            name: "ReadMcpResource".to_string(),
            description: "Read an MCP resource".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "uri": { "type": "string" }
                },
                "required": ["uri"]
            }),
            is_read_only: true,
        }
    }
}
