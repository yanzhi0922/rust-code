//! Filesystem permission helpers for tool path authorization.
//!
//! This is the Rust-side analogue of Claude Code's path permission pipeline:
//! raw input validation, symlink-aware destination checking, dangerous edit
//! detection, and working-directory/additional-directory allowlists.

use std::collections::HashSet;
use std::path::{MAIN_SEPARATOR, Path, PathBuf};

use claude_core::PermissionMode;
use claude_core::permission_types::PermissionBehavior;

use crate::decision::{PermissionUpdate, PermissionUpdateDestination};
use crate::mode::ExtendedPermissionMode;
use crate::path_validation::{
    PathValidation, clean_path_input, path_requires_manual_approval, validate_path,
};
use crate::rule::PermissionRuleValue;

/// Filesystem operation kind used for permission checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilesystemOperation {
    Read,
    Write,
    Create,
}

/// Result of a filesystem permission check.
#[derive(Debug, Clone)]
pub struct FilesystemCheckResult {
    /// Whether the operation can proceed without an explicit permission prompt.
    pub allowed: bool,
    /// Whether the caller should route through the interactive permission flow.
    pub requires_confirmation: bool,
    /// Reason for a prompt or denial.
    pub reason: Option<String>,
    /// Normalized absolute path used for the decision.
    pub normalized_path: PathBuf,
    /// All path forms considered during the decision (original, symlinks, real path).
    pub checked_paths: Vec<PathBuf>,
    /// Structured suggestion payloads for permission UIs / SDK hosts.
    pub suggestions: Vec<PermissionUpdate>,
    /// Machine-readable cause for the decision.
    pub cause: FilesystemCheckCause,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPath {
    pub resolved_path: PathBuf,
    pub is_symlink: bool,
    pub is_canonical: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilesystemCheckCause {
    Allowed,
    OutsideWorkingDirectories,
    ManualApprovalRequired,
    DangerousEdit,
    InvalidPath,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FilesystemAccessOptions {
    pub additional_dirs: Vec<PathBuf>,
    pub current_mode: Option<PermissionMode>,
    pub plan_file: Option<PathBuf>,
    pub internal_read_dirs: Vec<PathBuf>,
    pub internal_write_dirs: Vec<PathBuf>,
    pub internal_read_files: Vec<PathBuf>,
    pub internal_write_files: Vec<PathBuf>,
}

const DANGEROUS_FILES: &[&str] = &[
    ".gitconfig",
    ".gitmodules",
    ".bashrc",
    ".bash_profile",
    ".zshrc",
    ".zprofile",
    ".profile",
    ".ripgreprc",
    ".mcp.json",
    ".claude.json",
];

const DANGEROUS_DIRECTORIES: &[&str] = &[".git", ".vscode", ".idea", ".claude"];

/// Backward-compatible wrapper that checks whether a path sits inside the
/// current working directory or an explicitly allowed extra directory.
#[must_use]
pub fn check_filesystem_permission(
    path: &str,
    cwd: &str,
    additional_dirs: &[String],
) -> FilesystemCheckResult {
    let cwd = PathBuf::from(cwd);
    let options = FilesystemAccessOptions {
        additional_dirs: additional_dirs.iter().map(PathBuf::from).collect(),
        current_mode: None,
        ..FilesystemAccessOptions::default()
    };
    assess_filesystem_access(path, &cwd, &options, FilesystemOperation::Read)
}

/// Main path permission entry point used by the tool runtime.
#[must_use]
pub fn assess_filesystem_access(
    path: &str,
    cwd: &Path,
    options: &FilesystemAccessOptions,
    operation: FilesystemOperation,
) -> FilesystemCheckResult {
    let cleaned = clean_path_input(path);
    let normalized_path = resolve_candidate_path(cwd, Some(cleaned.as_str()));

    match validate_path(&cleaned) {
        PathValidation::Valid => {}
        PathValidation::Invalid(reason) => {
            return deny(normalized_path, reason, FilesystemCheckCause::InvalidPath);
        }
    }

    let checked_paths = get_paths_for_permission_check(&normalized_path);

    if is_plan_file_path(&checked_paths, options.plan_file.as_deref()) {
        return allow(normalized_path, checked_paths);
    }

    if internal_path_allowed(&checked_paths, operation, options) {
        return allow(normalized_path, checked_paths);
    }

    if let Some(reason) =
        path_requires_manual_approval(&cleaned, !matches!(operation, FilesystemOperation::Read))
    {
        let suggestions =
            generate_suggestions(&normalized_path, operation, cwd, options, &checked_paths);
        return ask(
            normalized_path,
            checked_paths,
            reason,
            suggestions,
            FilesystemCheckCause::ManualApprovalRequired,
        );
    }

    if matches!(
        operation,
        FilesystemOperation::Write | FilesystemOperation::Create
    ) && let Some(reason) = check_path_safety_for_auto_edit(&checked_paths)
    {
        let suggestions =
            generate_suggestions(&normalized_path, operation, cwd, options, &checked_paths);
        return ask(
            normalized_path,
            checked_paths,
            reason,
            suggestions,
            FilesystemCheckCause::DangerousEdit,
        );
    }

    if path_in_allowed_working_path(&checked_paths, cwd, &options.additional_dirs) {
        return allow(normalized_path, checked_paths);
    }

    let suggestions =
        generate_suggestions(&normalized_path, operation, cwd, options, &checked_paths);
    ask(
        normalized_path,
        checked_paths,
        "Path is outside the allowed working directories.".to_owned(),
        suggestions,
        FilesystemCheckCause::OutsideWorkingDirectories,
    )
}

/// Resolve a user-provided path against the current working directory.
#[must_use]
pub fn resolve_candidate_path(cwd: &Path, maybe_relative: Option<&str>) -> PathBuf {
    let candidate = match maybe_relative {
        Some(path) if !path.trim().is_empty() => {
            let candidate = PathBuf::from(clean_path_input(path));
            if candidate.is_absolute() {
                candidate
            } else {
                cwd.join(candidate)
            }
        }
        _ => cwd.to_path_buf(),
    };

    normalize_path_lexically(&candidate)
}

/// Normalize a path for case-insensitive comparison.
#[must_use]
pub fn normalize_for_comparison(path: &Path) -> String {
    let lexically_normalized = normalize_path_lexically(path);
    let raw = lexically_normalized.to_string_lossy();
    let stripped = raw.strip_prefix(r"\\?\").unwrap_or(&raw);
    let mut normalized = stripped.replace('\\', "/").to_ascii_lowercase();

    while normalized.len() > 1 && normalized.ends_with('/') && !is_root_like(&normalized) {
        normalized.pop();
    }

    normalized
}

/// Normalize a path without requiring it to exist.
///
/// This mirrors Node's lexical `resolve/normalize` behavior closely enough for
/// permission checks, so paths like `../outside.txt` are compared against the
/// real working-directory boundary instead of being treated as invalid or
/// accidentally left under the workspace prefix.
#[must_use]
pub fn normalize_path_lexically(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    let mut has_root = false;

    for component in path.components() {
        match component {
            std::path::Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            std::path::Component::RootDir => {
                has_root = true;
                normalized.push(std::path::MAIN_SEPARATOR.to_string());
            }
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if normalized.file_name().is_some() {
                    normalized.pop();
                } else if !has_root {
                    normalized.push("..");
                }
            }
            std::path::Component::Normal(segment) => normalized.push(segment),
        }
    }

    if normalized.as_os_str().is_empty() && has_root {
        PathBuf::from(std::path::MAIN_SEPARATOR.to_string())
    } else if normalized.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        normalized
    }
}

/// Resolve a path without failing on non-existent targets or broken symlinks.
#[must_use]
pub fn safe_resolve_path(path: &Path) -> ResolvedPath {
    if is_unc_path(path) {
        return ResolvedPath {
            resolved_path: path.to_path_buf(),
            is_symlink: false,
            is_canonical: false,
        };
    }

    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(_) => {
            return ResolvedPath {
                resolved_path: path.to_path_buf(),
                is_symlink: false,
                is_canonical: false,
            };
        }
    };

    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;

        let file_type = metadata.file_type();
        if file_type.is_fifo()
            || file_type.is_socket()
            || file_type.is_char_device()
            || file_type.is_block_device()
        {
            return ResolvedPath {
                resolved_path: path.to_path_buf(),
                is_symlink: false,
                is_canonical: false,
            };
        }
    }

