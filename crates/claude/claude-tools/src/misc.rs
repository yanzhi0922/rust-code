//! Miscellaneous tools: ask_user, lsp_tool, notebook_edit, skill_discover,
//! team_create/status, remote_trigger, tungsten, overflow_test, synthetic_output,
//! skill_execute, voice_input.

use std::process::Stdio;

use std::time::UNIX_EPOCH;

use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use uuid::Uuid;

use super::{FileState, ToolExecutionContext};

pub(crate) fn ask_user(input: &Value, _context: &ToolExecutionContext) -> Result<String> {
    let questions = normalize_ask_user_questions(input)?;
    let answers = input.get("answers").cloned().unwrap_or_else(|| json!({}));
    let annotations = input
        .get("annotations")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let metadata = input.get("metadata").cloned();

    let mut payload = json!({
        "type": "ask_user",
        "questions": questions,
        "answers": answers,
        "annotations": annotations,
        "message": "Waiting for user input. In headless mode, please provide the answer via the input stream."
    });
    if let Some(metadata) = metadata
        && let Some(object) = payload.as_object_mut()
    {
        object.insert("metadata".to_owned(), metadata);
    }
    Ok(payload.to_string())
}

fn normalize_ask_user_questions(input: &Value) -> Result<Vec<Value>> {
    if let Some(questions) = input.get("questions").and_then(Value::as_array) {
        validate_ask_user_questions(questions)?;
        return Ok(questions.clone());
    }

    let question = input
        .get("question")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("ask_user requires questions"))?;
    let options = input
        .get("suggestions")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(|label| {
                    json!({
                        "label": label,
                        "description": label,
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| {
            vec![
                json!({"label": "Yes", "description": "Proceed with this option."}),
                json!({"label": "No", "description": "Do not proceed with this option."}),
            ]
        });
    let header = input
        .get("header")
        .and_then(Value::as_str)
        .unwrap_or("Question");
    let legacy_questions = vec![json!({
        "question": question,
        "header": header,
        "options": options,
        "multiSelect": false,
    })];
    validate_ask_user_questions(&legacy_questions)?;
    Ok(legacy_questions)
}

fn validate_ask_user_questions(questions: &[Value]) -> Result<()> {
    if !(1..=4).contains(&questions.len()) {
        return Err(anyhow!("ask_user requires 1-4 questions"));
    }

    let mut seen_questions = std::collections::HashSet::new();
    for question in questions {
        let text = question
            .get("question")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("ask_user question text is required"))?;
        if !seen_questions.insert(text.to_owned()) {
            return Err(anyhow!("ask_user question texts must be unique"));
        }
        let header = question
            .get("header")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("ask_user question header is required"))?;
        if header.chars().count() > 12 {
            return Err(anyhow!("ask_user question header must be at most 12 chars"));
        }
        let options = question
            .get("options")
            .and_then(Value::as_array)
            .ok_or_else(|| anyhow!("ask_user question options are required"))?;
        if !(2..=4).contains(&options.len()) {
            return Err(anyhow!(
                "ask_user question options must contain 2-4 choices"
            ));
        }
        let multi_select = question
            .get("multiSelect")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let mut seen_labels = std::collections::HashSet::new();
        for option in options {
            let label = option
                .get("label")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("ask_user option label is required"))?;
            if !seen_labels.insert(label.to_owned()) {
                return Err(anyhow!(
                    "ask_user option labels must be unique within each question"
                ));
            }
            option
                .get("description")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("ask_user option description is required"))?;
            if multi_select && option.get("preview").is_some() {
                return Err(anyhow!(
                    "ask_user option previews are only supported for single-select questions"
                ));
            }
        }
    }
    Ok(())
}

