use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::semantics::ShellCommandAnalysis;
use crate::tool_result_storage::{
    PREVIEW_SIZE_BYTES, PersistedToolResult, build_large_tool_result_message,
    ensure_tool_results_dir, generate_preview, get_tool_result_path,
};

pub const MAX_PERSISTED_SHELL_OUTPUT_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShellOutputSummary {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    pub artifact_path: Option<PathBuf>,
}

#[must_use]
pub fn truncate_output(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let truncated = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}\n...[truncated]")
    } else {
        truncated
    }
}

pub fn persist_shell_output(
    output_dir: Option<&Path>,
    file_stem: &str,
    contents: &str,
) -> Result<Option<PathBuf>> {
    let Some(output_dir) = output_dir else {
        return Ok(None);
    };
    fs::create_dir_all(output_dir)
        .with_context(|| format!("failed to create {}", output_dir.display()))?;
    let path = output_dir.join(format!("{file_stem}.log"));
    fs::write(&path, contents).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(Some(path))
}

pub fn persist_large_shell_output(
    tool_results_dir: Option<&Path>,
    persist_id: &str,
    contents: &str,
) -> Result<Option<PersistedToolResult>> {
    persist_large_shell_output_with_cap(
        tool_results_dir,
        persist_id,
        contents,
        shell_preview_content(contents).as_ref(),
        MAX_PERSISTED_SHELL_OUTPUT_BYTES,
    )
}

pub fn persist_large_shell_output_file(
    tool_results_dir: Option<&Path>,
    persist_id: &str,
    source_path: &Path,
    original_size: u64,
    preview_source: &str,
) -> Result<Option<PersistedToolResult>> {
    persist_large_shell_output_file_with_cap(
        tool_results_dir,
        persist_id,
        source_path,
        original_size,
        preview_source,
        MAX_PERSISTED_SHELL_OUTPUT_BYTES as u64,
    )
}

fn persist_large_shell_output_with_cap(
    tool_results_dir: Option<&Path>,
    persist_id: &str,
    contents: &str,
    preview_source: &str,
    max_persisted_bytes: usize,
) -> Result<Option<PersistedToolResult>> {
    let Some(tool_results_dir) = tool_results_dir else {
        return Ok(None);
    };

    ensure_tool_results_dir(tool_results_dir)?;
    let filepath = get_tool_result_path(tool_results_dir, persist_id, false);
    let persisted = byte_prefix_at_char_boundary(contents, max_persisted_bytes);

    match fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&filepath)
    {
        Ok(mut file) => {
            use std::io::Write as _;

            file.write_all(persisted.as_bytes())
                .with_context(|| format!("failed to write {}", filepath.display()))?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => {
            return Err(error).with_context(|| format!("failed to write {}", filepath.display()));
        }
    }

    let (preview, has_more) = generate_preview(preview_source, PREVIEW_SIZE_BYTES);
    Ok(Some(build_persisted_shell_output_result(
        filepath,
        contents.len() as u64,
        preview,
        has_more,
    )))
}

fn persist_large_shell_output_file_with_cap(
    tool_results_dir: Option<&Path>,
    persist_id: &str,
    source_path: &Path,
    original_size: u64,
    preview_source: &str,
    max_persisted_bytes: u64,
) -> Result<Option<PersistedToolResult>> {
    let Some(tool_results_dir) = tool_results_dir else {
        return Ok(None);
    };

    ensure_tool_results_dir(tool_results_dir)?;
    let filepath = get_tool_result_path(tool_results_dir, persist_id, false);

    if !filepath.exists() {
        if original_size > max_persisted_bytes {
            let file = fs::OpenOptions::new()
                .write(true)
                .open(source_path)
                .with_context(|| format!("failed to open {}", source_path.display()))?;
            file.set_len(max_persisted_bytes)
                .with_context(|| format!("failed to truncate {}", source_path.display()))?;
        }

        match fs::hard_link(source_path, &filepath) {
            Ok(()) => {}
            Err(_) => {
                fs::copy(source_path, &filepath)
                    .with_context(|| format!("failed to copy {}", filepath.display()))?;
            }
        }
    }

    let (preview, has_more) = generate_preview(preview_source, PREVIEW_SIZE_BYTES);
    Ok(Some(build_persisted_shell_output_result(
        filepath,
        original_size,
        preview,
        has_more,
    )))
}

fn build_persisted_shell_output_result(
    filepath: PathBuf,
    original_size: u64,
    preview: String,
    has_more: bool,
) -> PersistedToolResult {
    PersistedToolResult {
        filepath,
        original_size: usize::try_from(original_size).unwrap_or(usize::MAX),
        is_json: false,
        preview,
        has_more,
    }
}

#[must_use]
pub fn prepare_stdout_for_display(
    stdout: &str,
    max_chars: usize,
    tool_results_dir: Option<&Path>,
    persist_id: &str,
) -> String {
    if stdout.chars().count() <= max_chars {
        return stdout.to_owned();
    }

    match persist_large_shell_output(tool_results_dir, persist_id, stdout) {
        Ok(Some(persisted)) => build_large_tool_result_message(&persisted),
        Ok(None) | Err(_) => truncate_output(stdout, max_chars),
    }
}

#[must_use]
pub fn prepare_stdout_for_display_from_file(
    stdout_preview: &str,
    stdout_size: u64,
    stdout_file_path: &Path,
    max_chars: usize,
    tool_results_dir: Option<&Path>,
    persist_id: &str,
) -> String {
    if stdout_size <= max_chars as u64 {
        return stdout_preview.to_owned();
    }

    match persist_large_shell_output_file(
        tool_results_dir,
        persist_id,
        stdout_file_path,
        stdout_size,
        shell_preview_content(stdout_preview).as_ref(),
    ) {
        Ok(Some(persisted)) => build_large_tool_result_message(&persisted),
        Ok(None) | Err(_) => truncate_output(stdout_preview, max_chars),
    }
}

