//! Auto-updater that checks GitHub Releases for new versions.

use anyhow::{Context, Result, anyhow};
use serde::Deserialize;

use crate::doctor::install::{InstallSourceKind, detect_install_source, release_repository_slug};

#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    html_url: String,
    assets: Vec<GitHubAsset>,
}

#[derive(Debug, Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
    #[allow(dead_code)]
    size: u64,
}

/// Result of a version check.
pub struct UpdateCheckResult {
    /// The GitHub owner/repository slug used for release lookup.
    pub repository: String,
    /// The latest available version (e.g. "v1.2.3").
    pub latest_version: String,
    /// URL to the release page.
    pub release_url: String,
    /// Whether the current version is outdated.
    pub update_available: bool,
    /// Download URL for the current platform's binary, if available.
    pub download_url: Option<String>,
    /// Description of how the current executable appears to be installed.
    pub install_source: String,
}

/// Check GitHub Releases for a newer version.
pub async fn check_for_update() -> Result<UpdateCheckResult> {
    let repository = release_repository_slug().ok_or_else(|| {
        anyhow!(
            "package repository `{}` is not a supported GitHub repository URL",
            env!("CARGO_PKG_REPOSITORY")
        )
    })?;
    let release_url = latest_release_api_url(&repository);

    let client = reqwest::Client::builder()
        .user_agent("remote-code-rust-updater")
        .build()?;

    let response = client
        .get(&release_url)
        .send()
        .await
        .context("failed to fetch latest release from GitHub")?;

    if !response.status().is_success() {
        anyhow::bail!(
            "GitHub API returned status {}: unable to check for updates",
            response.status()
        );
    }

    let release: GitHubRelease = response
        .json()
        .await
        .context("failed to parse GitHub release response")?;

    let current = env!("CARGO_PKG_VERSION");
    let latest = release.tag_name.trim_start_matches('v');
    let update_available = is_newer_version(latest, current);
    let install_source = detect_install_source();

    Ok(UpdateCheckResult {
        repository,
        latest_version: release.tag_name.clone(),
        release_url: release.html_url,
        update_available,
        download_url: select_download_asset(&release.assets, &platform_asset_suffix()),
        install_source: install_source.label().to_owned(),
    })
}

/// Run the update check and print results.
pub async fn run_check() -> Result<()> {
    println!("Checking for updates...");

    match check_for_update().await {
        Ok(result) => {
            let current = env!("CARGO_PKG_VERSION");
            println!("Repository: {}", result.repository);
            println!("Install source: {}", result.install_source);
            if result.update_available {
                println!(
                    "✨ Update available: {} (current: v{current})",
                    result.latest_version
                );
                println!("   Release notes: {}", result.release_url);
                if let Some(url) = &result.download_url {
                    println!("   Download: {url}");
                } else {
                    println!("   Download: no asset matched the current platform suffix");
                }
                println!();
                println!("Run `remote-code update run` to install the latest version.");
            } else {
                println!("✅ Already up to date (v{current})");
            }
            Ok(())
        }
        Err(error) => {
            eprintln!("Failed to check for updates: {error:#}");
            Err(error)
        }
    }
}

/// Download and install the latest version.
pub async fn run_update() -> Result<()> {
    println!("Checking for updates...");
    let install_source = detect_install_source();
    ensure_in_place_update_supported(install_source.kind, install_source.executable.as_path())?;

    let result = check_for_update().await?;
    let current = env!("CARGO_PKG_VERSION");
    if !result.update_available {
        println!("✅ Already up to date (v{current})");
        return Ok(());
    }

    let download_url = result.download_url.context(
        "no binary asset matched the current platform. Please download the release manually",
    )?;

    println!("Downloading {}...", result.latest_version);

    let client = reqwest::Client::builder()
        .user_agent("remote-code-rust-updater")
        .build()?;

    let response = client
        .get(&download_url)
        .send()
        .await
        .context("failed to download update")?;

    if !response.status().is_success() {
        anyhow::bail!("download failed with status {}", response.status());
    }

    let bytes = response
        .bytes()
        .await
        .context("failed to read download response")?;

    let current_exe =
        std::env::current_exe().context("failed to determine current executable path")?;
    let temp_path = current_exe.with_extension("new");

    std::fs::write(&temp_path, &bytes).context("failed to write new binary")?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&temp_path)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&temp_path, perms)?;
    }

    #[cfg(windows)]
    {
        let old_path = current_exe.with_extension("old");
        std::fs::rename(&current_exe, &old_path).context("failed to rename current executable")?;
        std::fs::rename(&temp_path, &current_exe).context("failed to install new version")?;
        let _ = std::fs::remove_file(&old_path);
    }

    #[cfg(not(windows))]
    {
        std::fs::rename(&temp_path, &current_exe).context("failed to install new version")?;
    }

    println!("✅ Updated to {} successfully!", result.latest_version);
    Ok(())
}

