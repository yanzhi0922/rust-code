//! Built-in tool specifications (schemas for all 40+ tools).

use serde_json::json;

use super::ToolSpec;
use super::tool_prompts;

/// Build the Bash tool input schema, dynamically omitting `run_in_background`
/// when the `CLAUDE_CODE_DISABLE_BACKGROUND_TASKS` environment variable is set.
#[must_use]
fn bash_tool_schema() -> serde_json::Value {
    let max_timeout = std::env::var("CLAUDE_BASH_MAX_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(600_000);

    let mut properties = serde_json::Map::from_iter([
        (
            "command".to_owned(),
            json!({"type": "string", "description": "The command to execute"}),
        ),
        (
            "timeout".to_owned(),
            json!({"type": "number", "description": format!("Optional timeout in milliseconds (max {})", max_timeout)}),
        ),
        (
            "description".to_owned(),
            json!({"type": "string", "description": "Clear, concise description of what this command does in active voice. Never use words like \"complex\" or \"risk\" in the description - just describe what it does.\n\nFor simple commands (git, npm, standard CLI tools), keep it brief (5-10 words):\n- ls -> \"List files in current directory\"\n- git status -> \"Show working tree status\"\n- npm install -> \"Install package dependencies\"\n\nFor commands that are harder to parse at a glance (piped commands, obscure flags, etc.), add enough context to clarify what it does:\n- find . -name \"*.tmp\" -exec rm {} \\; -> \"Find and delete all .tmp files recursively\"\n- git reset --hard origin/main -> \"Discard all local changes and match remote main\"\n- curl -s url | jq '.data[]' -> \"Fetch JSON from URL and extract data array elements\""}),
        ),
        (
            "dangerouslyDisableSandbox".to_owned(),
            json!({"type": "boolean", "description": "Set this to true to dangerously override sandbox mode and run commands without sandboxing."}),
        ),
    ]);

    if std::env::var("CLAUDE_CODE_DISABLE_BACKGROUND_TASKS").is_err() {
        properties.insert(
            "run_in_background".to_owned(),
            json!({"type": "boolean", "description": "Set to true to run this command in the background. Use Read to read the output later."}),
        );
    }

    json!({
        "type": "object",
        "properties": properties,
        "required": ["command"],
        "additionalProperties": false,
    })
}

#[must_use]
fn builtin_tool_specs_core() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "list_directory".to_owned(),
            protocol_name: "ListDirectory".to_owned(),
            permission_tool_name: "Read".to_owned(),
            description: tool_prompts::LIST_DIRECTORY.to_owned(),
            requires_permission: false,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "recursive": {"type": "boolean"},
                    "max_entries": {"type": "integer", "minimum": 1, "maximum": 500}
                },
                "additionalProperties": false,
            }),
        },
        ToolSpec {
            name: "read_file".to_owned(),
            protocol_name: "Read".to_owned(),
            permission_tool_name: "Read".to_owned(),
            description: tool_prompts::file_read_tool_prompt(),
            requires_permission: false,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "file_path": {"type": "string", "description": "The absolute path to the file to read"},
                    "offset": {"type": "integer", "minimum": 0, "description": "The line number to start reading from. Only provide if the file is too large to read at once"},
                    "limit": {"type": "integer", "minimum": 1, "description": "The number of lines to read. Only provide if the file is too large to read at once."},
                    "pages": {"type": "string", "description": "Page range for PDF files (e.g., \"1-5\", \"3\", \"10-20\"). Only applicable to PDF files. Maximum 20 pages per request."}
                },
                "required": ["file_path"],
                "additionalProperties": false,
            }),
        },
        ToolSpec {
            name: "search_text".to_owned(),
            protocol_name: "SearchText".to_owned(),
            permission_tool_name: "Read".to_owned(),
            description: tool_prompts::SEARCH_TEXT.to_owned(),
            requires_permission: false,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string"},
                    "path": {"type": "string"},
                    "max_matches": {"type": "integer", "minimum": 1, "maximum": 200}
                },
                "required": ["pattern"],
                "additionalProperties": false,
            }),
        },
        ToolSpec {
            name: "write_file".to_owned(),
            protocol_name: "Write".to_owned(),
            permission_tool_name: "Edit".to_owned(),
            description: tool_prompts::file_write_tool_prompt(),
            requires_permission: true,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "file_path": {"type": "string", "description": "The absolute path to the file to write (must be absolute, not relative)"},
                    "content": {"type": "string", "description": "The content to write to the file"}
                },
                "required": ["file_path", "content"],
                "additionalProperties": false,
            }),
        },
        ToolSpec {
            name: "replace_in_file".to_owned(),
            protocol_name: "ReplaceInFile".to_owned(),
            permission_tool_name: "Edit".to_owned(),
            description: tool_prompts::REPLACE_IN_FILE.to_owned(),
            requires_permission: true,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "file_path": {"type": "string"},
                    "search": {"type": "string"},
                    "replace": {"type": "string"},
                    "all": {"type": "boolean"}
                },
                "required": ["file_path", "search", "replace"],
                "additionalProperties": false,
            }),
        },
        ToolSpec {
            name: "edit_file".to_owned(),
            protocol_name: "Edit".to_owned(),
            permission_tool_name: "Edit".to_owned(),
            description: tool_prompts::file_edit_tool_prompt(),
            requires_permission: true,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "file_path": {"type": "string", "description": "The absolute path to the file to modify"},
                    "old_string": {"type": "string", "description": "The text to replace"},
                    "new_string": {"type": "string", "description": "The text to replace it with (must be different from old_string)"},
                    "replace_all": {"type": "boolean", "description": "Replace all occurrences of old_string (default false)"}
                },
                "required": ["file_path", "old_string", "new_string"],
                "additionalProperties": false,
            }),
        },
        ToolSpec {
            name: "bash_command".to_owned(),
            protocol_name: "Bash".to_owned(),
            permission_tool_name: "Bash".to_owned(),
            description: tool_prompts::bash_tool_prompt(),
            requires_permission: true,
            input_schema: bash_tool_schema(),
        },
        ToolSpec {
            name: "glob".to_owned(),
            protocol_name: "Glob".to_owned(),
            permission_tool_name: "Read".to_owned(),
            description: tool_prompts::GLOB.to_owned(),
            requires_permission: false,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "description": "The glob pattern to match files against"},
                    "path": {"type": "string", "description": "The directory to search in. If not specified, the current working directory will be used. IMPORTANT: Omit this field to use the default directory. DO NOT enter \"undefined\" or \"null\" - simply omit it for the default behavior. Must be a valid directory path if provided."}
                },
                "required": ["pattern"],
                "additionalProperties": false,
            }),
        },
        ToolSpec {
            name: "grep".to_owned(),
            protocol_name: "Grep".to_owned(),
            permission_tool_name: "Read".to_owned(),
            description: tool_prompts::grep_tool_prompt(),
            requires_permission: false,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "description": "The regular expression pattern to search for in the contents of files."},
                    "path": {"type": "string", "description": "File or directory to search in (rg PATH). Defaults to current working directory."},
                    "glob": {"type": "string", "description": "Glob pattern to filter files (e.g. \"*.js\", \"*.{ts,tsx}\") - maps to rg --glob"},
                    "output_mode": {"type": "string", "enum": ["content", "files_with_matches", "count"], "description": "Output mode: \"content\" shows matching lines (supports -A/-B/-C context, -n line numbers, head_limit), \"files_with_matches\" shows file paths (supports head_limit), \"count\" shows match counts (supports head_limit). Defaults to \"files_with_matches\"."},
                    "-A": {"type": "integer", "description": "Number of lines to show after each match (rg -A). Requires output_mode: \"content\", ignored otherwise."},
                    "-B": {"type": "integer", "description": "Number of lines to show before each match (rg -B). Requires output_mode: \"content\", ignored otherwise."},
                    "-C": {"type": "integer", "description": "Alias for context."},
                    "context": {"type": "integer", "description": "Number of lines to show before and after each match (rg -C). Requires output_mode: \"content\", ignored otherwise."},
                    "-i": {"type": "boolean", "description": "Case insensitive search (rg -i)"},
                    "-n": {"type": "boolean", "description": "Show line numbers in output (rg -n). Requires output_mode: \"content\", ignored otherwise. Defaults to true."},
                    "type": {"type": "string", "description": "File type to search (rg --type). Common types: js, py, rust, go, java, etc. More efficient than include for standard file types."},
                    "multiline": {"type": "boolean", "description": "Enable multiline mode where . matches newlines and patterns can span lines (rg -U --multiline-dotall). Default: false."},
                    "head_limit": {"type": "integer", "description": "Limit output to first N lines/entries, equivalent to \"| head -N\". Works across all output modes: content (limits output lines), files_with_matches (limits file paths), count (limits count entries). Defaults to 250 when unspecified. Pass 0 for unlimited (use sparingly \u{2014} large result sets waste context)."},
                    "offset": {"type": "integer", "description": "Skip first N lines/entries before applying head_limit, equivalent to \"| tail -n +N | head -N\". Works across all output modes. Defaults to 0."}
                },
                "required": ["pattern"],
                "additionalProperties": false,
            }),
        },
        ToolSpec {
            name: "web_fetch".to_owned(),
            protocol_name: "WebFetch".to_owned(),
            permission_tool_name: "WebFetch".to_owned(),
            description: tool_prompts::web_fetch_tool_prompt(),
            requires_permission: true,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "url": {"type": "string", "format": "uri", "description": "The URL to fetch content from."},
                    "prompt": {"type": "string", "description": "The prompt to run on the fetched content"}
                },
                "required": ["url", "prompt"],
                "additionalProperties": false,
            }),
        },
        ToolSpec {
            name: "ask_user".to_owned(),
            protocol_name: "AskUserQuestion".to_owned(),
            permission_tool_name: "AskUserQuestion".to_owned(),
            description: tool_prompts::ASK_USER.to_owned(),
            requires_permission: false,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "questions": {
                        "type": "array",
                        "minItems": 1,
                        "maxItems": 4,
                        "description": "Questions to ask the user (1-4 questions)",
                        "items": {
                            "type": "object",
                            "properties": {
                                "question": {
                                    "type": "string",
                                    "description": "The complete question to ask the user. Should be clear, specific, and end with a question mark."
                                },
                                "header": {
                                    "type": "string",
                                    "maxLength": 12,
                                    "description": "Very short label displayed as a chip/tag (max 12 chars)."
                                },
                                "options": {
                                    "type": "array",
                                    "minItems": 2,
                                    "maxItems": 4,
                                    "description": "The available choices for this question. Do not include an Other option; it is provided automatically.",
                                    "items": {
                                        "type": "object",
                                        "properties": {
                                            "label": {
                                                "type": "string",
                                                "description": "The display text for this option that the user will see and select. Should be concise (1-5 words)."
                                            },
                                            "description": {
                                                "type": "string",
                                                "description": "Explanation of what this option means or what will happen if chosen."
                                            },
                                            "preview": {
                                                "type": "string",
                                                "description": "Optional preview content rendered when this option is focused. Only use for single-select questions."
                                            }
                                        },
                                        "required": ["label", "description"],
                                        "additionalProperties": false
                                    }
                                },
                                "multiSelect": {
                                    "type": "boolean",
                                    "default": false,
                                    "description": "Set to true to allow the user to select multiple options instead of just one."
                                }
                            },
                            "required": ["question", "header", "options"],
                            "additionalProperties": false
                        }
                    },
                    "answers": {
                        "type": "object",
                        "additionalProperties": {"type": "string"},
                        "description": "User answers collected by the permission component"
                    },
                    "annotations": {
                        "type": "object",
                        "description": "Optional per-question annotations from the user, keyed by question text."
                    },
                    "metadata": {
                        "type": "object",
                        "properties": {
                            "source": {"type": "string"}
                        },
                        "additionalProperties": true
                    }
                },
                "required": ["questions"],
                "additionalProperties": false,
            }),
        },
        ToolSpec {
            name: "todo_write".to_owned(),
            protocol_name: "TodoWrite".to_owned(),
            permission_tool_name: "TodoWrite".to_owned(),
            description: tool_prompts::todo_write_tool_prompt(),
            requires_permission: false,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "todos": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "content": {"type": "string", "description": "Brief description of the task."},
                                "status": {"type": "string", "enum": ["pending", "in_progress", "completed"]},
                                "activeForm": {"type": "string", "description": "Present continuous form of the task description (e.g., 'Fixing authentication bug'). Shown during in_progress status. Must be at least 1 character."}
                            },
                            "required": ["content", "status", "activeForm"],
                            "additionalProperties": false
                        }
                    }
                },
                "required": ["todos"],
                "additionalProperties": false,
            }),
        },
        ToolSpec {
            name: "config_read".to_owned(),
            protocol_name: "Config".to_owned(),
            permission_tool_name: "Config".to_owned(),
            description: tool_prompts::CONFIG_READ.to_owned(),
            requires_permission: false,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "action": {"type": "string", "enum": ["get", "set"]},
                    "key": {"type": "string"},
                    "value": {}
                },
                "required": ["action", "key"],
                "additionalProperties": false,
            }),
        },
        ToolSpec {
            name: "agent".to_owned(),
            protocol_name: "Agent".to_owned(),
            permission_tool_name: "Agent".to_owned(),
            description: tool_prompts::agent_tool_prompt(),
            requires_permission: true,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "description": {"type": "string", "description": "A short (3-5 word) description of the task."},
                    "prompt": {"type": "string", "description": "The task for the agent to perform."},
                    "subagent_type": {"type": "string", "description": "The type of specialized agent to use for this task. If omitted and fork mode is enabled, this forks the current agent and inherits conversation context."},
                    "model": {"type": "string", "description": "Optional model override for this agent. Omit it or use inherit to reuse the parent model. Implicit forks inherit the parent model."},
                    "run_in_background": {"type": "boolean", "description": "Set to true to run this agent in the background. Fork mode may force background execution automatically."},
                    "name": {"type": "string", "description": "Name for the spawned agent. Makes it addressable via SendMessage({to: name}) while running."},
                    "team_name": {"type": "string", "description": "Team name for spawning. Uses the current team context if omitted."},
                    "mode": {"type": "string", "enum": ["default", "plan"], "description": "Permission mode for the spawned teammate."},
                    "cwd": {"type": "string", "description": "Absolute path to run the agent in. Overrides the working directory for all filesystem and shell operations within this agent."}
                },
                "required": ["description", "prompt"],
                "additionalProperties": false,
            }),
        },
        ToolSpec {
            name: "web_search".to_owned(),
            protocol_name: "WebSearch".to_owned(),
            permission_tool_name: "WebSearch".to_owned(),
            description: tool_prompts::web_search_tool_prompt(),
            requires_permission: true,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string", "minLength": 2, "description": "The search query to use"},
                    "allowed_domains": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Only include search results from these domains"
                    },
                    "blocked_domains": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Never include search results from these domains"
                    }
                },
                "required": ["query"],
                "additionalProperties": false,
            }),
        },
        // ── LSP tool ───────────────────────────────────────────────────────
        ToolSpec {
            name: "lsp".to_owned(),
            protocol_name: "LSP".to_owned(),
            permission_tool_name: "LSP".to_owned(),
            description: tool_prompts::LSP.to_owned(),
            requires_permission: false,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "operation": {"type": "string", "enum": [
                        "goToDefinition",
                        "findReferences",
                        "hover",
                        "documentSymbol",
                        "workspaceSymbol",
                        "goToImplementation",
                        "prepareCallHierarchy",
                        "incomingCalls",
                        "outgoingCalls"
                    ]},
                    "filePath": {"type": "string", "description": "The absolute or relative path to the file"},
                    "line": {"type": "integer", "minimum": 1, "description": "The line number (1-based, as shown in editors)"},
                    "character": {"type": "integer", "minimum": 1, "description": "The character offset (1-based, as shown in editors)"}
                },
                "required": ["operation", "filePath", "line", "character"],
                "additionalProperties": false,
            }),
        },
        // ── Shared task-list tools ────────────────────────────────────────
        ToolSpec {
            name: "task_create".to_owned(),
            protocol_name: "TaskCreate".to_owned(),
            permission_tool_name: "TaskCreate".to_owned(),
            description: tool_prompts::TASK_CREATE.to_owned(),
            requires_permission: false,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "subject": {"type": "string", "description": "A brief title for the task"},
                    "description": {"type": "string", "description": "What needs to be done"},
                    "activeForm": {"type": "string", "description": "Present continuous form shown in spinner when in_progress (e.g., \"Running tests\")"},
                    "metadata": {"type": "object", "description": "Arbitrary metadata to attach to the task"}
                },
                "required": ["subject", "description"],
                "additionalProperties": false,
            }),
        },
        ToolSpec {
            name: "task_get".to_owned(),
            protocol_name: "TaskGet".to_owned(),
            permission_tool_name: "TaskGet".to_owned(),
            description: tool_prompts::TASK_GET.to_owned(),
            requires_permission: false,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "taskId": {"type": "string", "description": "The ID of the task to retrieve"}
                },
                "required": ["taskId"],
                "additionalProperties": false,
            }),
        },
        ToolSpec {
            name: "task_list".to_owned(),
            protocol_name: "TaskList".to_owned(),
            permission_tool_name: "TaskList".to_owned(),
            description: tool_prompts::TASK_LIST.to_owned(),
            requires_permission: false,
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false,
            }),
        },
        ToolSpec {
            name: "task_update".to_owned(),
            protocol_name: "TaskUpdate".to_owned(),
            permission_tool_name: "TaskUpdate".to_owned(),
            description: tool_prompts::TASK_UPDATE.to_owned(),
            requires_permission: false,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "taskId": {"type": "string", "description": "The ID of the task to update"},
                    "subject": {"type": "string", "description": "New subject for the task"},
                    "description": {"type": "string", "description": "New description for the task"},
                    "activeForm": {"type": "string", "description": "Present continuous form shown in spinner when in_progress (e.g., \"Running tests\")"},
                    "status": {"type": "string", "enum": ["pending", "in_progress", "completed", "deleted"], "description": "New status for the task"},
                    "addBlocks": {"type": "array", "items": {"type": "string"}, "description": "Task IDs that this task blocks"},
                    "addBlockedBy": {"type": "array", "items": {"type": "string"}, "description": "Task IDs that block this task"},
                    "owner": {"type": "string", "description": "New owner for the task"},
                    "metadata": {"type": "object", "description": "Metadata keys to merge into the task. Set a key to null to delete it."}
                },
                "required": ["taskId"],
                "additionalProperties": false,
            }),
        },
        ToolSpec {
            name: "task_output".to_owned(),
            protocol_name: "TaskOutput".to_owned(),
            permission_tool_name: "TaskOutput".to_owned(),
            description: tool_prompts::TASK_OUTPUT.to_owned(),
            requires_permission: false,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "task_id": {"type": "string", "description": "The ID of the task to get output from"},
                    "block": {"type": "boolean", "description": "Whether to wait for task completion (default true)"},
                    "timeout": {"type": "number", "description": "Max wait time in ms (default 30000)"}
                },
                "required": ["task_id"],
                "additionalProperties": false,
            }),
        },
        ToolSpec {
            name: "task_stop".to_owned(),
            protocol_name: "TaskStop".to_owned(),
            permission_tool_name: "TaskStop".to_owned(),
            description: tool_prompts::TASK_STOP.to_owned(),
            requires_permission: true,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "task_id": {"type": "string", "description": "The ID of the task to stop"}
                },
                "required": ["task_id"],
                "additionalProperties": false,
            }),
        },
        // ── Notebook edit tool ─────────────────────────────────────────────
        ToolSpec {
            name: "notebook_edit".to_owned(),
            protocol_name: "NotebookEdit".to_owned(),
            permission_tool_name: "Edit".to_owned(),
            description: tool_prompts::NOTEBOOK_EDIT.to_owned(),
            requires_permission: true,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "notebook_path": {"type": "string", "description": "The absolute path to the Jupyter notebook file to edit (must be absolute, not relative)"},
                    "cell_id": {"type": "string", "description": "The ID of the cell to edit"},
                    "new_source": {"type": "string", "description": "The new source for the cell"},
                    "cell_type": {"type": "string", "enum": ["code", "markdown"], "description": "The type of the cell (code or markdown). If not specified, it defaults to the current cell type. If using edit_mode=insert, this is required."},
                    "edit_mode": {"type": "string", "enum": ["replace", "insert", "delete"], "description": "The type of edit to make (replace, insert, delete). Defaults to replace."}
                },
                "required": ["notebook_path", "new_source"],
                "additionalProperties": false,
            }),
        },
        ToolSpec {
            name: "skill_execute".to_owned(),
            protocol_name: "Skill".to_owned(),
            permission_tool_name: "Skill".to_owned(),
            description: tool_prompts::SKILL_EXECUTE.to_owned(),
            requires_permission: false,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "skill": {"type": "string", "description": "The skill name. E.g., \"commit\", \"review-pr\", or \"pdf\""},
                    "args": {"type": "string", "description": "Optional arguments for the skill"}
                },
                "required": ["skill"],
                "additionalProperties": false,
            }),
        },
        // ── Send message tool ──────────────────────────────────────────────
        ToolSpec {
            name: "send_message".to_owned(),
            protocol_name: "SendMessage".to_owned(),
            permission_tool_name: "SendMessage".to_owned(),
            description: tool_prompts::send_message_tool_prompt(),
            requires_permission: false,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "to": {"type": "string", "description": "Recipient: teammate name, or \"*\" for broadcast to all teammates"},
                    "summary": {"type": "string", "description": "A 5-10 word summary shown as a preview in the UI (required when message is a string)"},
                    "message": {
                        "oneOf": [
                            {"type": "string", "description": "Plain text message content"},
                            {
                                "type": "object",
                                "oneOf": [
                                    {
                                        "type": "object",
                                        "properties": {
                                            "type": {"const": "shutdown_request"},
                                            "reason": {"type": "string"}
                                        },
                                        "required": ["type"],
                                        "additionalProperties": false
                                    },
                                    {
                                        "type": "object",
                                        "properties": {
                                            "type": {"const": "shutdown_response"},
                                            "request_id": {"type": "string"},
                                            "approve": {"type": "boolean"},
                                            "reason": {"type": "string"}
                                        },
                                        "required": ["type", "request_id", "approve"],
                                        "additionalProperties": false
                                    },
                                    {
                                        "type": "object",
                                        "properties": {
                                            "type": {"const": "plan_approval_response"},
                                            "request_id": {"type": "string"},
                                            "approve": {"type": "boolean"},
                                            "feedback": {"type": "string"}
                                        },
                                        "required": ["type", "request_id", "approve"],
                                        "additionalProperties": false
                                    }
                                ]
                            }
                        ]
                    }
                },
                "required": ["to", "message"],
                "additionalProperties": false,
            }),
        },
        // ── Plan mode tools ────────────────────────────────────────────────
        ToolSpec {
            name: "enter_plan_mode".to_owned(),
            protocol_name: "EnterPlanMode".to_owned(),
            permission_tool_name: "EnterPlanMode".to_owned(),
            description: tool_prompts::ENTER_PLAN_MODE.to_owned(),
            requires_permission: true,
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false,
            }),
        },
        ToolSpec {
            name: "exit_plan_mode".to_owned(),
            protocol_name: "ExitPlanMode".to_owned(),
            permission_tool_name: "ExitPlanMode".to_owned(),
            description: tool_prompts::EXIT_PLAN_MODE.to_owned(),
            requires_permission: true,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "allowedPrompts": {
                        "type": "array",
                        "description": "Prompt-based permissions needed to implement the plan. These describe categories of actions rather than specific commands.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "tool": {"type": "string", "enum": ["Bash"], "description": "The tool this prompt applies to"},
                                "prompt": {"type": "string", "description": "Semantic description of the action, e.g. \"run tests\", \"install dependencies\""}
                            },
                            "required": ["tool", "prompt"],
                            "additionalProperties": false
                        }
                    },
                    "plan": {
                        "type": "string",
                        "description": "The plan content (injected by normalizeToolInput from disk)"
                    },
                    "planFilePath": {
                        "type": "string",
                        "description": "The plan file path (injected by normalizeToolInput)"
                    },
                    "plan_file_path": {
                        "type": "string",
                        "description": "The plan file path (runtime compatibility alias)"
                    }
                },
                "additionalProperties": false,
            }),
        },
        // ── Sleep tool ─────────────────────────────────────────────────────
        ToolSpec {
            name: "sleep".to_owned(),
            protocol_name: "Sleep".to_owned(),
            permission_tool_name: "Sleep".to_owned(),
            description: tool_prompts::SLEEP.to_owned(),
            requires_permission: false,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "seconds": {"type": "integer", "minimum": 0, "maximum": 30}
                },
                "required": ["seconds"],
                "additionalProperties": false,
            }),
        },
        // ── Snip tool ──────────────────────────────────────────────────────
        ToolSpec {
            name: "snip".to_owned(),
            protocol_name: "Snip".to_owned(),
            permission_tool_name: "Snip".to_owned(),
            description: tool_prompts::SNIP.to_owned(),
            requires_permission: false,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "content": {"type": "string"},
                    "label": {"type": "string"}
                },
                "required": ["content"],
                "additionalProperties": false,
            }),
        },
        // ── Phase 3 tools ──────────────────────────────────────────────────
        ToolSpec {
            name: "team_create".to_owned(),
            protocol_name: "TeamCreate".to_owned(),
            permission_tool_name: "TeamCreate".to_owned(),
            description: tool_prompts::team_create_tool_prompt(),
            requires_permission: false,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "team_name": {"type": "string", "description": "Name for the new team to create."},
                    "description": {"type": "string", "description": "Team description/purpose."},
                    "agent_type": {"type": "string", "description": "Type/role of the team lead (for team file and inter-agent coordination)."}
                },
                "required": ["team_name"],
                "additionalProperties": false,
            }),
        },
        ToolSpec {
            name: "team_status".to_owned(),
            protocol_name: "TeamStatus".to_owned(),
            permission_tool_name: "TeamStatus".to_owned(),
            description: tool_prompts::TEAM_STATUS.to_owned(),
            requires_permission: false,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "team_name": {"type": "string", "description": "Optional team name. If omitted, returns the active team or summaries for all teams."}
                },
                "additionalProperties": false,
            }),
        },
        ToolSpec {
            name: "web_browser".to_owned(),
            protocol_name: "WebBrowser".to_owned(),
            permission_tool_name: "WebBrowser".to_owned(),
            description: tool_prompts::WEB_BROWSER.to_owned(),
            requires_permission: true,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "url": {"type": "string"},
                    "action": {"type": "string", "enum": ["fetch", "extract_links", "extract_text", "screenshot"], "description": "Action to perform (default: fetch)"}
                },
                "required": ["url"],
                "additionalProperties": false,
            }),
        },
        ToolSpec {
            name: "tool_search".to_owned(),
            protocol_name: "ToolSearch".to_owned(),
            permission_tool_name: "ToolSearch".to_owned(),
            description: tool_prompts::TOOL_SEARCH.to_owned(),
            requires_permission: false,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string"},
                    "max_results": {"type": "integer", "minimum": 1, "maximum": 20}
                },
                "required": ["query"],
                "additionalProperties": false,
            }),
        },
        ToolSpec {
            name: "verify_plan".to_owned(),
            protocol_name: "VerifyPlan".to_owned(),
            permission_tool_name: "VerifyPlan".to_owned(),
            description: tool_prompts::VERIFY_PLAN.to_owned(),
            requires_permission: false,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "plan": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "List of plan item descriptions"
                    },
                    "completed": {
                        "type": "array",
                        "items": {"type": "boolean"},
                        "description": "Parallel boolean array indicating completion status"
                    }
                },
                "required": ["plan", "completed"],
                "additionalProperties": false,
            }),
        },
        ToolSpec {
            name: "terminal_capture".to_owned(),
            protocol_name: "TerminalCapture".to_owned(),
            permission_tool_name: "TerminalCapture".to_owned(),
            description: tool_prompts::TERMINAL_CAPTURE.to_owned(),
            requires_permission: true,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string"}
                },
                "required": ["command"],
                "additionalProperties": false,
            }),
        },
        // ── Phase 4: Upstream gap-fill tools ──────────────────────────────
        ToolSpec {
            name: "powershell".to_owned(),
            protocol_name: "PowerShell".to_owned(),
            permission_tool_name: "PowerShell".to_owned(),
            description: tool_prompts::powershell_tool_prompt(
                tool_prompts::PowerShellEdition::Unknown,
            ),
            requires_permission: true,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string"},
                    "cwd": {"type": "string", "description": "Optional working directory, relative to the current workspace. Use this instead of prefixing the command with cd or Set-Location."},
                    "description": {"type": "string", "description": "Optional short human description of what the command is doing."},
                    "timeout_ms": {"type": "integer", "minimum": 1000, "maximum": 600000},
                    "background": {"type": "boolean", "description": "Run the command in the background and return a task handle immediately."}
                },
                "required": ["command"],
                "additionalProperties": false,
            }),
        },
        ToolSpec {
            name: "repl".to_owned(),
            protocol_name: "REPL".to_owned(),
            permission_tool_name: "Bash".to_owned(),
            description: tool_prompts::REPL.to_owned(),
            requires_permission: true,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "language": {"type": "string", "enum": ["python", "node", "rust"]},
                    "code": {"type": "string"}
                },
                "required": ["language", "code"],
                "additionalProperties": false,
            }),
        },
        ToolSpec {
            name: "monitor".to_owned(),
            protocol_name: "Monitor".to_owned(),
            permission_tool_name: "Read".to_owned(),
            description: tool_prompts::MONITOR.to_owned(),
            requires_permission: false,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "target": {"type": "string", "enum": ["agents", "tasks", "sessions"]},
                    "interval_ms": {"type": "integer", "minimum": 100, "maximum": 60000}
                },
                "required": ["target"],
                "additionalProperties": false,
            }),
        },
        ToolSpec {
            name: "schedule_cron".to_owned(),
            protocol_name: "CronCreate".to_owned(),
            permission_tool_name: "Edit".to_owned(),
            description: tool_prompts::SCHEDULE_CRON.to_owned(),
            requires_permission: true,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "cron": {
                        "type": "string",
                        "description": "Standard 5-field cron expression in local time: \"M H DoM Mon DoW\" (e.g. \"*/5 * * * *\" = every 5 minutes, \"30 14 28 2 *\" = Feb 28 at 2:30pm local once)."
                    },
                    "prompt": {
                        "type": "string",
                        "description": "The prompt to enqueue at each fire time"
                    },
                    "recurring": {
                        "type": "boolean",
                        "default": true,
                        "description": "true = fire on every cron match until deleted or auto-expired after 7 days. false = fire once at the next match, then auto-delete. Use false for \"remind me at X\" one-shot requests with pinned minute/hour/dom/month."
                    },
                    "durable": {
                        "type": "boolean",
                        "default": false,
                        "description": "true = persist to .claude/scheduled_tasks.json and survive restarts. false (default) = in-memory only, dies when this Claude session ends. Use true only when the user asks the task to survive across sessions."
                    }
                },
                "required": ["cron", "prompt"],
                "additionalProperties": false,
            }),
        },
        ToolSpec {
            name: "cron_delete".to_owned(),
            protocol_name: "CronDelete".to_owned(),
            permission_tool_name: "CronDelete".to_owned(),
            description: tool_prompts::CRON_DELETE.to_owned(),
            requires_permission: false,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "Job ID returned by CronCreate"
                    }
                },
                "required": ["id"],
                "additionalProperties": false,
            }),
        },
        ToolSpec {
            name: "cron_list".to_owned(),
            protocol_name: "CronList".to_owned(),
            permission_tool_name: "CronList".to_owned(),
            description: tool_prompts::CRON_LIST.to_owned(),
            requires_permission: false,
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false,
            }),
        },
        ToolSpec {
            name: "remote_trigger".to_owned(),
            protocol_name: "RemoteTrigger".to_owned(),
            permission_tool_name: "RemoteTrigger".to_owned(),
            description: tool_prompts::REMOTE_TRIGGER.to_owned(),
            requires_permission: true,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "url": {"type": "string"},
                    "event": {"type": "string"},
                    "payload": {}
                },
                "required": ["url", "event"],
                "additionalProperties": false,
            }),
        },
        ToolSpec {
            name: "workflow".to_owned(),
            protocol_name: "Workflow".to_owned(),
            permission_tool_name: "Read".to_owned(),
            description: tool_prompts::WORKFLOW.to_owned(),
            requires_permission: false,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "action": {"type": "string", "enum": ["create", "run", "status", "list", "delete"]},
                    "name": {"type": "string"},
                    "steps": {
                        "type": "array",
                        "items": {"type": "string"}
                    },
                    "description": {"type": "string", "description": "Description for the workflow (used with create)"}
                },
                "required": ["action", "name"],
                "additionalProperties": false,
            }),
        },
        ToolSpec {
            name: "suggest_pr".to_owned(),
            protocol_name: "SuggestPR".to_owned(),
            permission_tool_name: "Read".to_owned(),
            description: tool_prompts::SUGGEST_PR.to_owned(),
            requires_permission: false,
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false,
            }),
        },
        ToolSpec {
            name: "enter_worktree".to_owned(),
            protocol_name: "EnterWorktree".to_owned(),
            permission_tool_name: "Edit".to_owned(),
            description: tool_prompts::enter_worktree_tool_prompt(),
            requires_permission: true,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string", "description": "Optional name for the worktree. Each \"/\"-separated segment may contain only letters, digits, dots, underscores, and dashes; max 64 chars total. A random name is generated if not provided."}
                },
                "additionalProperties": false,
            }),
        },
        ToolSpec {
            name: "exit_worktree".to_owned(),
            protocol_name: "ExitWorktree".to_owned(),
            permission_tool_name: "Edit".to_owned(),
            description: tool_prompts::exit_worktree_tool_prompt(),
            requires_permission: true,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "action": {"type": "string", "enum": ["keep", "remove"], "description": "\"keep\" leaves the worktree and branch on disk; \"remove\" deletes both."},
                    "discard_changes": {"type": "boolean", "description": "Required true when action is \"remove\" and the worktree has uncommitted files or unmerged commits. The tool will refuse and list them otherwise."}
                },
                "required": ["action"],
                "additionalProperties": false,
            }),
        },
        ToolSpec {
            name: "list_worktrees".to_owned(),
            protocol_name: "ListWorktrees".to_owned(),
            permission_tool_name: "Read".to_owned(),
            description: tool_prompts::LIST_WORKTREES.to_owned(),
            requires_permission: false,
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false,
            }),
        },
        ToolSpec {
            name: "brief".to_owned(),
            protocol_name: "Brief".to_owned(),
            permission_tool_name: "Read".to_owned(),
            description: tool_prompts::BRIEF.to_owned(),
            requires_permission: false,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "message": {"type": "string", "description": "The message to send to the user. Supports markdown."},
                    "attachments": {"type": "array", "items": {"type": "string"}, "description": "File paths (absolute or cwd-relative) for images, diffs, logs."},
                    "status": {"type": "string", "enum": ["normal", "proactive"], "description": "Intent label: 'normal' when replying, 'proactive' when initiating."}
                },
                "required": ["message"],
                "additionalProperties": false,
            }),
        },
        ToolSpec {
            name: "ctx_inspect".to_owned(),
            protocol_name: "CtxInspect".to_owned(),
            permission_tool_name: "Read".to_owned(),
            description: tool_prompts::CTX_INSPECT.to_owned(),
            requires_permission: false,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "action": {"type": "string", "enum": ["tokens", "messages", "tools"]}
                },
                "required": ["action"],
                "additionalProperties": false,
            }),
        },
        ToolSpec {
            name: "list_peers".to_owned(),
            protocol_name: "ListPeers".to_owned(),
            permission_tool_name: "Read".to_owned(),
            description: tool_prompts::LIST_PEERS.to_owned(),
            requires_permission: false,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "team_name": {"type": "string", "description": "Optional team name to scope the peer listing."}
                },
                "additionalProperties": false,
            }),
        },
        ToolSpec {
            name: "tungsten".to_owned(),
            protocol_name: "Tungsten".to_owned(),
            permission_tool_name: "Bash".to_owned(),
            description: tool_prompts::TUNGSTEN.to_owned(),
            requires_permission: true,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "action": {"type": "string", "enum": ["compile", "run", "test"]},
                    "target": {"type": "string"}
                },
                "required": ["action", "target"],
                "additionalProperties": false,
            }),
        },
        ToolSpec {
            name: "overflow_test".to_owned(),
            protocol_name: "OverflowTest".to_owned(),
            permission_tool_name: "Read".to_owned(),
            description: tool_prompts::OVERFLOW_TEST.to_owned(),
            requires_permission: false,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "scenario": {"type": "string", "enum": ["large_output", "many_messages", "deep_recursion"]}
                },
                "required": ["scenario"],
                "additionalProperties": false,
            }),
        },
        ToolSpec {
            name: "synthetic_output".to_owned(),
            protocol_name: "SyntheticOutput".to_owned(),
            permission_tool_name: "Read".to_owned(),
            description: tool_prompts::SYNTHETIC_OUTPUT.to_owned(),
            requires_permission: false,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "type": {"type": "string", "enum": ["json", "csv", "markdown", "text"]},
                    "rows": {"type": "integer", "minimum": 1, "maximum": 1000}
                },
                "required": ["type"],
                "additionalProperties": false,
            }),
        },
        ToolSpec {
            name: "mcp_auth".to_owned(),
            protocol_name: "McpAuth".to_owned(),
            permission_tool_name: "McpAuth".to_owned(),
            description: tool_prompts::MCP_AUTH.to_owned(),
            requires_permission: true,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "server": {"type": "string"},
                    "action": {"type": "string", "enum": ["login", "logout", "status"]}
                },
                "required": ["server", "action"],
                "additionalProperties": false,
            }),
        },
        ToolSpec {
            name: "mcp_call".to_owned(),
            protocol_name: "McpCall".to_owned(),
            permission_tool_name: "McpCall".to_owned(),
            description: tool_prompts::MCP_CALL.to_owned(),
            requires_permission: true,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "server": {"type": "string", "description": "MCP server name as defined in the MCP config"},
                    "tool": {"type": "string", "description": "Name of the tool to call on the MCP server"},
                    "arguments": {"type": "object", "description": "Arguments to pass to the MCP tool"}
                },
                "required": ["server", "tool"],
                "additionalProperties": false,
            }),
        },
        ToolSpec {
            name: "voice_input".to_owned(),
            protocol_name: "VoiceInput".to_owned(),
            permission_tool_name: "Read".to_owned(),
            description: tool_prompts::VOICE_INPUT.to_owned(),
            requires_permission: false,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "duration_secs": {"type": "integer", "minimum": 1, "maximum": 60, "description": "Recording duration in seconds (default 5)"},
                    "language": {"type": "string", "description": "Language code for transcription (default 'en')"}
                },
                "additionalProperties": false,
            }),
        },
        ToolSpec {
            name: "daemon".to_owned(),
            protocol_name: "Daemon".to_owned(),
            permission_tool_name: "Daemon".to_owned(),
            description: tool_prompts::DAEMON.to_owned(),
            requires_permission: true,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "action": {"type": "string", "enum": ["start", "stop", "status", "list", "restart", "logs"]},
                    "command": {"type": "string", "description": "Command to run (for start)"},
                    "id": {"type": "string", "description": "Daemon ID (for stop, restart, logs)"},
                    "lines": {"type": "integer", "description": "Number of log lines to read (for logs, default 50)", "minimum": 1, "maximum": 500}
                },
                "required": ["action"],
                "additionalProperties": false,
            }),
        },
    ]
}