pub(crate) async fn lsp_tool(input: &Value, context: &ToolExecutionContext) -> Result<String> {
    // Accept both "operation" (TS parity) and legacy "action" for backward compat.
    let operation = input
        .get("operation")
        .or_else(|| input.get("action"))
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("operation is required"))?;

    // Accept both "file_path" (Rust convention) and "filePath" (TS parity).
    let file_path = input
        .get("file_path")
        .or_else(|| input.get("filePath"))
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("file_path is required"))?;

    // Accept both "character" (TS parity) and legacy "column".
    let character = input
        .get("character")
        .or_else(|| input.get("column"))
        .and_then(Value::as_u64)
        .unwrap_or(1);
    let line = input.get("line").and_then(Value::as_u64).unwrap_or(1);

    let client = super::lsp::LspClient::new(&context.cwd);

    match operation {
        "goToDefinition" | "definitions" => {
            let symbol = input.get("symbol").and_then(Value::as_str).unwrap_or({
                // Best-effort: use position-based lookup when no explicit symbol given.
                ""
            });
            if symbol.is_empty() {
                // Position-based: try to find a symbol at the given line/character.
                let locations =
                    client.find_definitions_at(file_path, line as u32, character as u32)?;
                if locations.is_empty() {
                    Ok("No definitions found at the given position.".to_owned())
                } else {
                    Ok(super::lsp::format_locations(&locations))
                }
            } else {
                let locations = client.find_definitions(symbol, Some(file_path))?;
                if locations.is_empty() {
                    Ok(format!("No definitions found for '{symbol}'."))
                } else {
                    Ok(super::lsp::format_locations(&locations))
                }
            }
        }
        "findReferences" | "references" => {
            let symbol = input.get("symbol").and_then(Value::as_str).unwrap_or("");
            if symbol.is_empty() {
                let locations =
                    client.find_references_at(file_path, line as u32, character as u32)?;
                if locations.is_empty() {
                    Ok("No references found at the given position.".to_owned())
                } else {
                    Ok(super::lsp::format_locations(&locations))
                }
            } else {
                let locations = client.find_references(symbol)?;
                if locations.is_empty() {
                    Ok(format!("No references found for '{symbol}'."))
                } else {
                    Ok(super::lsp::format_locations(&locations))
                }
            }
        }
        "hover" => {
            let symbol = input.get("symbol").and_then(Value::as_str).unwrap_or("");
            if symbol.is_empty() {
                client.hover_at(file_path, line as u32, character as u32)
            } else {
                client.hover(file_path, symbol)
            }
        }
        "documentSymbol" => {
            let symbols = client.document_symbols(file_path)?;
            if symbols.is_empty() {
                Ok("No symbols found in the document.".to_owned())
            } else {
                Ok(symbols.join("\n"))
            }
        }
        "workspaceSymbol" => {
            let query = input.get("symbol").and_then(Value::as_str).unwrap_or("");
            let symbols = client.workspace_symbols(query)?;
            if symbols.is_empty() {
                Ok("No workspace symbols found.".to_owned())
            } else {
                Ok(symbols.join("\n"))
            }
        }
        "goToImplementation" => {
            let symbol = input.get("symbol").and_then(Value::as_str).unwrap_or("");
            if symbol.is_empty() {
                let locations =
                    client.find_implementations_at(file_path, line as u32, character as u32)?;
                if locations.is_empty() {
                    Ok("No implementations found at the given position.".to_owned())
                } else {
                    Ok(super::lsp::format_locations(&locations))
                }
            } else {
                let locations = client.find_implementations(symbol)?;
                if locations.is_empty() {
                    Ok(format!("No implementations found for '{symbol}'."))
                } else {
                    Ok(super::lsp::format_locations(&locations))
                }
            }
        }
        "prepareCallHierarchy" => {
            let items = client.prepare_call_hierarchy(file_path, line as u32, character as u32)?;
            if items.is_empty() {
                Ok("No call hierarchy item at the given position.".to_owned())
            } else {
                Ok(items.join("\n"))
            }
        }
        "incomingCalls" => {
            let calls = client.incoming_calls(file_path, line as u32, character as u32)?;
            if calls.is_empty() {
                Ok("No incoming calls found.".to_owned())
            } else {
                Ok(calls.join("\n"))
            }
        }
        "outgoingCalls" => {
            let calls = client.outgoing_calls(file_path, line as u32, character as u32)?;
            if calls.is_empty() {
                Ok("No outgoing calls found.".to_owned())
            } else {
                Ok(calls.join("\n"))
            }
        }
        _ => Err(anyhow!("Unknown LSP operation: {operation}")),
    }
}

