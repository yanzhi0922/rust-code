use std::collections::HashMap;
use std::path::PathBuf;

use claude_config::RuntimeConfig;
use claude_tools::tasks::{
    BackgroundTask, load_persisted_task, load_persisted_tasks, task_snapshots,
};

pub fn dispatch(input: &str, config: &RuntimeConfig) {
    let mut parts = input.split_whitespace();
    let _command = parts.next();
    match parts.next() {
        Some("show") => {
            let Some(task_id) = parts.next() else {
                println!("Usage: /tasks show <task-id>");
                return;
            };
            render_task_detail(config, task_id);
        }
        Some("output") => {
            let Some(task_id) = parts.next() else {
                println!("Usage: /tasks output <task-id>");
                return;
            };
            render_task_output(config, task_id);
        }
        Some(subcommand) => {
            println!("Unknown /tasks subcommand '{subcommand}'.");
            println!("Usage: /tasks [show <task-id>|output <task-id>]");
        }
        None => render_task_list(config),
    }
}

fn render_task_list(config: &RuntimeConfig) {
    let tasks = current_or_persisted_tasks(config);
    if tasks.is_empty() {
        println!("No tasks found.");
        return;
    }

    println!("Tasks:");

    let mut by_parent: HashMap<Option<String>, Vec<BackgroundTask>> = HashMap::new();
    for task in tasks {
        by_parent
            .entry(task.parent_task_id.clone())
            .or_default()
            .push(task);
    }

    let mut roots = by_parent.remove(&None).unwrap_or_default();
    roots.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
    for task in roots {
        render_tree_row(&task, &by_parent);
    }
}

fn render_tree_row(
    task: &BackgroundTask,
    by_parent: &HashMap<Option<String>, Vec<BackgroundTask>>,
) {
    let indent = "  ".repeat(task.depth as usize);
    let kind = task.kind.as_str();
    let summary = if task.summary.trim().is_empty() {
        String::new()
    } else {
        format!(" — {}", task.summary)
    };
    println!(
        "{indent}{}  {:<10} {:<11} {}{}",
        task.id,
        task.status.as_str(),
        kind,
        task.title,
        summary
    );
    if let Some(path) = &task.output_path {
        println!("{indent}    output: {path}");
    }

    let mut children = by_parent
        .get(&Some(task.id.clone()))
        .cloned()
        .unwrap_or_default();
    children.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
    for child in children {
        render_tree_row(&child, by_parent);
    }
}

fn render_task_detail(config: &RuntimeConfig, task_id: &str) {
    let task = current_or_persisted_tasks(config)
        .into_iter()
        .find(|task| task.id == task_id);
    let Some(task) = task else {
        println!("Task '{task_id}' not found.");
        return;
    };

    println!("Task: {}", task.id);
    println!("Title: {}", task.title);
    println!("Status: {}", task.status.as_str());
    println!("Kind: {}", task.kind.as_str());
    println!("Depth: {}", task.depth);
    if let Some(parent_task_id) = &task.parent_task_id {
        println!("Parent: {parent_task_id}");
    }
    if !task.summary.trim().is_empty() {
        println!("Summary: {}", task.summary);
    }
    if let Some(turns_used) = task.turns_used {
        println!("Turns used: {turns_used}");
    }
    println!("Created: {}", task.created_at);
    println!("Updated: {}", task.updated_at);
    if let Some(path) = &task.output_path {
        println!("Output file: {path}");
    }
    println!("Usage:");
    println!("  /tasks output {}", task.id);
}

fn render_task_output(config: &RuntimeConfig, task_id: &str) {
    let task = task_snapshots()
        .into_iter()
        .find(|task| task.id == task_id)
        .or_else(|| {
            load_persisted_task(&task_dir(config), task_id)
                .ok()
                .flatten()
        });
    let Some(task) = task else {
        println!("Task '{task_id}' not found.");
        return;
    };

    if task.output.trim().is_empty() {
        println!("Task '{task_id}' has no captured output.");
        if let Some(path) = &task.output_path {
            println!("Output file: {path}");
        }
        return;
    }

    println!("{}", task.output);
}

fn current_or_persisted_tasks(config: &RuntimeConfig) -> Vec<BackgroundTask> {
    let tasks = task_snapshots();
    if tasks.is_empty() {
        load_persisted_tasks(&task_dir(config)).unwrap_or_default()
    } else {
        tasks
    }
}

fn task_dir(config: &RuntimeConfig) -> PathBuf {
    config
        .paths
        .artifacts_dir
        .join("tasks")
        .join(config.session_id.to_string())
}
