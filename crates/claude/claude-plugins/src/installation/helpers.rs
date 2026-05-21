//! Plugin installation helpers.
//!
//! Provides utilities for downloading, extracting, and verifying plugins,
//! as well as computing installation paths.

use std::fs::{self, File};
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use flate2::read::GzDecoder;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use tar::Archive;
use zip::ZipArchive;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Result of a download operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownloadResult {
    /// Path to the downloaded file.
    pub path: PathBuf,
    /// Size of the downloaded file in bytes.
    pub size_bytes: u64,
    /// Whether the download succeeded.
    pub success: bool,
    /// Error message if download failed.
    pub error: Option<String>,
}

/// Result of an extraction operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtractResult {
    /// Path to the extracted directory.
    pub path: PathBuf,
    /// Number of files extracted.
    pub file_count: usize,
    /// Whether the extraction succeeded.
    pub success: bool,
    /// Error message if extraction failed.
    pub error: Option<String>,
}

/// Result of a verification operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifyResult {
    /// Whether the verification succeeded.
    pub valid: bool,
    /// Issues found during verification.
    pub issues: Vec<String>,
}

// ---------------------------------------------------------------------------
// Core functions
// ---------------------------------------------------------------------------

/// Download a plugin from a source URL.
///
/// Supports HTTP(S), `file://` URLs, and direct local file paths.
pub fn download_plugin(source_url: &str, target_dir: &Path) -> DownloadResult {
    if source_url.is_empty() {
        return DownloadResult {
            path: PathBuf::new(),
            size_bytes: 0,
            success: false,
            error: Some("empty source URL".to_owned()),
        };
    }

    if !target_dir.exists()
        && let Err(e) = std::fs::create_dir_all(target_dir)
    {
        return DownloadResult {
            path: PathBuf::new(),
            size_bytes: 0,
            success: false,
            error: Some(format!("failed to create target dir: {e}")),
        };
    }

    let filename = extract_filename(source_url);
    let target_path = target_dir.join(filename);

    match download_to_path(source_url, &target_path) {
        Ok(size_bytes) => DownloadResult {
            path: target_path,
            size_bytes,
            success: true,
            error: None,
        },
        Err(error) => DownloadResult {
            path: PathBuf::new(),
            size_bytes: 0,
            success: false,
            error: Some(error),
        },
    }
}

/// Extract a plugin archive to a target directory.
///
/// Supports `.zip`, `.tar`, `.tar.gz`, and `.tgz` archives.
pub fn extract_plugin(archive_path: &Path, target_dir: &Path) -> ExtractResult {
    if !archive_path.exists() {
        return ExtractResult {
            path: PathBuf::new(),
            file_count: 0,
            success: false,
            error: Some(format!("archive not found: {}", archive_path.display())),
        };
    }

    if let Err(e) = std::fs::create_dir_all(target_dir) {
        return ExtractResult {
            path: PathBuf::new(),
            file_count: 0,
            success: false,
            error: Some(format!("failed to create target dir: {e}")),
        };
    }

    match extract_archive(archive_path, target_dir) {
        Ok(file_count) => ExtractResult {
            path: target_dir.to_path_buf(),
            file_count,
            success: true,
            error: None,
        },
        Err(error) => ExtractResult {
            path: PathBuf::new(),
            file_count: 0,
            success: false,
            error: Some(error),
        },
    }
}

/// Verify a plugin's integrity.
///
/// Checks that the plugin directory contains a valid manifest.
pub fn verify_plugin(plugin_dir: &Path) -> VerifyResult {
    let mut issues = Vec::new();

    if !plugin_dir.exists() {
        issues.push(format!(
            "plugin directory {} does not exist",
            plugin_dir.display()
        ));
        return VerifyResult {
            valid: false,
            issues,
        };
    }

    let manifest_path = plugin_dir
        .join(crate::PLUGIN_MANIFEST_DIR)
        .join(crate::PLUGIN_MANIFEST_FILE);

    if !manifest_path.exists() {
        issues.push(format!("manifest not found at {}", manifest_path.display()));
    } else if let Ok(content) = std::fs::read_to_string(&manifest_path) {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&content) {
            if value.get("name").and_then(|v| v.as_str()).is_none() {
                issues.push("manifest missing 'name' field".to_owned());
            }
            if value.get("version").and_then(|v| v.as_str()).is_none() {
                issues.push("manifest missing 'version' field".to_owned());
            }
        } else {
            issues.push("manifest is not valid JSON".to_owned());
        }
    }

    VerifyResult {
        valid: issues.is_empty(),
        issues,
    }
}

/// Compute the installation path for a plugin.
///
/// Returns a path like `<base>/<marketplace>/<plugin-name>/<version>`.
pub fn compute_install_path(
    base: &Path,
    marketplace: &str,
    plugin_name: &str,
    version: &str,
) -> PathBuf {
    base.join(marketplace).join(plugin_name).join(version)
}