pub(crate) fn notebook_edit(input: &Value, context: &ToolExecutionContext) -> Result<String> {
    let path = input
        .get("notebook_path")
        .or_else(|| input.get("file_path"))
        .or_else(|| input.get("path"))
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("notebook_path is required"))?;
    let new_source = input["new_source"]
        .as_str()
        .ok_or_else(|| anyhow!("new_source is required"))?;
    let edit_mode = input
        .get("edit_mode")
        .and_then(Value::as_str)
        .unwrap_or("replace");
    if !matches!(edit_mode, "replace" | "insert" | "delete") {
        return Err(anyhow!("Edit mode must be replace, insert, or delete."));
    }

    let target = super::file_ops::resolve_workspace_path_for_operation(
        context,
        Some(path),
        claude_permissions::FilesystemOperation::Write,
    )?;
    if target.extension().and_then(|ext| ext.to_str()) != Some("ipynb") {
        return Err(anyhow!(
            "File must be a Jupyter notebook (.ipynb file). For editing other file types, use the FileEdit tool."
        ));
    }
    let content = std::fs::read_to_string(&target)
        .with_context(|| format!("failed to read notebook {}", target.display()))?;
    let Some(read_state) = context.read_file_state.get(&target) else {
        return Err(anyhow!(
            "File has not been read yet. Read it first before writing to it."
        ));
    };
    if read_state.is_partial_view {
        return Err(anyhow!(
            "File has not been read yet. Read it first before writing to it."
        ));
    }
    let current_mtime = notebook_mtime_ms(&target)?;
    if current_mtime > read_state.timestamp {
        return Err(anyhow!(
            "File has been modified since read, either by the user or by a linter. Read it again before attempting to write it."
        ));
    }
    let mut notebook: Value = serde_json::from_str(&content)
        .with_context(|| format!("failed to parse notebook {}", target.display()))?;

    let notebook_shape = NotebookShape::from_notebook(&notebook);
    let cells = notebook
        .get_mut("cells")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| anyhow!("notebook has no cells array"))?;

    let cell_index = notebook_cell_index(cells, input, edit_mode)?;

    if edit_mode == "replace" && cell_index == cells.len() {
        let cell_type = input
            .get("cell_type")
            .and_then(Value::as_str)
            .unwrap_or("code");
        cells.push(new_notebook_cell(&notebook_shape, new_source, cell_type)?);
    } else if cell_index >= cells.len() && edit_mode != "insert" {
        return Err(anyhow!(
            "cell index {} out of range ({} cells)",
            cell_index,
            cells.len()
        ));
    }

    match edit_mode {
        "delete" => {
            cells.remove(cell_index);
        }
        "insert" => {
            let cell_type = input
                .get("cell_type")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("Cell type is required when using edit_mode=insert."))?;
            let insert_index = cell_index.min(cells.len());
            cells.insert(
                insert_index,
                new_notebook_cell(&notebook_shape, new_source, cell_type)?,
            );
        }
        "replace" => {
            let cell = &mut cells[cell_index];
            cell["source"] = json!(new_source);
            if let Some(cell_type) = input.get("cell_type").and_then(Value::as_str) {
                cell["cell_type"] = json!(cell_type);
            }
            if cell
                .get("cell_type")
                .and_then(Value::as_str)
                .is_some_and(|ct| ct == "code")
            {
                cell["outputs"] = json!([]);
                cell["execution_count"] = Value::Null;
            }
        }
        _ => unreachable!("validated edit_mode"),
    }

    if notebook_mtime_ms(&target)? > read_state.timestamp {
        return Err(anyhow!(
            "File has been unexpectedly modified. Read it again before attempting to write it."
        ));
    }
    let output = serde_json::to_string_pretty(&notebook)?;
    std::fs::write(&target, output)?;
    let updated = std::fs::read_to_string(&target).unwrap_or_default();
    context.read_file_state.set(
        &target,
        FileState::post_write(updated, notebook_mtime_ms(&target)?),
    );

    Ok(format!(
        "{edit_mode} cell {cell_index} in {}",
        target.display()
    ))
}