    match std::fs::canonicalize(path) {
        Ok(resolved_path) => ResolvedPath {
            is_symlink: normalize_for_comparison(&resolved_path) != normalize_for_comparison(path),
            resolved_path,
            is_canonical: true,
        },
        Err(_) => ResolvedPath {
            resolved_path: path.to_path_buf(),
            is_symlink: metadata.file_type().is_symlink(),
            is_canonical: false,
        },
    }
}

/// Resolve the deepest existing ancestor of a path, preserving non-existent tail segments.
#[must_use]
pub fn resolve_deepest_existing_ancestor(path: &Path) -> Option<PathBuf> {
    let mut current = path.to_path_buf();
    let mut tail_segments = Vec::new();

    loop {
        let metadata = match std::fs::symlink_metadata(&current) {
            Ok(metadata) => metadata,
            Err(_) => {
                let parent = current.parent()?;
                if parent == current {
                    return None;
                }
                if let Some(name) = current.file_name() {
                    tail_segments.push(name.to_owned());
                }
                current = parent.to_path_buf();
                continue;
            }
        };

        if metadata.file_type().is_symlink() {
            if let Ok(resolved) = std::fs::canonicalize(&current) {
                return Some(rejoin_tail(resolved, &tail_segments));
            }
            if let Ok(target) = std::fs::read_link(&current) {
                let absolute_target = if target.is_absolute() {
                    target
                } else {
                    current
                        .parent()
                        .unwrap_or_else(|| Path::new("."))
                        .join(target)
                };
                return Some(rejoin_tail(absolute_target, &tail_segments));
            }
            return None;
        }

        if let Ok(resolved) = std::fs::canonicalize(&current) {
            let rejoined = rejoin_tail(resolved.clone(), &tail_segments);
            if normalize_for_comparison(&rejoined) != normalize_for_comparison(path) {
                return Some(rejoined);
            }
        }
        return None;
    }
}