#[must_use]
pub fn format_shell_result(
    command: &str,
    description: Option<&str>,
    cwd: &Path,
    analysis: &ShellCommandAnalysis,
    summary: &ShellOutputSummary,
) -> String {
    let mut sections = vec![
        format!("command: {command}"),
        format!("cwd: {}", cwd.display()),
        format!("semantic: {:?}", analysis.semantic).to_ascii_lowercase(),
        format!("read_only: {}", analysis.read_only),
    ];
    if let Some(description) = description.filter(|value| !value.trim().is_empty()) {
        sections.push(format!("description: {}", description.trim()));
    }
    if let Some(exit_code) = summary.exit_code {
        sections.push(format!("exit_code: {exit_code}"));
    }
    if summary.timed_out {
        sections.push("timed_out: true".to_owned());
    }
    if !summary.stdout.trim().is_empty() {
        sections.push(format!("stdout:\n{}", summary.stdout.trim_end()));
    }
    if !summary.stderr.trim().is_empty() {
        sections.push(format!("stderr:\n{}", summary.stderr.trim_end()));
    }
    sections.join("\n\n")
}

fn byte_prefix_at_char_boundary(content: &str, max_bytes: usize) -> &str {
    if content.len() <= max_bytes {
        return content;
    }
    let mut boundary = max_bytes;
    while boundary > 0 && !content.is_char_boundary(boundary) {
        boundary -= 1;
    }
    &content[..boundary]
}

fn shell_preview_content(content: &str) -> std::borrow::Cow<'_, str> {
    let mut start = 0usize;
    let bytes = content.as_bytes();
    while start < bytes.len() {
        let line_start = start;
        while start < bytes.len() && bytes[start] != b'\n' {
            start += 1;
        }
        let line_end = start;
        if start < bytes.len() && bytes[start] == b'\n' {
            start += 1;
        }
        if !content[line_start..line_end].trim().is_empty() {
            return std::borrow::Cow::Owned(content[line_start..].trim_end().to_owned());
        }
    }
    std::borrow::Cow::Borrowed("")
}

pub fn read_shell_output_preview(path: &Path, max_chars: usize) -> Result<String> {
    let preview_bytes = max_chars.saturating_mul(4).saturating_add(4);
    let mut file =
        fs::File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let mut buffer = vec![0u8; preview_bytes];
    let bytes_read = file
        .read(&mut buffer)
        .with_context(|| format!("failed to read {}", path.display()))?;
    buffer.truncate(bytes_read);
    Ok(utf8_prefix(&buffer)
        .map(|text| text.chars().take(max_chars).collect::<String>())
        .unwrap_or_default())
}

pub fn output_size(path: &Path) -> Result<u64> {
    Ok(fs::metadata(path)
        .with_context(|| format!("failed to stat {}", path.display()))?
        .len())
}

fn utf8_prefix(bytes: &[u8]) -> Option<&str> {
    for boundary in (0..=bytes.len()).rev() {
        if let Ok(text) = std::str::from_utf8(&bytes[..boundary]) {
            return Some(text);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{
        ShellOutputSummary, format_shell_result, persist_large_shell_output_with_cap,
        prepare_stdout_for_display, truncate_output,
    };
    use crate::shell::semantics::{ShellCommandAnalysis, ShellCommandSemantic};
    use tempfile::tempdir;

    #[test]
    fn truncate_output_marks_large_content() {
        let value = "hello world";
        assert!(truncate_output(value, 5).contains("[truncated]"));
    }

    #[test]
    fn format_shell_result_includes_core_fields() {
        let analysis = ShellCommandAnalysis {
            semantic: ShellCommandSemantic::ReadOnly,
            read_only: true,
            background: false,
            destructive_git: false,
            dangerous: false,
            changes_directory: false,
            security_flags: vec![],
        };
        let summary = ShellOutputSummary {
            exit_code: Some(0),
            stdout: "ok".to_owned(),
            stderr: String::new(),
            timed_out: false,
            artifact_path: Some(PathBuf::from("artifact.log")),
        };
        let rendered = format_shell_result(
            "pwd",
            Some("show current directory"),
            PathBuf::from(".").as_path(),
            &analysis,
            &summary,
        );
        assert!(rendered.contains("command: pwd"));
        assert!(rendered.contains("description: show current directory"));
        assert!(!rendered.contains("artifact: artifact.log"));
    }

    #[test]
    fn prepare_stdout_for_display_wraps_large_output_with_persisted_message() {
        let tempdir = tempdir().expect("tempdir");
        let rendered = prepare_stdout_for_display(
            &"line\n".repeat(8_000),
            30_000,
            Some(tempdir.path()),
            "shell-call",
        );
        assert!(rendered.starts_with("<persisted-output>"));
        assert!(tempdir.path().join("shell-call.txt").exists());
    }

    #[test]
    fn persist_large_shell_output_caps_written_file_but_keeps_original_size() {
        let tempdir = tempdir().expect("tempdir");
        let persisted = persist_large_shell_output_with_cap(
            Some(tempdir.path()),
            "shell-call",
            &"x".repeat(128),
            "x",
            32,
        )
        .expect("persist")
        .expect("result");
        let on_disk = std::fs::read_to_string(tempdir.path().join("shell-call.txt")).expect("read");
        assert_eq!(on_disk.len(), 32);
        assert_eq!(persisted.original_size, 128);
    }
}