fn latest_release_api_url(repository: &str) -> String {
    format!("https://api.github.com/repos/{repository}/releases/latest")
}

fn select_download_asset(assets: &[GitHubAsset], target_suffix: &str) -> Option<String> {
    assets
        .iter()
        .find(|asset| asset.name.contains(target_suffix))
        .map(|asset| asset.browser_download_url.clone())
}

fn ensure_in_place_update_supported(
    kind: InstallSourceKind,
    executable: &std::path::Path,
) -> Result<()> {
    if matches!(
        kind,
        InstallSourceKind::CargoInstall | InstallSourceKind::Standalone
    ) {
        return Ok(());
    }

    let guidance = match kind {
        InstallSourceKind::CargoTarget => {
            "this looks like a development build under `target/`; rebuild or reinstall instead of self-updating"
        }
        InstallSourceKind::GitCheckout => {
            "this looks like a git checkout; update the repo or rebuild instead of self-updating"
        }
        InstallSourceKind::Unknown => {
            "the current executable origin could not be identified safely"
        }
        InstallSourceKind::CargoInstall | InstallSourceKind::Standalone => unreachable!(),
    };
    Err(anyhow!(
        "refusing to overwrite `{}` because {guidance}",
        executable.display()
    ))
}

/// Compare version strings. Returns true if `latest` > `current`.
fn is_newer_version(latest: &str, current: &str) -> bool {
    let parse_parts = |v: &str| -> Vec<u32> {
        v.split('.')
            .filter_map(|segment| segment.parse().ok())
            .collect()
    };

    let latest_parts = parse_parts(latest);
    let current_parts = parse_parts(current);

    for index in 0..latest_parts.len().max(current_parts.len()) {
        let latest_part = latest_parts.get(index).unwrap_or(&0);
        let current_part = current_parts.get(index).unwrap_or(&0);
        if latest_part > current_part {
            return true;
        }
        if latest_part < current_part {
            return false;
        }
    }
    false
}

/// Return the asset suffix for the current platform.
fn platform_asset_suffix() -> String {
    let os = if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        "unknown"
    };

    let arch = if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "unknown"
    };

    format!("{os}-{arch}")
}

#[cfg(test)]
mod tests {
    use super::{
        GitHubAsset, InstallSourceKind, ensure_in_place_update_supported, is_newer_version,
        latest_release_api_url, platform_asset_suffix, select_download_asset,
    };
    use std::path::Path;

    #[test]
    fn newer_version_detected() {
        assert!(is_newer_version("1.1.0", "1.0.0"));
        assert!(is_newer_version("2.0.0", "1.9.9"));
        assert!(is_newer_version("1.0.1", "1.0.0"));
    }

    #[test]
    fn same_version_not_newer() {
        assert!(!is_newer_version("1.0.0", "1.0.0"));
    }

    #[test]
    fn older_version_not_newer() {
        assert!(!is_newer_version("0.9.0", "1.0.0"));
        assert!(!is_newer_version("1.0.0", "1.0.1"));
    }

    #[test]
    fn platform_suffix_contains_os_and_arch() {
        let suffix = platform_asset_suffix();
        assert!(
            suffix.contains("windows") || suffix.contains("macos") || suffix.contains("linux"),
            "unexpected suffix: {suffix}"
        );
    }

    #[test]
    fn release_api_url_uses_workspace_repository() {
        assert_eq!(
            latest_release_api_url("yanzhi0922/remote-code-rust"),
            "https://api.github.com/repos/yanzhi0922/remote-code-rust/releases/latest"
        );
    }

    #[test]
    fn asset_selection_matches_platform_suffix_substring() {
        let assets = vec![
            GitHubAsset {
                name: "remote-code-windows-x86_64.zip".to_owned(),
                browser_download_url: "https://example.com/windows.zip".to_owned(),
                size: 1,
            },
            GitHubAsset {
                name: "remote-code-linux-x86_64.tar.gz".to_owned(),
                browser_download_url: "https://example.com/linux.tar.gz".to_owned(),
                size: 1,
            },
        ];
        assert_eq!(
            select_download_asset(&assets, "linux-x86_64").as_deref(),
            Some("https://example.com/linux.tar.gz")
        );
    }

    #[test]
    fn updater_refuses_dev_build_paths() {
        assert!(
            ensure_in_place_update_supported(
                InstallSourceKind::Standalone,
                Path::new("/tmp/remote-code")
            )
            .is_ok()
        );
        assert!(
            ensure_in_place_update_supported(
                InstallSourceKind::CargoTarget,
                Path::new("/repo/target/debug/remote-code")
            )
            .is_err()
        );
    }
}