/// Collect all path forms relevant to a permission check.
#[must_use]
pub fn get_paths_for_permission_check(path: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    push_unique_path(&mut paths, path.to_path_buf());

    if is_unc_path(path) {
        return paths;
    }

    let mut current = path.to_path_buf();
    let mut visited = HashSet::new();
    for _ in 0..40 {
        let current_key = normalize_for_comparison(&current);
        if !visited.insert(current_key) {
            break;
        }

        if !current.exists() {
            if normalize_for_comparison(&current) == normalize_for_comparison(path)
                && let Some(resolved) = resolve_deepest_existing_ancestor(path)
            {
                push_unique_path(&mut paths, resolved);
            }
            break;
        }

        let metadata = match std::fs::symlink_metadata(&current) {
            Ok(metadata) => metadata,
            Err(_) => break,
        };

        if !metadata.file_type().is_symlink() {
            break;
        }

        let target = match std::fs::read_link(&current) {
            Ok(target) => target,
            Err(_) => break,
        };

        let absolute_target = if target.is_absolute() {
            target
        } else {
            current
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join(target)
        };

        push_unique_path(&mut paths, absolute_target.clone());
        current = absolute_target;
    }

    let resolved = safe_resolve_path(path);
    if resolved.is_symlink
        && normalize_for_comparison(&resolved.resolved_path) != normalize_for_comparison(path)
    {
        push_unique_path(&mut paths, resolved.resolved_path);
    }

    paths
}

/// Check whether a path sits inside a single allowed root.
#[must_use]
pub fn path_in_working_path(path: &Path, root: &Path) -> bool {
    let normalized_path = normalize_for_comparison(path);
    let normalized_root = normalize_for_comparison(root);

    normalized_path == normalized_root
        || normalized_path.starts_with(&(ensure_trailing_separator(&normalized_root)))
}

