use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;

use crate::tasks::BackgroundTask;

#[derive(Debug, Clone, Serialize)]
struct PersistedTaskMetadata<'a> {
    id: &'a str,
    parent_task_id: Option<&'a str>,
    depth: u32,
    kind: &'a crate::tasks::TaskKind,
    title: &'a str,
    status: &'a crate::tasks::TaskStatus,
    summary: &'a str,
    turns_used: Option<u32>,
    created_at: &'a str,
    updated_at: &'a str,
    output_path: Option<&'a str>,
}

/// # Errors
/// Returns an error if the task-output directory cannot be created.
pub fn ensure_task_output_dir(base_dir: &Path) -> Result<()> {
    fs::create_dir_all(base_dir).with_context(|| format!("failed to create {}", base_dir.display()))
}

#[must_use]
pub fn task_output_file_path(base_dir: &Path, task_id: &str) -> PathBuf {
    base_dir.join(format!("{task_id}.output"))
}

/// Persist a task's metadata and output to the configured artifact directory.
///
/// # Errors
/// Returns an error if the task files cannot be written.
pub fn persist_task(base_dir: &Path, task: &BackgroundTask) -> Result<Option<PathBuf>> {
    ensure_task_output_dir(base_dir)?;

    let output_path = if task.output.trim().is_empty() {
        None
    } else {
        let path = base_dir.join(format!("{}.output.txt", task.id));
        fs::write(&path, &task.output)
            .with_context(|| format!("failed to write {}", path.display()))?;
        Some(path)
    };
    let output_path_string = output_path
        .as_ref()
        .map(|path| path.to_string_lossy().to_string());

    let metadata = PersistedTaskMetadata {
        id: &task.id,
        parent_task_id: task.parent_task_id.as_deref(),
        depth: task.depth,
        kind: &task.kind,
        title: &task.title,
        status: &task.status,
        summary: &task.summary,
        turns_used: task.turns_used,
        created_at: &task.created_at,
        updated_at: &task.updated_at,
        output_path: output_path_string.as_deref(),
    };
    let metadata_path = base_dir.join(format!("{}.json", task.id));
    let contents = serde_json::to_vec_pretty(&metadata)?;
    fs::write(&metadata_path, contents)
        .with_context(|| format!("failed to write {}", metadata_path.display()))?;
    Ok(output_path)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use tempfile::tempdir;

    use crate::tasks::{BackgroundTask, TaskStatus};

    use super::{persist_task, task_output_file_path};

    #[test]
    fn task_output_file_path_matches_research_suffix() {
        let path = task_output_file_path(PathBuf::from("base").as_path(), "task-1");
        assert_eq!(path, PathBuf::from("base").join("task-1.output"));
    }

    #[test]
    fn persist_task_writes_metadata_and_output() {
        let tempdir = tempdir().expect("tempdir");
        let task = BackgroundTask {
            id: "task-1".to_owned(),
            parent_task_id: Some("parent-1".to_owned()),
            depth: 1,
            kind: crate::tasks::TaskKind::Delegation,
            title: "Persist task".to_owned(),
            status: TaskStatus::Completed,
            summary: "done".to_owned(),
            output: "hello".to_owned(),
            output_path: None,
            turns_used: Some(2),
            created_at: "1".to_owned(),
            updated_at: "2".to_owned(),
        };
        let output_path = persist_task(tempdir.path(), &task).expect("persist task");
        assert_eq!(
            output_path,
            Some(PathBuf::from(
                tempdir
                    .path()
                    .join("task-1.output.txt")
                    .to_string_lossy()
                    .to_string()
            ))
        );
        assert!(tempdir.path().join("task-1.json").exists());
    }
}