fn notebook_mtime_ms(path: &std::path::Path) -> Result<u128> {
    Ok(std::fs::metadata(path)?
        .modified()?
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis())
}

fn notebook_cell_index(cells: &[Value], input: &Value, edit_mode: &str) -> Result<usize> {
    // Accept both "cell_number" (current schema) and "cell_index" (legacy).
    let numeric_index = input
        .get("cell_number")
        .or_else(|| input.get("cell_index"))
        .and_then(Value::as_u64);
    if let Some(index) = numeric_index {
        return Ok(if edit_mode == "insert" {
            index as usize + 1
        } else {
            index as usize
        });
    }

    let Some(cell_id) = input.get("cell_id").and_then(Value::as_str) else {
        if edit_mode == "insert" {
            return Ok(0);
        }
        return Err(anyhow!(
            "Cell ID must be specified when not inserting a new cell."
        ));
    };

    if let Some(found) = cells
        .iter()
        .position(|cell| cell.get("id").and_then(Value::as_str) == Some(cell_id))
    {
        return Ok(if edit_mode == "insert" {
            found + 1
        } else {
            found
        });
    }

    if let Some(index) = parse_cell_id(cell_id) {
        return Ok(if edit_mode == "insert" {
            index + 1
        } else {
            index
        });
    }

    Err(anyhow!(
        "Cell with ID \"{}\" not found in notebook.",
        cell_id
    ))
}

fn parse_cell_id(cell_id: &str) -> Option<usize> {
    let suffix = cell_id.strip_prefix("cell-")?;
    suffix.parse::<usize>().ok()
}

#[derive(Debug, Clone, Copy)]
struct NotebookShape {
    nbformat: u64,
    nbformat_minor: u64,
}

impl NotebookShape {
    fn from_notebook(notebook: &Value) -> Self {
        Self {
            nbformat: notebook
                .get("nbformat")
                .and_then(Value::as_u64)
                .unwrap_or(4),
            nbformat_minor: notebook
                .get("nbformat_minor")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        }
    }

    fn includes_cell_ids(self) -> bool {
        self.nbformat > 4 || (self.nbformat == 4 && self.nbformat_minor >= 5)
    }
}

fn new_notebook_cell(shape: &NotebookShape, new_source: &str, cell_type: &str) -> Result<Value> {
    if !matches!(cell_type, "code" | "markdown") {
        return Err(anyhow!("cell_type must be code or markdown"));
    }

    let cell_id = shape
        .includes_cell_ids()
        .then(|| format!("cell-{}", uuid::Uuid::new_v4().simple()));

    let mut cell = if cell_type == "markdown" {
        json!({
            "cell_type": "markdown",
            "source": new_source,
            "metadata": {},
        })
    } else {
        json!({
            "cell_type": "code",
            "source": new_source,
            "metadata": {},
            "execution_count": null,
            "outputs": [],
        })
    };

    if let Some(cell_id) = cell_id {
        cell["id"] = json!(cell_id);
    }
    Ok(cell)
}