/// Check whether every checked path form sits inside the cwd or one of the additional roots.
#[must_use]
pub fn path_in_allowed_working_path(
    checked_paths: &[PathBuf],
    cwd: &Path,
    additional_dirs: &[PathBuf],
) -> bool {
    let mut roots = vec![cwd.to_path_buf()];
    roots.extend(additional_dirs.iter().cloned());

    let root_forms = roots
        .iter()
        .map(|root| get_paths_for_permission_check(root))
        .collect::<Vec<_>>();

    checked_paths.iter().all(|path| {
        root_forms
            .iter()
            .any(|forms| forms.iter().any(|root| path_in_working_path(path, root)))
    })
}

fn is_plan_file_path(checked_paths: &[PathBuf], plan_file: Option<&Path>) -> bool {
    let Some(plan_file) = plan_file else {
        return false;
    };
    let plan_forms = get_paths_for_permission_check(plan_file);
    checked_paths.iter().any(|checked| {
        plan_forms
            .iter()
            .any(|plan| normalize_for_comparison(checked) == normalize_for_comparison(plan))
    })
}

fn internal_path_allowed(
    checked_paths: &[PathBuf],
    operation: FilesystemOperation,
    options: &FilesystemAccessOptions,
) -> bool {
    match operation {
        FilesystemOperation::Read => {
            path_matches_any_file(checked_paths, &options.internal_read_files)
                || path_matches_any_root(checked_paths, &options.internal_read_dirs)
        }
        FilesystemOperation::Write | FilesystemOperation::Create => {
            path_matches_any_file(checked_paths, &options.internal_write_files)
                || path_matches_any_root(checked_paths, &options.internal_write_dirs)
        }
    }
}

fn path_matches_any_file(checked_paths: &[PathBuf], files: &[PathBuf]) -> bool {
    checked_paths.iter().any(|checked| {
        files.iter().any(|file| {
            get_paths_for_permission_check(file)
                .iter()
                .any(|candidate| {
                    normalize_for_comparison(checked) == normalize_for_comparison(candidate)
                })
        })
    })
}

fn path_matches_any_root(checked_paths: &[PathBuf], roots: &[PathBuf]) -> bool {
    if roots.is_empty() {
        return false;
    }
    checked_paths.iter().all(|checked| {
        roots.iter().any(|root| {
            get_paths_for_permission_check(root)
                .iter()
                .any(|candidate| path_in_working_path(checked, candidate))
        })
    })
}

fn check_path_safety_for_auto_edit(checked_paths: &[PathBuf]) -> Option<String> {
    for path in checked_paths {
        if is_claude_config_file_path(path) {
            return Some("Editing Claude settings files requires manual approval.".to_owned());
        }
        if is_dangerous_file_path_to_auto_edit(path) {
            return Some(
                "Editing dangerous configuration paths requires manual approval.".to_owned(),
            );
        }
    }
    None
}

fn is_claude_config_file_path(path: &Path) -> bool {
    let normalized = normalize_for_comparison(path);
    normalized.ends_with("/.claude/settings.json")
        || normalized.ends_with("/.claude/settings.local.json")
}

fn is_dangerous_file_path_to_auto_edit(path: &Path) -> bool {
    let normalized = normalize_for_comparison(path);
    if normalized.starts_with("//") {
        return true;
    }

    let segments = normalized
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();

    for (index, segment) in segments.iter().enumerate() {
        if !DANGEROUS_DIRECTORIES.contains(segment) {
            continue;
        }
        if *segment == ".claude"
            && segments
                .get(index + 1)
                .is_some_and(|next| *next == "worktrees")
        {
            continue;
        }
        return true;
    }

    segments
        .last()
        .is_some_and(|segment| DANGEROUS_FILES.contains(segment))
}

