use anyhow::{Result, anyhow};
use claude_config::RuntimeConfig;
use claude_tools::tasks::{BackgroundTask, load_persisted_task, load_persisted_tasks};
use uuid::Uuid;

use crate::cli::{TaskShowArgs, TasksCommand, TasksListArgs};

pub(crate) fn run_tasks(config: &RuntimeConfig, command: Option<TasksCommand>) -> Result<()> {
    match command.unwrap_or(TasksCommand::List(TasksListArgs {
        session_id: None,
        json: false,
    })) {
        TasksCommand::List(args) => run_task_list(config, args),
        TasksCommand::Show(args) => run_task_show(config, args),
    }
}

fn run_task_list(config: &RuntimeConfig, args: TasksListArgs) -> Result<()> {
    let session_id = args.session_id.unwrap_or(config.session_id);
    let tasks = load_persisted_tasks(&task_dir(config, session_id))?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&tasks)?);
        return Ok(());
    }
    if tasks.is_empty() {
        println!("No tasks found for session {session_id}.");
        return Ok(());
    }

    println!("Tasks for session {session_id}:");
    for task in tasks {
        let summary = if task.summary.trim().is_empty() {
            String::new()
        } else {
            format!(" — {}", task.summary)
        };
        println!(
            "{}  {:<10} {:<11} {}{}",
            task.id,
            task.status.as_str(),
            task.kind.as_str(),
            task.title,
            summary
        );
        if let Some(path) = &task.output_path {
            println!("    output: {path}");
        }
    }
    Ok(())
}

fn run_task_show(config: &RuntimeConfig, args: TaskShowArgs) -> Result<()> {
    let session_id = args.session_id.unwrap_or(config.session_id);
    let task =
        load_persisted_task(&task_dir(config, session_id), &args.task_id)?.ok_or_else(|| {
            anyhow!(
                "task '{}' not found for session {}",
                args.task_id,
                session_id
            )
        })?;
    if args.output {
        if task.output.trim().is_empty() {
            println!("Task '{}' has no captured output.", task.id);
        } else {
            println!("{}", task.output);
        }
        return Ok(());
    }
    if args.json {
        println!("{}", serde_json::to_string_pretty(&task)?);
        return Ok(());
    }
    print_task(task);
    Ok(())
}

fn print_task(task: BackgroundTask) {
    println!("Task {}", task.id);
    println!("- title: {}", task.title);
    println!("- status: {}", task.status.as_str());
    println!("- kind: {}", task.kind.as_str());
    println!("- depth: {}", task.depth);
    if let Some(parent_task_id) = &task.parent_task_id {
        println!("- parent: {parent_task_id}");
    }
    if !task.summary.trim().is_empty() {
        println!("- summary: {}", task.summary);
    }
    if let Some(turns_used) = task.turns_used {
        println!("- turns used: {turns_used}");
    }
    println!("- created: {}", task.created_at);
    println!("- updated: {}", task.updated_at);
    if let Some(path) = &task.output_path {
        println!("- output file: {path}");
    }
    if !task.output.trim().is_empty() {
        println!();
        println!("{}", task.output);
    }
}

fn task_dir(config: &RuntimeConfig, session_id: Uuid) -> std::path::PathBuf {
    config
        .paths
        .artifacts_dir
        .join("tasks")
        .join(session_id.to_string())
}