pub(crate) fn skill_discover(input: &Value, context: &ToolExecutionContext) -> Result<String> {
    // Search common skill locations
    let search_dirs = [
        context.cwd.join(".roo"),
        context.cwd.join(".remote-code-rust"),
    ];

    let mut all_skills = Vec::new();
    for dir in &search_dirs {
        if dir.exists() {
            match claude_skills::discover_skills(dir) {
                Ok(skills) => {
                    for skill in skills {
                        all_skills.push(json!({
                            "slug": skill.metadata.slug,
                            "title": skill.metadata.title,
                            "summary": skill.metadata.summary,
                            "path": skill.metadata.path,
                            "tools": skill.metadata.tools,
                            "triggers": skill.metadata.triggers,
                        }));
                    }
                }
                Err(e) => {
                    all_skills.push(json!({
                        "error": format!("Error scanning {}: {e}", dir.display())
                    }));
                }
            }
        }
    }

    // Also search the workspace root itself
    if let Ok(skills) = claude_skills::discover_skills(&context.cwd) {
        for skill in skills {
            all_skills.push(json!({
                "slug": skill.metadata.slug,
                "title": skill.metadata.title,
                "summary": skill.metadata.summary,
                "path": skill.metadata.path,
                "tools": skill.metadata.tools,
                "triggers": skill.metadata.triggers,
            }));
        }
    }

    // Suppress unused variable warning for input
    let _ = input;

    if all_skills.is_empty() {
        Ok("No skills found in the current workspace.".to_owned())
    } else {
        Ok(serde_json::to_string_pretty(&all_skills)?)
    }
}

pub(crate) async fn team_create_tool(
    input: &Value,
    context: &ToolExecutionContext,
) -> Result<String> {
    super::team_runtime::create_team(input, &context.cwd).await
}

pub(crate) async fn team_status_tool(input: &Value) -> Result<String> {
    super::team_runtime::team_status(input).await
}

pub(crate) async fn remote_trigger_tool(input: &Value) -> Result<String> {
    let url = input["url"]
        .as_str()
        .ok_or_else(|| anyhow!("url is required"))?;
    let event = input["event"]
        .as_str()
        .ok_or_else(|| anyhow!("event is required"))?;
    let payload = input.get("payload").cloned().unwrap_or(json!({}));

    let body = json!({
        "event": event,
        "payload": payload,
    });

    let client = reqwest::Client::new();
    let response = client
        .post(url)
        .json(&body)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .context("failed to send remote trigger")?;

    let status = response.status();
    let response_text = response
        .text()
        .await
        .context("failed to read trigger response")?;

    Ok(json!({
        "url": url,
        "event": event,
        "http_status": status.as_u16(),
        "response": response_text.chars().take(5000).collect::<String>(),
    })
    .to_string())
}