fn generate_suggestions(
    path: &Path,
    operation: FilesystemOperation,
    cwd: &Path,
    options: &FilesystemAccessOptions,
    checked_paths: &[PathBuf],
) -> Vec<PermissionUpdate> {
    let outside_working_dir =
        !path_in_allowed_working_path(checked_paths, cwd, &options.additional_dirs);
    let should_suggest_accept_edits = matches!(
        options.current_mode.unwrap_or(PermissionMode::Default),
        PermissionMode::Default | PermissionMode::Plan
    );
    match operation {
        FilesystemOperation::Read if outside_working_dir => {
            let dirs_to_add = path
                .parent()
                .map(get_paths_for_permission_check)
                .unwrap_or_default();
            dirs_to_add
                .into_iter()
                .map(|dir| PermissionUpdate::AddRules {
                    destination: PermissionUpdateDestination::Session,
                    rules: vec![PermissionRuleValue {
                        tool_name: "Read".to_owned(),
                        rule_content: Some(permission_rule_path_pattern(&dir)),
                    }],
                    behavior: PermissionBehavior::Allow,
                })
                .collect()
        }
        FilesystemOperation::Write | FilesystemOperation::Create => {
            let mut updates = if should_suggest_accept_edits {
                vec![PermissionUpdate::SetMode {
                    destination: PermissionUpdateDestination::Session,
                    mode: ExtendedPermissionMode::AcceptEdits,
                }]
            } else {
                Vec::new()
            };
            if outside_working_dir {
                let dirs_to_add = path
                    .parent()
                    .map(get_paths_for_permission_check)
                    .unwrap_or_default();
                updates.push(PermissionUpdate::AddDirectories {
                    destination: PermissionUpdateDestination::Session,
                    directories: dirs_to_add
                        .into_iter()
                        .map(|dir| dir.to_string_lossy().into_owned())
                        .collect(),
                });
            }
            updates
        }
        _ if should_suggest_accept_edits => vec![PermissionUpdate::SetMode {
            destination: PermissionUpdateDestination::Session,
            mode: ExtendedPermissionMode::AcceptEdits,
        }],
        _ => Vec::new(),
    }
}

fn permission_rule_path_pattern(path: &Path) -> String {
    let mut rendered = normalize_for_comparison(path);
    if MAIN_SEPARATOR == '\\' {
        rendered = rendered.replace('\\', "/");
    }
    if rendered.ends_with('/') {
        format!("{rendered}**")
    } else {
        format!("{rendered}/**")
    }
}

fn is_root_like(path: &str) -> bool {
    path == "/" || (path.len() == 3 && path.as_bytes()[1] == b':' && path.ends_with('/'))
}

fn ensure_trailing_separator(path: &str) -> String {
    if path.ends_with('/') {
        path.to_owned()
    } else {
        format!("{path}/")
    }
}

fn is_unc_path(path: &Path) -> bool {
    let rendered = path.to_string_lossy();
    rendered.starts_with(r"\\") || rendered.starts_with("//")
}

fn rejoin_tail(mut base: PathBuf, tail_segments: &[std::ffi::OsString]) -> PathBuf {
    for segment in tail_segments.iter().rev() {
        base.push(segment);
    }
    base
}

fn push_unique_path(paths: &mut Vec<PathBuf>, path: PathBuf) {
    let normalized = normalize_for_comparison(&path);
    if paths
        .iter()
        .any(|existing| normalize_for_comparison(existing) == normalized)
    {
        return;
    }
    paths.push(path);
}

fn allow(normalized_path: PathBuf, checked_paths: Vec<PathBuf>) -> FilesystemCheckResult {
    FilesystemCheckResult {
        allowed: true,
        requires_confirmation: false,
        reason: None,
        normalized_path,
        checked_paths,
        suggestions: Vec::new(),
        cause: FilesystemCheckCause::Allowed,
    }
}

fn ask(
    normalized_path: PathBuf,
    checked_paths: Vec<PathBuf>,
    reason: String,
    suggestions: Vec<PermissionUpdate>,
    cause: FilesystemCheckCause,
) -> FilesystemCheckResult {
    FilesystemCheckResult {
        allowed: false,
        requires_confirmation: true,
        reason: Some(reason),
        normalized_path,
        checked_paths,
        suggestions,
        cause,
    }
}

