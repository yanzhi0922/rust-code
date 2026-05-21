use std::env;
use std::path::{Path, PathBuf};

use serde::Serialize;

pub(crate) const PACKAGE_REPOSITORY_URL: &str = env!("CARGO_PKG_REPOSITORY");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum InstallSourceKind {
    CargoInstall,
    CargoTarget,
    GitCheckout,
    Standalone,
    Unknown,
}

impl InstallSourceKind {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::CargoInstall => "cargo install",
            Self::CargoTarget => "cargo target build",
            Self::GitCheckout => "git checkout",
            Self::Standalone => "standalone binary",
            Self::Unknown => "unknown",
        }
    }

    pub(crate) fn supports_in_place_update(self) -> bool {
        matches!(self, Self::CargoInstall | Self::Standalone)
    }
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct InstallSource {
    pub kind: InstallSourceKind,
    pub executable: PathBuf,
    pub repository_url: String,
    pub repository_slug: Option<String>,
}

impl InstallSource {
    pub(crate) fn label(&self) -> &'static str {
        self.kind.label()
    }

    pub(crate) fn supports_in_place_update(&self) -> bool {
        self.kind.supports_in_place_update()
    }
}

pub(crate) fn detect_install_source() -> InstallSource {
    let executable = env::current_exe().unwrap_or_else(|_| {
        env::args()
            .next()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("remote-code"))
    });
    detect_install_source_from_path(&executable)
}

pub(crate) fn detect_install_source_from_path(executable: &Path) -> InstallSource {
    InstallSource {
        kind: detect_install_source_kind(executable),
        executable: executable.to_path_buf(),
        repository_url: PACKAGE_REPOSITORY_URL.to_owned(),
        repository_slug: release_repository_slug(),
    }
}

pub(crate) fn release_repository_slug() -> Option<String> {
    github_repo_slug_from_repository_url(PACKAGE_REPOSITORY_URL)
}

pub(crate) fn github_repo_slug_from_repository_url(url: &str) -> Option<String> {
    let trimmed = url.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }

    let stripped = trimmed
        .strip_prefix("https://github.com/")
        .or_else(|| trimmed.strip_prefix("http://github.com/"))
        .or_else(|| trimmed.strip_prefix("git@github.com:"))?;
    let slug = stripped.trim_end_matches(".git").trim_matches('/');
    let mut parts = slug.split('/');
    let owner = parts.next()?.trim();
    let repo = parts.next()?.trim();
    if owner.is_empty() || repo.is_empty() || parts.next().is_some() {
        return None;
    }
    Some(format!("{owner}/{repo}"))
}

fn detect_install_source_kind(executable: &Path) -> InstallSourceKind {
    if is_cargo_install_path(executable) {
        return InstallSourceKind::CargoInstall;
    }
    if looks_like_cargo_target_path(executable) {
        return InstallSourceKind::CargoTarget;
    }
    if has_git_checkout_ancestor(executable) {
        return InstallSourceKind::GitCheckout;
    }
    if executable.exists() {
        InstallSourceKind::Standalone
    } else {
        InstallSourceKind::Unknown
    }
}

fn is_cargo_install_path(executable: &Path) -> bool {
    let Some(parent) = executable.parent() else {
        return false;
    };

    let configured_home = env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(default_cargo_home);
    configured_home
        .map(|home| parent == home.join("bin"))
        .unwrap_or(false)
}

fn default_cargo_home() -> Option<PathBuf> {
    env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".cargo")))
        .or_else(|| env::var_os("USERPROFILE").map(|home| PathBuf::from(home).join(".cargo")))
}

fn looks_like_cargo_target_path(executable: &Path) -> bool {
    let components = executable
        .components()
        .map(|component| component.as_os_str().to_string_lossy().to_string())
        .collect::<Vec<_>>();
    components
        .windows(2)
        .any(|window| window[0] == "target" && matches!(window[1].as_str(), "debug" | "release"))
}

fn has_git_checkout_ancestor(executable: &Path) -> bool {
    executable
        .ancestors()
        .skip(1)
        .any(|ancestor| ancestor.join(".git").exists())
}

#[cfg(test)]
mod tests {
    use super::{
        InstallSourceKind, detect_install_source_from_path, github_repo_slug_from_repository_url,
    };
    use std::fs;
    use std::path::Path;
    use tempfile::tempdir;

    #[test]
    fn github_repository_slug_is_parsed_from_https_and_ssh_urls() {
        assert_eq!(
            github_repo_slug_from_repository_url("https://github.com/yanzhi0922/remote-code-rust"),
            Some("yanzhi0922/remote-code-rust".to_owned())
        );
        assert_eq!(
            github_repo_slug_from_repository_url("git@github.com:yanzhi0922/remote-code-rust.git"),
            Some("yanzhi0922/remote-code-rust".to_owned())
        );
        assert_eq!(
            github_repo_slug_from_repository_url("https://example.com/repo"),
            None
        );
    }

    #[test]
    fn cargo_target_install_source_is_detected_from_path_shape() {
        let source = detect_install_source_from_path(Path::new(
            "C:/Users/test/remote-code-rust/target/debug/remote-code.exe",
        ));
        assert_eq!(source.kind, InstallSourceKind::CargoTarget);
        assert!(!source.supports_in_place_update());
    }

    #[test]
    fn git_checkout_install_source_is_detected_from_ancestor() {
        let temp = tempdir().expect("tempdir should work");
        fs::create_dir_all(temp.path().join(".git")).expect("git dir create should work");
        let exe = temp.path().join("bin").join("remote-code");
        fs::create_dir_all(exe.parent().expect("exe parent should exist"))
            .expect("bin dir create should work");
        fs::write(&exe, b"binary").expect("exe write should work");

        let source = detect_install_source_from_path(&exe);
        assert_eq!(source.kind, InstallSourceKind::GitCheckout);
        assert!(!source.supports_in_place_update());
    }
}