pub(crate) async fn tungsten_tool(input: &Value, context: &ToolExecutionContext) -> Result<String> {
    let action = input["action"]
        .as_str()
        .ok_or_else(|| anyhow!("action is required (compile, run, or test)"))?;
    let target = input["target"]
        .as_str()
        .ok_or_else(|| anyhow!("target is required"))?;

    // Detect project type by checking for marker files.
    let is_rust = context.cwd.join("Cargo.toml").exists()
        || context.cwd.join(target).join("Cargo.toml").exists();
    let is_node = context.cwd.join("package.json").exists()
        || context.cwd.join(target).join("package.json").exists();
    let is_python = context.cwd.join("setup.py").exists()
        || context.cwd.join("pyproject.toml").exists()
        || context.cwd.join(target).join("setup.py").exists();

    let command = match action {
        "compile" => {
            if is_rust {
                format!("cargo build --manifest-path {target}/Cargo.toml 2>&1 || cargo build 2>&1")
            } else if is_node {
                "npm run build 2>&1".to_owned()
            } else if is_python {
                "python -m py_compile . 2>&1".to_owned()
            } else {
                return Ok("Unable to detect project type. No Cargo.toml, package.json, or setup.py found.".to_owned());
            }
        }
        "run" => {
            if is_rust {
                format!("cargo run --manifest-path {target}/Cargo.toml 2>&1 || cargo run 2>&1")
            } else if is_node {
                "npm start 2>&1".to_owned()
            } else if is_python {
                "python main.py 2>&1".to_owned()
            } else {
                return Ok("Unable to detect project type.".to_owned());
            }
        }
        "test" => {
            if is_rust {
                format!("cargo test --manifest-path {target}/Cargo.toml 2>&1 || cargo test 2>&1")
            } else if is_node {
                "npm test 2>&1".to_owned()
            } else if is_python {
                "python -m pytest 2>&1".to_owned()
            } else {
                return Ok("Unable to detect project type.".to_owned());
            }
        }
        _ => return Err(anyhow!("action must be 'compile', 'run', or 'test'")),
    };

    let mut process = if cfg!(windows) {
        let mut cmd = Command::new("powershell");
        cmd.args(["-NoProfile", "-Command", &command]);
        cmd
    } else {
        let mut cmd = Command::new("sh");
        cmd.args(["-lc", &command]);
        cmd
    };
    process.current_dir(&context.cwd);
    process.stdout(Stdio::piped());
    process.stderr(Stdio::piped());

    let mut child = process
        .spawn()
        .context("failed to spawn tungsten command")?;
    let future = async {
        let mut stdout = String::new();
        let mut stderr = String::new();
        if let Some(mut stream) = child.stdout.take() {
            let _ = stream.read_to_string(&mut stdout).await;
        }
        if let Some(mut stream) = child.stderr.take() {
            let _ = stream.read_to_string(&mut stderr).await;
        }
        let status = child.wait().await?;
        Ok::<_, anyhow::Error>((status.success(), stdout, stderr))
    };
    let (success, stdout, stderr) =
        tokio::time::timeout(std::time::Duration::from_millis(context.timeout_ms), future)
            .await
            .map_err(|_| anyhow!("tungsten command timed out"))??;

    let mut parts = Vec::new();
    if !stdout.trim().is_empty() {
        parts.push(stdout.trim_end().to_owned());
    }
    if !stderr.trim().is_empty() {
        parts.push(format!("stderr:\n{}", stderr.trim_end()));
    }
    if !success {
        parts.push("exit_status: failed".to_owned());
    }
    Ok(if parts.is_empty() {
        "Command completed with no output.".to_owned()
    } else {
        parts.join("\n\n")
    })
}

pub(crate) fn overflow_test_tool(input: &Value) -> Result<String> {
    let scenario = input["scenario"].as_str().ok_or_else(|| {
        anyhow!("scenario is required (large_output, many_messages, or deep_recursion)")
    })?;

    match scenario {
        "large_output" => {
            let data: String = (0..10_000)
                .map(|i| format!("Line {i}: This is test output data for overflow testing.\n"))
                .collect();
            Ok(json!({
                "scenario": "large_output",
                "size_chars": data.len(),
                "size_lines": 10_000,
                "data_preview": data.chars().take(500).collect::<String>(),
            })
            .to_string())
        }
        "many_messages" => {
            let messages: Vec<Value> = (0..100)
                .map(|i| {
                    json!({
                        "id": i,
                        "role": if i % 2 == 0 { "user" } else { "assistant" },
                        "content": format!("Message {i}: Test content for context overflow testing."),
                    })
                })
                .collect();
            Ok(json!({
                "scenario": "many_messages",
                "count": messages.len(),
                "messages": messages,
            })
            .to_string())
        }
        "deep_recursion" => {
            let depth = 50;
            let mut nested = json!("leaf");
            for _ in 0..depth {
                nested = json!({ "child": nested });
            }
            Ok(json!({
                "scenario": "deep_recursion",
                "depth": depth,
                "structure": nested,
            })
            .to_string())
        }
        _ => Err(anyhow!(
            "scenario must be 'large_output', 'many_messages', or 'deep_recursion'"
        )),
    }
}