fn deny(
    normalized_path: PathBuf,
    reason: String,
    cause: FilesystemCheckCause,
) -> FilesystemCheckResult {
    FilesystemCheckResult {
        allowed: false,
        requires_confirmation: false,
        reason: Some(reason),
        normalized_path,
        checked_paths: Vec::new(),
        suggestions: Vec::new(),
        cause,
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[cfg(unix)]
    fn symlink_dir(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::unix::fs::symlink(target, link)
    }

    #[cfg(windows)]
    fn symlink_dir(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::windows::fs::symlink_dir(target, link)
    }

    #[test]
    fn allow_within_cwd() {
        let tempdir = tempdir().expect("tempdir");
        let result = assess_filesystem_access(
            "src/main.rs",
            tempdir.path(),
            &FilesystemAccessOptions::default(),
            FilesystemOperation::Read,
        );
        assert!(result.allowed);
    }

    #[test]
    fn ask_outside_cwd() {
        let tempdir = tempdir().expect("tempdir");
        let outside = tempdir.path().parent().expect("parent").join("outside.txt");
        let result = assess_filesystem_access(
            outside.to_string_lossy().as_ref(),
            tempdir.path(),
            &FilesystemAccessOptions::default(),
            FilesystemOperation::Read,
        );
        assert!(!result.allowed);
        assert!(result.requires_confirmation);
        assert!(!result.suggestions.is_empty());
    }

    #[test]
    fn allow_additional_dir() {
        let tempdir = tempdir().expect("tempdir");
        let extra = tempdir.path().join("extra");
        std::fs::create_dir_all(&extra).expect("extra");
        let target = extra.join("file.txt");

        let result = assess_filesystem_access(
            target.to_string_lossy().as_ref(),
            tempdir.path(),
            &FilesystemAccessOptions {
                additional_dirs: vec![extra],
                ..FilesystemAccessOptions::default()
            },
            FilesystemOperation::Read,
        );
        assert!(result.allowed);
    }

    #[test]
    fn dangerous_write_requires_confirmation() {
        let tempdir = tempdir().expect("tempdir");
        let target = tempdir.path().join(".git").join("config");
        let result = assess_filesystem_access(
            target.to_string_lossy().as_ref(),
            tempdir.path(),
            &FilesystemAccessOptions::default(),
            FilesystemOperation::Write,
        );
        assert!(!result.allowed);
        assert!(result.requires_confirmation);
        assert!(result.reason.is_some());
    }

    #[test]
    fn claude_worktrees_are_not_dangerous_auto_edit_path() {
        let tempdir = tempdir().expect("tempdir");
        let target = tempdir
            .path()
            .join(".claude")
            .join("worktrees")
            .join("feature")
            .join("notes.md");
        let result = assess_filesystem_access(
            target.to_string_lossy().as_ref(),
            tempdir.path(),
            &FilesystemAccessOptions::default(),
            FilesystemOperation::Write,
        );
        assert!(result.allowed, "{:?}", result.reason);
    }

    #[test]
    fn nested_claude_inside_worktree_still_requires_confirmation() {
        let tempdir = tempdir().expect("tempdir");
        let target = tempdir
            .path()
            .join(".claude")
            .join("worktrees")
            .join("feature")
            .join(".claude")
            .join("settings.json");
        let result = assess_filesystem_access(
            target.to_string_lossy().as_ref(),
            tempdir.path(),
            &FilesystemAccessOptions::default(),
            FilesystemOperation::Write,
        );
        assert!(!result.allowed);
        assert!(result.requires_confirmation);
    }

    #[test]
    fn invalid_path_is_denied() {
        let tempdir = tempdir().expect("tempdir");
        let result = assess_filesystem_access(
            "file\0.txt",
            tempdir.path(),
            &FilesystemAccessOptions::default(),
            FilesystemOperation::Read,
        );
        assert!(!result.allowed);
        assert!(!result.requires_confirmation);
    }

    #[test]
    fn active_plan_file_is_allowed_outside_workspace() {
        let tempdir = tempdir().expect("tempdir");
        let workspace = tempdir.path().join("workspace");
        let profile = tempdir.path().join(".remote-code-rust");
        let plan_file = profile.join("plans").join("plan.md");
        std::fs::create_dir_all(&workspace).expect("workspace");
        std::fs::create_dir_all(plan_file.parent().expect("plan dir")).expect("plan dir");

        let result = assess_filesystem_access(
            plan_file.to_string_lossy().as_ref(),
            &workspace,
            &FilesystemAccessOptions {
                plan_file: Some(plan_file.clone()),
                ..FilesystemAccessOptions::default()
            },
            FilesystemOperation::Write,
        );
        assert!(result.allowed);
    }

    #[test]
    fn internal_write_dir_is_allowed_before_dangerous_claude_check() {
        let tempdir = tempdir().expect("tempdir");
        let workspace = tempdir.path().join("workspace");
        let internal = tempdir.path().join(".claude").join("agent-memory");
        let target = internal.join("reviewer").join("MEMORY.md");
        std::fs::create_dir_all(&workspace).expect("workspace");

        let result = assess_filesystem_access(
            target.to_string_lossy().as_ref(),
            &workspace,
            &FilesystemAccessOptions {
                internal_write_dirs: vec![internal],
                ..FilesystemAccessOptions::default()
            },
            FilesystemOperation::Write,
        );
        assert!(result.allowed, "{:?}", result.reason);
    }

    #[test]
    fn parent_symlink_escape_requires_confirmation() {
        let tempdir = tempdir().expect("tempdir");
        let workspace = tempdir.path().join("workspace");
        let outside = tempdir.path().join("outside");
        std::fs::create_dir_all(&workspace).expect("workspace");
        std::fs::create_dir_all(&outside).expect("outside");

        let link = workspace.join("out");
        if symlink_dir(&outside, &link).is_err() {
            return;
        }

        let result = assess_filesystem_access(
            link.join("new.txt").to_string_lossy().as_ref(),
            &workspace,
            &FilesystemAccessOptions::default(),
            FilesystemOperation::Create,
        );
        assert!(!result.allowed);
        assert!(result.requires_confirmation);
    }

    #[test]
    fn lexical_parent_traversal_routes_to_outside_workspace_confirmation() {
        let tempdir = tempdir().expect("tempdir");
        let workspace = tempdir.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");

        let result = assess_filesystem_access(
            "../outside.txt",
            &workspace,
            &FilesystemAccessOptions::default(),
            FilesystemOperation::Read,
        );
        assert!(!result.allowed);
        assert!(result.requires_confirmation);
        assert_eq!(
            result.cause,
            FilesystemCheckCause::OutsideWorkingDirectories
        );
        assert!(
            result
                .normalized_path
                .to_string_lossy()
                .replace('\\', "/")
                .ends_with("/outside.txt")
        );
    }

    #[test]
    fn write_suggestions_respect_current_mode_for_accept_edits_upgrade() {
        let tempdir = tempdir().expect("tempdir");
        let workspace = tempdir.path().join("workspace");
        let outside = tempdir.path().join("outside").join("file.txt");
        std::fs::create_dir_all(&workspace).expect("workspace");
        std::fs::create_dir_all(outside.parent().expect("outside dir")).expect("outside dir");

        let default_result = assess_filesystem_access(
            outside.to_string_lossy().as_ref(),
            &workspace,
            &FilesystemAccessOptions {
                current_mode: Some(PermissionMode::Default),
                ..FilesystemAccessOptions::default()
            },
            FilesystemOperation::Write,
        );
        assert!(default_result.requires_confirmation);
        assert!(
            default_result
                .suggestions
                .iter()
                .any(|update| matches!(update, PermissionUpdate::SetMode { .. }))
        );

        let auto_result = assess_filesystem_access(
            outside.to_string_lossy().as_ref(),
            &workspace,
            &FilesystemAccessOptions {
                current_mode: Some(PermissionMode::DontAsk),
                ..FilesystemAccessOptions::default()
            },
            FilesystemOperation::Write,
        );
        assert!(auto_result.requires_confirmation);
        assert!(
            !auto_result
                .suggestions
                .iter()
                .any(|update| matches!(update, PermissionUpdate::SetMode { .. }))
        );
    }
}