/// MCP resource tools are injected only when at least one connected MCP server
/// advertises resources support. Keeping them out of the unconditional built-in
/// prefix matches Claude Code's MCP resource surface and avoids exposing dead
/// schemas to the model.
#[must_use]
pub fn mcp_resource_tool_specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "list_mcp_resources".to_owned(),
            protocol_name: "ListMcpResources".to_owned(),
            permission_tool_name: "Read".to_owned(),
            description: tool_prompts::LIST_MCP_RESOURCES.to_owned(),
            requires_permission: false,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "server": {
                        "type": "string",
                        "description": "Optional server name to filter resources by"
                    }
                },
                "additionalProperties": false,
            }),
        },
        ToolSpec {
            name: "read_mcp_resource".to_owned(),
            protocol_name: "ReadMcpResource".to_owned(),
            permission_tool_name: "Read".to_owned(),
            description: tool_prompts::READ_MCP_RESOURCE.to_owned(),
            requires_permission: false,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "server": {"type": "string"},
                    "uri": {"type": "string"}
                },
                "required": ["server", "uri"],
                "additionalProperties": false,
            }),
        },
    ]
}

#[must_use]
pub fn builtin_tool_specs() -> Vec<ToolSpec> {
    let mut specs = builtin_tool_specs_core();
    specs.extend(phase9_tool_specs());
    specs
}