pub(crate) fn synthetic_output_tool(input: &Value) -> Result<String> {
    let output_type = input["type"]
        .as_str()
        .ok_or_else(|| anyhow!("type is required (json, csv, markdown, or text)"))?;
    let rows = input.get("rows").and_then(Value::as_u64).unwrap_or(10) as usize;

    match output_type {
        "json" => {
            let data: Vec<Value> = (0..rows)
                .map(|i| {
                    json!({
                        "id": i,
                        "name": format!("item_{i}"),
                        "value": i * 10,
                        "active": i % 2 == 0,
                    })
                })
                .collect();
            Ok(serde_json::to_string_pretty(&data)?)
        }
        "csv" => {
            let mut lines = vec!["id,name,value,active".to_owned()];
            for i in 0..rows {
                lines.push(format!("{i},item_{i},{},{}", i * 10, i % 2 == 0));
            }
            Ok(lines.join("\n"))
        }
        "markdown" => {
            let mut md = String::from("# Synthetic Report\n\n");
            md.push_str("| id | name | value | active |\n");
            md.push_str("|----|------|-------|--------|\n");
            for i in 0..rows {
                md.push_str(&format!(
                    "| {i} | item_{i} | {} | {} |\n",
                    i * 10,
                    i % 2 == 0
                ));
            }
            Ok(md)
        }
        "text" => {
            let lines: Vec<String> = (0..rows)
                .map(|i| {
                    format!(
                        "Row {i}: name=item_{i}, value={}, active={}",
                        i * 10,
                        i % 2 == 0
                    )
                })
                .collect();
            Ok(lines.join("\n"))
        }
        _ => Err(anyhow!("type must be 'json', 'csv', 'markdown', or 'text'")),
    }
}

/// Load and return a skill's instructions by slug.
///
/// Searches the workspace skill directories for a matching skill and returns
/// its full content (instructions) for the agent to follow.
pub(crate) fn skill_execute_tool(input: &Value, context: &ToolExecutionContext) -> Result<String> {
    let slug = input["slug"]
        .as_str()
        .ok_or_else(|| anyhow!("slug is required"))?;
    let arguments = input.get("arguments").cloned().unwrap_or(json!({}));

    let search_dirs = [
        context.cwd.join(".roo"),
        context.cwd.join(".remote-code-rust"),
        context.cwd.clone(),
    ];

    for dir in &search_dirs {
        if !dir.exists() {
            continue;
        }
        if let Ok(skills) = claude_skills::discover_skills(dir) {
            for skill in skills {
                if skill.metadata.slug == slug {
                    let summary = skill.metadata.summary.as_deref().unwrap_or("(no summary)");
                    let mut output = format!(
                        "# Skill: {} ({})\n\n{}\n\n",
                        skill.metadata.title, skill.metadata.slug, summary
                    );
                    if !skill.instructions.is_empty() {
                        output.push_str(&skill.instructions);
                    }
                    if !arguments.is_null() && !arguments.as_object().is_none_or(|o| o.is_empty()) {
                        output.push_str(&format!(
                            "\n\n## Arguments\n```json\n{}\n```",
                            serde_json::to_string_pretty(&arguments)?
                        ));
                    }
                    return Ok(output);
                }
            }
        }
    }

    Err(anyhow!(
        "Skill '{slug}' not found. Use skill_discover to list available skills."
    ))
}