/// Extract a filename from a URL.
fn extract_filename(url: &str) -> String {
    let trimmed = url.split('?').next().unwrap_or(url).trim_end_matches('/');
    let candidate = trimmed.rsplit('/').next().unwrap_or_default();
    if candidate.is_empty() {
        "plugin.zip".to_owned()
    } else {
        candidate.to_owned()
    }
}

/// Validate that a resolved path stays within a base directory.
///
/// Prevents path traversal attacks.
pub fn validate_path_within_base(base: &Path, relative: &str) -> Option<PathBuf> {
    let base = absolute_base_path(base)?;
    let mut resolved = base.clone();

    for component in Path::new(relative).components() {
        match component {
            Component::Prefix(_) | Component::RootDir => return None,
            Component::CurDir => {}
            Component::ParentDir => {
                if resolved == base {
                    return None;
                }
                resolved.pop();
            }
            Component::Normal(segment) => resolved.push(segment),
        }
    }

    Some(resolved)
}

fn absolute_base_path(base: &Path) -> Option<PathBuf> {
    if base.is_absolute() {
        Some(base.to_path_buf())
    } else {
        std::env::current_dir().ok().map(|cwd| cwd.join(base))
    }
}

fn download_to_path(source_url: &str, target_path: &Path) -> Result<u64, String> {
    if let Some(local_path) = source_url.strip_prefix("file://") {
        return copy_local_file(Path::new(local_path), target_path);
    }
    if Path::new(source_url).exists() {
        return copy_local_file(Path::new(source_url), target_path);
    }
    if source_url.starts_with("http://") || source_url.starts_with("https://") {
        return download_http_file(source_url, target_path);
    }

    Err(format!("unsupported plugin source: {source_url}"))
}

fn copy_local_file(source_path: &Path, target_path: &Path) -> Result<u64, String> {
    if !source_path.exists() {
        return Err(format!("local source not found: {}", source_path.display()));
    }

    if source_path == target_path {
        return fs::metadata(source_path)
            .map(|metadata| metadata.len())
            .map_err(|error| format!("failed to stat {}: {error}", source_path.display()));
    }

    if let Some(parent) = target_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create target directory: {error}"))?;
    }

    fs::copy(source_path, target_path).map_err(|error| {
        format!(
            "failed to copy {} to {}: {error}",
            source_path.display(),
            target_path.display()
        )
    })
}

fn download_http_file(source_url: &str, target_path: &Path) -> Result<u64, String> {
    let client = Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|error| format!("failed to build HTTP client: {error}"))?;

    let mut response = client
        .get(source_url)
        .send()
        .map_err(|error| format!("failed to download plugin from {source_url}: {error}"))?;

    if !response.status().is_success() {
        return Err(format!(
            "plugin download failed with HTTP {} from {}",
            response.status(),
            source_url
        ));
    }

    if let Some(parent) = target_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create target directory: {error}"))?;
    }

    let mut file = File::create(target_path).map_err(|error| {
        format!(
            "failed to create download target {}: {error}",
            target_path.display()
        )
    })?;

    io::copy(&mut response, &mut file).map_err(|error| {
        format!(
            "failed to write plugin archive to {}: {error}",
            target_path.display()
        )
    })
}

fn extract_archive(archive_path: &Path, target_dir: &Path) -> Result<usize, String> {
    let file_name = archive_path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();

    if file_name.ends_with(".tar.gz") || file_name.ends_with(".tgz") {
        let archive = File::open(archive_path).map_err(|error| {
            format!("failed to open archive {}: {error}", archive_path.display())
        })?;
        let decoder = GzDecoder::new(archive);
        let mut archive = Archive::new(decoder);
        return extract_tar_entries(&mut archive, target_dir);
    }

    match archive_path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase())
        .as_deref()
    {
        Some("zip") => extract_zip_archive(archive_path, target_dir),
        Some("tar") => {
            let archive = File::open(archive_path).map_err(|error| {
                format!("failed to open archive {}: {error}", archive_path.display())
            })?;
            let mut archive = Archive::new(archive);
            extract_tar_entries(&mut archive, target_dir)
        }
        _ => Err(format!(
            "unsupported plugin archive format: {}",
            archive_path.display()
        )),
    }
}

fn extract_zip_archive(archive_path: &Path, target_dir: &Path) -> Result<usize, String> {
    let file = File::open(archive_path)
        .map_err(|error| format!("failed to open archive {}: {error}", archive_path.display()))?;
    let mut archive =
        ZipArchive::new(file).map_err(|error| format!("failed to read ZIP archive: {error}"))?;
    let mut file_count = 0usize;

    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| format!("failed to read ZIP entry #{index}: {error}"))?;
        let enclosed = entry.enclosed_name().ok_or_else(|| {
            format!(
                "ZIP entry '{}' would escape the target directory",
                entry.name()
            )
        })?;
        let output_path = target_dir.join(enclosed);

        if entry.is_dir() {
            fs::create_dir_all(&output_path).map_err(|error| {
                format!(
                    "failed to create directory {}: {error}",
                    output_path.display()
                )
            })?;
            continue;
        }

        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                format!("failed to create directory {}: {error}", parent.display())
            })?;
        }

        let mut output = File::create(&output_path)
            .map_err(|error| format!("failed to create {}: {error}", output_path.display()))?;
        io::copy(&mut entry, &mut output)
            .map_err(|error| format!("failed to extract {}: {error}", output_path.display()))?;
        file_count += 1;
    }

    Ok(file_count)
}

