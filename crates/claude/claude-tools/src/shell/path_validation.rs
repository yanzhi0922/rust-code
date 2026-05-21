use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};

use super::readonly::ShellKind;

#[must_use]
pub fn command_changes_directory(kind: ShellKind, command: &str) -> bool {
    let normalized = command.trim().to_ascii_lowercase();
    match kind {
        ShellKind::Bash => {
            normalized.starts_with("cd ")
                || normalized.contains("&& cd ")
                || normalized.contains("; cd ")
                || normalized.starts_with("pushd ")
                || normalized.starts_with("popd ")
        }
        ShellKind::PowerShell => {
            normalized.starts_with("cd ")
                || normalized.contains("; cd ")
                || normalized.starts_with("set-location ")
                || normalized.starts_with("push-location ")
                || normalized.starts_with("pop-location ")
        }
    }
}

/// Resolve an optional `cwd` override against the current tool context.
///
/// # Errors
/// Returns an error when the target directory does not exist or is not a directory.
pub fn resolve_working_dir(base: &Path, override_cwd: Option<&str>) -> Result<PathBuf> {
    let path = match override_cwd {
        Some(raw) => {
            let candidate = PathBuf::from(raw);
            if candidate.is_absolute() {
                candidate
            } else {
                base.join(candidate)
            }
        }
        None => base.to_path_buf(),
    };
    if !path.exists() {
        return Err(anyhow!(
            "working directory {} does not exist",
            path.display()
        ));
    }
    if !path.is_dir() {
        return Err(anyhow!(
            "working directory {} is not a directory",
            path.display()
        ));
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::{command_changes_directory, resolve_working_dir};
    use crate::shell::readonly::ShellKind;

    #[test]
    fn detects_inline_cwd_changes() {
        assert!(command_changes_directory(
            ShellKind::Bash,
            "cd src && cargo test"
        ));
        assert!(command_changes_directory(
            ShellKind::PowerShell,
            "Set-Location src; cargo test"
        ));
        assert!(!command_changes_directory(ShellKind::Bash, "cargo test"));
    }

    #[test]
    fn resolve_working_dir_handles_relative_override() {
        let tempdir = tempdir().expect("tempdir");
        let nested = tempdir.path().join("nested");
        std::fs::create_dir_all(&nested).expect("create nested");
        let resolved =
            resolve_working_dir(tempdir.path(), Some("nested")).expect("resolve working dir");
        assert_eq!(resolved, nested);
    }
}