pub(crate) fn voice_input_tool(input: &Value) -> Result<String> {
    let duration_secs = input
        .get("duration_secs")
        .and_then(Value::as_u64)
        .unwrap_or(5);
    let language = input["language"].as_str().unwrap_or("en");

    // Try to record audio using sox/rec/ffmpeg and transcribe with whisper.
    let temp_dir = std::env::temp_dir();
    let voice_stem = format!("remote-code-voice-{}", Uuid::new_v4().simple());
    let wav_path = temp_dir.join(format!("{voice_stem}.wav"));
    let txt_path = temp_dir.join(format!("{voice_stem}.txt"));

    // Attempt recording with sox (rec command) or ffmpeg.
    let record_result = if cfg!(windows) {
        // On Windows, try ffmpeg.
        std::process::Command::new("ffmpeg")
            .args([
                "-y",
                "-f",
                "dshow",
                "-i",
                "audio=microphone",
                "-t",
                &duration_secs.to_string(),
                "-ar",
                "16000",
                "-ac",
                "1",
                wav_path.to_str().unwrap_or(""),
            ])
            .output()
    } else {
        // On Unix, try rec (sox) first, then ffmpeg.
        let sox_result = std::process::Command::new("rec")
            .args([
                "-r",
                "16000",
                "-c",
                "1",
                wav_path.to_str().unwrap_or(""),
                "trim",
                "0",
                &duration_secs.to_string(),
            ])
            .output();
        match sox_result {
            Ok(out) if out.status.success() => Ok(out),
            _ => std::process::Command::new("ffmpeg")
                .args([
                    "-y",
                    "-f",
                    "alsa",
                    "-i",
                    "default",
                    "-t",
                    &duration_secs.to_string(),
                    "-ar",
                    "16000",
                    "-ac",
                    "1",
                    wav_path.to_str().unwrap_or(""),
                ])
                .output(),
        }
    };

    let recorded = matches!(record_result, Ok(out) if out.status.success() && wav_path.exists());

    if !recorded {
        let _ = std::fs::remove_file(&wav_path);
        return Ok(json!({
            "type": "voice_input",
            "duration_secs": duration_secs,
            "status": "recording_failed",
            "message": "Voice recording failed. Install sox (rec) or ffmpeg with audio support.",
            "hint": "Windows: install ffmpeg. macOS: brew install sox. Linux: apt install sox.",
        })
        .to_string());
    }

    // Try to transcribe with whisper CLI.
    let whisper_result = std::process::Command::new("whisper")
        .args([
            wav_path.to_str().unwrap_or(""),
            "--model",
            "base",
            "--language",
            language,
            "--output_format",
            "txt",
            "--output_dir",
            temp_dir.to_str().unwrap_or(""),
        ])
        .output();

    let transcription = match whisper_result {
        Ok(out) if out.status.success() => {
            if txt_path.exists() {
                std::fs::read_to_string(&txt_path).unwrap_or_default()
            } else {
                let _ = std::fs::remove_file(&wav_path);
                return Ok(json!({
                    "type": "voice_input",
                    "duration_secs": duration_secs,
                    "language": language,
                    "status": "transcription_failed",
                    "message": "Voice transcription failed because whisper did not produce a transcript file.",
                })
                .to_string());
            }
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr).to_string();
            let _ = std::fs::remove_file(&wav_path);
            let _ = std::fs::remove_file(&txt_path);
            return Ok(json!({
                "type": "voice_input",
                "duration_secs": duration_secs,
                "language": language,
                "status": "transcription_failed",
                "message": "Voice transcription failed.",
                "error": stderr.chars().take(200).collect::<String>(),
            })
            .to_string());
        }
        Err(error) => {
            let _ = std::fs::remove_file(&wav_path);
            let _ = std::fs::remove_file(&txt_path);
            return Ok(json!({
                "type": "voice_input",
                "duration_secs": duration_secs,
                "language": language,
                "status": "transcription_unavailable",
                "message": "Voice transcription is unavailable because the whisper CLI is not installed or not on PATH.",
                "error": error.to_string(),
            })
            .to_string());
        }
    };

    // Clean up temp files.
    let _ = std::fs::remove_file(&wav_path);
    let _ = std::fs::remove_file(&txt_path);

    Ok(json!({
        "type": "voice_input",
        "duration_secs": duration_secs,
        "language": language,
        "status": "success",
        "transcription": transcription.trim(),
    })
    .to_string())
}