fn extract_tar_entries<R: io::Read>(
    archive: &mut Archive<R>,
    target_dir: &Path,
) -> Result<usize, String> {
    let mut file_count = 0usize;
    let entries = archive
        .entries()
        .map_err(|error| format!("failed to read TAR archive entries: {error}"))?;

    for entry in entries {
        let mut entry = entry.map_err(|error| format!("failed to inspect TAR entry: {error}"))?;
        if !entry
            .unpack_in(target_dir)
            .map_err(|error| format!("failed to extract TAR entry: {error}"))?
        {
            return Err("TAR entry would escape the target directory".to_owned());
        }
        if entry.header().entry_type().is_file() {
            file_count += 1;
        }
    }

    Ok(file_count)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;
    use tempfile::tempdir;
    use zip::write::SimpleFileOptions;

    fn ok<T, E: std::fmt::Display>(result: Result<T, E>) -> T {
        match result {
            Ok(value) => value,
            Err(error) => panic!("unexpected error: {error}"),
        }
    }

    #[test]
    fn download_plugin_basic() {
        let temp = ok(tempdir());
        let source = temp.path().join("plugin.zip");
        fs::write(&source, b"archive-bytes").expect("write source");

        let result = download_plugin(&source.to_string_lossy(), temp.path());
        assert!(result.success);
        assert_eq!(result.path, temp.path().join("plugin.zip"));
        assert_eq!(result.size_bytes, 13);
    }

    #[test]
    fn download_plugin_empty_url() {
        let temp = ok(tempdir());
        let result = download_plugin("", temp.path());
        assert!(!result.success);
    }

    #[test]
    fn extract_plugin_basic() {
        let temp = ok(tempdir());
        let archive = temp.path().join("plugin.zip");
        let file = fs::File::create(&archive).expect("create archive");
        let mut zip = zip::ZipWriter::new(file);
        zip.start_file("README.md", SimpleFileOptions::default())
            .expect("start file");
        zip.write_all(b"# demo\n").expect("write entry");
        zip.finish().expect("finish archive");
        let target = temp.path().join("extracted");

        let result = extract_plugin(&archive, &target);
        assert!(result.success);
        assert_eq!(result.file_count, 1);
        assert_eq!(
            fs::read_to_string(target.join("README.md")).expect("read extracted"),
            "# demo\n"
        );
    }

    #[test]
    fn extract_plugin_nonexistent() {
        let result = extract_plugin(Path::new("/nonexistent.zip"), Path::new("/tmp/out"));
        assert!(!result.success);
    }

    #[test]
    fn verify_plugin_valid() {
        let temp = ok(tempdir());
        let manifest_dir = temp.path().join(crate::PLUGIN_MANIFEST_DIR);
        fs::create_dir_all(&manifest_dir).expect("create dir");
        fs::write(
            manifest_dir.join(crate::PLUGIN_MANIFEST_FILE),
            r#"{"name":"test","version":"1.0.0"}"#,
        )
        .expect("write");

        let result = verify_plugin(temp.path());
        assert!(result.valid);
        assert!(result.issues.is_empty());
    }

    #[test]
    fn verify_plugin_missing_manifest() {
        let temp = ok(tempdir());
        let result = verify_plugin(temp.path());
        assert!(!result.valid);
        assert!(!result.issues.is_empty());
    }

    #[test]
    fn verify_plugin_nonexistent() {
        let result = verify_plugin(Path::new("/nonexistent"));
        assert!(!result.valid);
    }

    #[test]
    fn verify_plugin_invalid_manifest() {
        let temp = ok(tempdir());
        let manifest_dir = temp.path().join(crate::PLUGIN_MANIFEST_DIR);
        fs::create_dir_all(&manifest_dir).expect("create dir");
        fs::write(
            manifest_dir.join(crate::PLUGIN_MANIFEST_FILE),
            r#"{"no-name": true}"#,
        )
        .expect("write");

        let result = verify_plugin(temp.path());
        assert!(!result.valid);
    }

    #[test]
    fn compute_install_path_works() {
        let path = compute_install_path(Path::new("/plugins"), "mkt", "my-plugin", "1.0.0");
        assert_eq!(path, PathBuf::from("/plugins/mkt/my-plugin/1.0.0"));
    }

    #[test]
    fn extract_filename_works() {
        assert_eq!(
            extract_filename("https://example.com/plugin.zip"),
            "plugin.zip"
        );
        assert_eq!(extract_filename("no-slash"), "no-slash");
    }

    #[test]
    fn validate_path_within_base_rejects_traversal() {
        let temp = ok(tempdir());
        assert!(validate_path_within_base(temp.path(), "../escape.txt").is_none());
        assert!(validate_path_within_base(temp.path(), "safe/file.txt").is_some());
    }
}