/// Phase 9: Additional tool specs for new dedicated modules.
#[must_use]
pub fn phase9_tool_specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "discover_skills".to_owned(),
            protocol_name: "DiscoverSkills".to_owned(),
            permission_tool_name: "Read".to_owned(),
            description: tool_prompts::DISCOVER_SKILLS.to_owned(),
            requires_permission: false,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Task description to search for matching skills"},
                    "max_results": {"type": "integer", "minimum": 1, "maximum": 20, "description": "Maximum number of results (default 10)"}
                },
                "required": ["query"],
                "additionalProperties": false,
            }),
        },
        ToolSpec {
            name: "team_delete".to_owned(),
            protocol_name: "TeamDelete".to_owned(),
            permission_tool_name: "TeamDelete".to_owned(),
            description: tool_prompts::team_delete_tool_prompt(),
            requires_permission: true,
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false,
            }),
        },
        ToolSpec {
            name: "team_list".to_owned(),
            protocol_name: "TeamList".to_owned(),
            permission_tool_name: "Read".to_owned(),
            description: tool_prompts::TEAM_LIST.to_owned(),
            requires_permission: false,
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false,
            }),
        },
        ToolSpec {
            name: "broadcast_message".to_owned(),
            protocol_name: "BroadcastMessage".to_owned(),
            permission_tool_name: "SendMessage".to_owned(),
            description: tool_prompts::BROADCAST_MESSAGE.to_owned(),
            requires_permission: false,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "team_name": {"type": "string", "description": "Optional team name when more than one team exists."},
                    "message": {"type": "string", "description": "Message content to broadcast"},
                    "sender": {"type": "string", "description": "Sender agent name (default: coordinator)"},
                    "priority": {"type": "string", "enum": ["low", "normal", "high"], "description": "Message priority (default: normal)"},
                    "recipients": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Optional list of specific recipient agent names"
                    }
                },
                "required": ["message"],
                "additionalProperties": false,
            }),
        },
        ToolSpec {
            name: "review_artifact".to_owned(),
            protocol_name: "ReviewArtifact".to_owned(),
            permission_tool_name: "Read".to_owned(),
            description: tool_prompts::REVIEW_ARTIFACT.to_owned(),
            requires_permission: false,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "action": {"type": "string", "enum": ["view_diff", "add_comment", "update_status", "get_comments", "summary"], "description": "Review action to perform"},
                    "artifact_id": {"type": "string", "description": "Artifact identifier"},
                    "comment": {"type": "string", "description": "Comment text (for add_comment)"},
                    "status": {"type": "string", "enum": ["pending", "in_progress", "approved", "changes_requested", "rejected"], "description": "Review status (for update_status)"},
                    "author": {"type": "string", "description": "Comment author (default: reviewer)"},
                    "severity": {"type": "string", "enum": ["info", "suggestion", "warning", "critical"], "description": "Comment severity (default: info)"},
                    "file_path": {"type": "string", "description": "File path for inline comment"},
                    "line": {"type": "integer", "description": "Line number for inline comment"},
                    "from_version": {"type": "string", "description": "Git ref for diff start (default: HEAD~1)"},
                    "to_version": {"type": "string", "description": "Git ref for diff end (default: HEAD)"},
                    "reviewer": {"type": "string", "description": "Reviewer name (for update_status)"}
                },
                "required": ["action", "artifact_id"],
                "additionalProperties": false,
            }),
        },
        ToolSpec {
            name: "send_user_file".to_owned(),
            protocol_name: "SendUserFile".to_owned(),
            permission_tool_name: "Read".to_owned(),
            description: tool_prompts::SEND_USER_FILE.to_owned(),
            requires_permission: false,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "file_path": {"type": "string", "description": "Path to the file (relative to workspace)"},
                    "description": {"type": "string", "description": "Optional description of the file"},
                    "max_size_bytes": {"type": "integer", "minimum": 1, "maximum": 104857600, "description": "Maximum file size in bytes (default 10MB)"},
                    "max_text_chars": {"type": "integer", "minimum": 1000, "maximum": 500000, "description": "Maximum text content characters (default 50000)"}
                },
                "required": ["file_path"],
                "additionalProperties": false,
            }),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::{builtin_tool_specs, mcp_resource_tool_specs};
    use crate::tool_prompts;

    #[test]
    fn shell_tool_schemas_expose_cwd_controls() {
        let specs = builtin_tool_specs();

        let bash = specs
            .iter()
            .find(|spec| spec.name == "bash_command")
            .unwrap_or_else(|| panic!("missing tool spec for bash_command"));
        let bash_properties = bash
            .input_schema
            .get("properties")
            .and_then(|value| value.as_object())
            .unwrap_or_else(|| panic!("missing properties object for bash_command"));

        assert!(
            bash_properties.contains_key("description"),
            "bash_command should expose description"
        );
        assert!(
            bash_properties.contains_key("run_in_background"),
            "bash_command should expose run_in_background"
        );
        assert!(
            bash_properties.contains_key("timeout"),
            "bash_command should expose timeout"
        );
        assert!(
            bash_properties.contains_key("dangerouslyDisableSandbox"),
            "bash_command should expose dangerouslyDisableSandbox"
        );
        assert!(
            !bash_properties.contains_key("cwd"),
            "bash_command should not expose cwd"
        );
        assert!(
            !bash_properties.contains_key("timeout_ms"),
            "bash_command should not expose legacy timeout_ms"
        );
        assert!(
            !bash_properties.contains_key("background"),
            "bash_command should not expose legacy background"
        );

        let tool_name = "powershell";
        let spec = specs
            .iter()
            .find(|spec| spec.name == tool_name)
            .unwrap_or_else(|| panic!("missing tool spec for {tool_name}"));
        let properties = spec
            .input_schema
            .get("properties")
            .and_then(|value| value.as_object())
            .unwrap_or_else(|| panic!("missing properties object for {tool_name}"));

        assert!(
            properties.contains_key("cwd"),
            "{tool_name} should expose cwd"
        );
        assert!(
            properties.contains_key("description"),
            "{tool_name} should expose description"
        );
        assert!(
            properties.contains_key("background"),
            "{tool_name} should expose background"
        );
    }

    #[test]
    fn rich_tool_prompt_generators_drive_primary_file_and_shell_specs() {
        let specs = builtin_tool_specs();

        let expected = [
            ("read_file", tool_prompts::file_read_tool_prompt()),
            ("write_file", tool_prompts::file_write_tool_prompt()),
            ("edit_file", tool_prompts::file_edit_tool_prompt()),
            ("bash_command", tool_prompts::bash_tool_prompt()),
            ("web_search", tool_prompts::web_search_tool_prompt()),
            ("send_message", tool_prompts::send_message_tool_prompt()),
        ];

        for (tool_name, prompt) in expected {
            let spec = specs
                .iter()
                .find(|spec| spec.name == tool_name)
                .unwrap_or_else(|| panic!("missing tool spec for {tool_name}"));
            assert_eq!(
                spec.description, prompt,
                "{tool_name} should use the dynamic parity prompt generator"
            );
        }
    }

    #[test]
    fn filesystem_tool_schemas_match_research_path_fields() {
        let specs = builtin_tool_specs();
        let spec_by_name = |name: &str| {
            specs
                .iter()
                .find(|spec| spec.name == name)
                .unwrap_or_else(|| panic!("missing tool spec for {name}"))
        };
        let properties_for = |name: &str| {
            spec_by_name(name)
                .input_schema
                .get("properties")
                .and_then(|value| value.as_object())
                .unwrap_or_else(|| panic!("missing properties object for {name}"))
        };
        let required_for = |name: &str| {
            spec_by_name(name)
                .input_schema
                .get("required")
                .and_then(|value| value.as_array())
                .expect("required array")
                .iter()
                .filter_map(|value| value.as_str())
                .collect::<std::collections::BTreeSet<_>>()
        };

        for name in ["read_file", "write_file", "replace_in_file", "edit_file"] {
            let properties = properties_for(name);
            assert!(properties.contains_key("file_path"), "{name}");
            assert!(!properties.contains_key("path"), "{name}");
            assert!(required_for(name).contains("file_path"), "{name}");
        }

        let read = properties_for("read_file");
        assert!(read.contains_key("offset"));
        assert!(read.contains_key("limit"));
        assert!(!read.contains_key("start_line"));
        assert!(!read.contains_key("end_line"));
        assert!(!read.contains_key("max_chars"));

        let write = properties_for("write_file");
        assert!(!write.contains_key("append"));

        let edit = properties_for("edit_file");
        for field in ["old_string", "new_string", "replace_all"] {
            assert!(edit.contains_key(field), "edit_file should expose {field}");
        }
        assert!(!edit.contains_key("edits"));
        assert!(!edit.contains_key("create_if_missing"));

        let notebook = properties_for("notebook_edit");
        for field in [
            "notebook_path",
            "cell_id",
            "new_source",
            "cell_type",
            "edit_mode",
        ] {
            assert!(
                notebook.contains_key(field),
                "notebook_edit should expose {field}"
            );
        }
        assert!(!notebook.contains_key("path"));
        assert!(!notebook.contains_key("file_path"));
        assert!(!notebook.contains_key("cell_index"));
        assert!(required_for("notebook_edit").contains("notebook_path"));

        for name in ["glob", "grep", "list_directory"] {
            let properties = properties_for(name);
            assert!(properties.contains_key("path"), "{name}");
            assert!(!properties.contains_key("file_path"), "{name}");
        }
    }

    #[test]
    fn lsp_schema_matches_research_tool_contract() {
        let specs = builtin_tool_specs();
        let spec = specs
            .iter()
            .find(|spec| spec.name == "lsp")
            .expect("missing lsp spec");
        let properties = spec
            .input_schema
            .get("properties")
            .and_then(|value| value.as_object())
            .expect("lsp properties");
        let required = spec
            .input_schema
            .get("required")
            .and_then(|value| value.as_array())
            .expect("lsp required fields")
            .iter()
            .filter_map(|value| value.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        let operations = properties
            .get("operation")
            .and_then(|value| value.get("enum"))
            .and_then(|value| value.as_array())
            .expect("operation enum")
            .iter()
            .filter_map(|value| value.as_str())
            .collect::<std::collections::BTreeSet<_>>();

        assert!(properties.contains_key("filePath"));
        assert!(properties.contains_key("line"));
        assert!(properties.contains_key("character"));
        assert!(!properties.contains_key("file_path"));
        assert!(!properties.contains_key("symbol"));
        assert_eq!(
            required,
            ["operation", "filePath", "line", "character"]
                .into_iter()
                .collect()
        );
        assert_eq!(
            operations,
            [
                "goToDefinition",
                "findReferences",
                "hover",
                "documentSymbol",
                "workspaceSymbol",
                "goToImplementation",
                "prepareCallHierarchy",
                "incomingCalls",
                "outgoingCalls"
            ]
            .into_iter()
            .collect()
        );
    }

    #[test]
    fn enter_plan_mode_schema_matches_research_empty_input_contract() {
        let specs = builtin_tool_specs();
        let spec = specs
            .iter()
            .find(|spec| spec.name == "enter_plan_mode")
            .expect("enter_plan_mode spec");

        assert_eq!(spec.protocol_name, "EnterPlanMode");
        assert!(spec.requires_permission);
        assert_eq!(
            spec.input_schema,
            serde_json::json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false,
            })
        );
    }

    #[test]
    fn exit_plan_mode_schema_allows_runtime_injected_plan_fields() {
        let specs = builtin_tool_specs();
        let spec = specs
            .iter()
            .find(|spec| spec.name == "exit_plan_mode")
            .expect("exit_plan_mode spec");
        let properties = spec
            .input_schema
            .get("properties")
            .and_then(|value| value.as_object())
            .expect("properties");

        assert!(spec.requires_permission);
        assert!(properties.contains_key("allowedPrompts"));
        assert!(properties.contains_key("plan"));
        assert!(properties.contains_key("planFilePath"));
        assert!(properties.contains_key("plan_file_path"));
        assert_eq!(
            spec.input_schema["additionalProperties"],
            serde_json::Value::Bool(false)
        );
    }

    #[test]
    fn agent_schema_matches_research_surface() {
        let specs = builtin_tool_specs();
        let agent = specs
            .iter()
            .find(|spec| spec.name == "agent")
            .expect("missing agent spec");
        let properties = agent
            .input_schema
            .get("properties")
            .and_then(|value| value.as_object())
            .expect("agent properties");

        for field in [
            "description",
            "prompt",
            "subagent_type",
            "model",
            "run_in_background",
            "name",
            "team_name",
            "mode",
            "cwd",
        ] {
            assert!(
                properties.contains_key(field),
                "agent schema should expose {field}"
            );
        }

        assert!(
            !properties.contains_key("isolation"),
            "agent schema should not expose worktree isolation until the runtime implements it"
        );
        assert!(
            !properties.contains_key("tools"),
            "agent schema should hide legacy tools overrides from the model"
        );
        assert!(
            !properties.contains_key("tasks"),
            "agent schema should hide legacy batch delegation fields from the model"
        );

        let required = agent
            .input_schema
            .get("required")
            .and_then(|value| value.as_array())
            .expect("agent required list");
        let required_fields = required
            .iter()
            .filter_map(|value| value.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        assert!(required_fields.contains("description"));
        assert!(required_fields.contains("prompt"));
    }

    #[test]
    fn team_and_message_schemas_match_runtime_contract() {
        let specs = builtin_tool_specs();
        let spec_by_name = |name: &str| {
            specs
                .iter()
                .find(|spec| spec.name == name)
                .unwrap_or_else(|| panic!("missing tool spec for {name}"))
        };
        let properties_for = |name: &str| {
            spec_by_name(name)
                .input_schema
                .get("properties")
                .and_then(|value| value.as_object())
                .unwrap_or_else(|| panic!("missing properties object for {name}"))
        };

        let send_message = properties_for("send_message");
        for field in ["to", "summary", "message"] {
            assert!(
                send_message.contains_key(field),
                "send_message should expose {field}"
            );
        }
        for hidden in [
            "team_name",
            "recipient",
            "sender",
            "priority",
            "correlation_id",
        ] {
            assert!(
                !send_message.contains_key(hidden),
                "send_message should hide legacy field {hidden}"
            );
        }

        let team_create = properties_for("team_create");
        for field in ["team_name", "description", "agent_type"] {
            assert!(
                team_create.contains_key(field),
                "team_create should expose {field}"
            );
        }
        for hidden in ["objective", "lead", "agents"] {
            assert!(
                !team_create.contains_key(hidden),
                "team_create should hide legacy field {hidden}"
            );
        }

        let team_create_required = spec_by_name("team_create")
            .input_schema
            .get("required")
            .and_then(|value| value.as_array())
            .expect("team_create required list")
            .iter()
            .filter_map(|value| value.as_str())
            .collect::<Vec<_>>();
        assert_eq!(team_create_required, vec!["team_name"]);

        let team_status = properties_for("team_status");
        assert!(
            team_status.contains_key("team_name"),
            "team_status should expose team_name"
        );

        let list_peers = properties_for("list_peers");
        assert!(
            list_peers.contains_key("team_name"),
            "list_peers should expose team_name"
        );

        let team_delete = spec_by_name("team_delete");
        assert_eq!(
            team_delete.input_schema,
            serde_json::json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false,
            })
        );

        let broadcast = properties_for("broadcast_message");
        for field in ["team_name", "message", "sender", "priority", "recipients"] {
            assert!(
                broadcast.contains_key(field),
                "broadcast_message should expose {field}"
            );
        }
    }

    #[test]
    fn mcp_resource_schemas_match_runtime_contract() {
        let specs = mcp_resource_tool_specs();
        let read = specs
            .iter()
            .find(|spec| spec.name == "read_mcp_resource")
            .expect("read_mcp_resource spec");
        let required = read
            .input_schema
            .get("required")
            .and_then(|value| value.as_array())
            .expect("required array")
            .iter()
            .filter_map(|value| value.as_str())
            .collect::<Vec<_>>();
        assert_eq!(required, vec!["server", "uri"]);

        let list = specs
            .iter()
            .find(|spec| spec.name == "list_mcp_resources")
            .expect("list_mcp_resources spec");
        assert!(
            list.input_schema.get("required").is_none(),
            "server filter is optional like upstream ListMcpResources"
        );
        assert!(list.input_schema["properties"].get("server").is_some());
    }
}
