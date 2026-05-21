//! Enhanced plugin loader with validation integration.
//!
//! Provides [`load_plugin_from_directory`] for loading plugins with full
//! validation, [`load_plugin_with_runtime`] for runtime initialization,
//! and [`resolve_plugin_dependencies`] for dependency graph resolution.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::dependency::{DependencyLookupResult, ResolutionResult, resolve_dependency_closure};
use crate::{
    PluginBundle, PluginHostInfo, PluginRuntimeError, PluginRuntimeInspection,
    PluginValidationReport, discover_plugins, inspect_runtime, load_plugin_from_root,
    validate_plugin_bundle,
};

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

/// Errors during plugin loading.
#[derive(Debug, Error)]
pub enum LoaderError {
    /// Plugin not found at the specified path.
    #[error("plugin not found at `{path}`")]
    NotFound { path: PathBuf },
    /// Validation failed with errors.
    #[error("plugin validation failed for `{plugin_name}`: {errors:?}")]
    ValidationFailed {
        plugin_name: String,
        errors: Vec<String>,
    },
    /// Dependency resolution failed.
    #[error("dependency resolution failed: {reason}")]
    DependencyResolution { reason: String },
    /// I/O error.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

// ---------------------------------------------------------------------------
// Configuration types
// ---------------------------------------------------------------------------

/// Options for loading a plugin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginLoadOptions {
    /// Whether to run validation during loading.
    #[serde(default = "default_true")]
    pub validate: bool,
    /// Whether to fail on validation errors (vs. just warnings).
    #[serde(default)]
    pub strict: bool,
    /// Whether to resolve dependencies.
    #[serde(default)]
    pub resolve_deps: bool,
    /// Whether to initialize the runtime.
    #[serde(default)]
    pub init_runtime: bool,
    /// Optional host info for runtime initialization.
    #[serde(default)]
    pub host_info: Option<PluginHostInfo>,
}

impl Default for PluginLoadOptions {
    fn default() -> Self {
        Self {
            validate: true,
            strict: false,
            resolve_deps: false,
            init_runtime: false,
            host_info: None,
        }
    }
}

fn default_true() -> bool {
    true
}

/// Result of loading a plugin with full metadata.
#[derive(Debug, Clone)]
pub struct PluginLoadResult {
    /// The loaded plugin bundle.
    pub bundle: PluginBundle,
    /// Validation report (if validation was requested).
    pub validation: Option<PluginValidationReport>,
    /// Runtime inspection (if runtime was initialized).
    pub runtime_inspection: Option<PluginRuntimeInspection>,
    /// Dependency resolution result (if deps were resolved).
    pub dependency_resolution: Option<ResolutionResult>,
    /// Any warnings collected during loading.
    pub warnings: Vec<String>,
}

// ---------------------------------------------------------------------------
// Core functions
// ---------------------------------------------------------------------------

/// Load a plugin from a directory with optional validation.
///
/// This is the enhanced version of [`load_plugin_from_root`] that also
/// runs validation and collects warnings.
pub fn load_plugin_from_directory(
    directory: &Path,
    options: &PluginLoadOptions,
) -> Result<PluginLoadResult, LoaderError> {
    let bundle = load_plugin_from_root(directory).map_err(|_e| LoaderError::NotFound {
        path: directory.to_path_buf(),
    })?;

    let validation = if options.validate {
        let report = validate_plugin_bundle(&bundle);
        Some(report)
    } else {
        None
    };

    // Check for validation errors in strict mode
    if options.strict
        && let Some(ref report) = validation
        && !report.errors.is_empty()
    {
        return Err(LoaderError::ValidationFailed {
            plugin_name: bundle.manifest.name.clone(),
            errors: report.errors.clone(),
        });
    }

    let mut warnings = Vec::new();
    if let Some(ref report) = validation {
        warnings.extend(report.warnings.clone());
    }

    Ok(PluginLoadResult {
        bundle,
        validation,
        runtime_inspection: None,
        dependency_resolution: None,
        warnings,
    })
}

/// Load a plugin and initialize its runtime.
///
/// Combines loading, validation, and runtime initialization into one call.
pub async fn load_plugin_with_runtime(
    directory: &Path,
    host_info: &PluginHostInfo,
) -> Result<PluginLoadResult, LoaderError> {
    let options = PluginLoadOptions {
        validate: true,
        strict: false,
        resolve_deps: false,
        init_runtime: true,
        host_info: Some(host_info.clone()),
    };

    let mut result = load_plugin_from_directory(directory, &options)?;

    if result.bundle.runtime_config().is_some() {
        match inspect_runtime(&result.bundle, host_info).await {
            Ok(inspection) => {
                result.runtime_inspection = Some(inspection);
            }
            Err(PluginRuntimeError::MissingRuntimeConfig { .. }) => {
                // No runtime configured, that's fine
            }
            Err(e) => {
                result
                    .warnings
                    .push(format!("runtime initialization failed: {e}"));
            }
        }
    }

    Ok(result)
}

/// Resolve plugin dependencies for a loaded plugin.
///
/// Uses the dependency resolver to compute the full transitive closure.
pub fn resolve_plugin_dependencies(
    plugin: &PluginBundle,
    marketplace_name: Option<&str>,
    lookup: &dyn Fn(&str) -> Option<DependencyLookupResult>,
    already_enabled: &std::collections::HashSet<String>,
) -> Result<ResolutionResult, LoaderError> {
    let root_id = match marketplace_name {
        Some(mkt) => format!("{}@{}", plugin.manifest.name, mkt),
        None => plugin.manifest.name.clone(),
    };

    Ok(resolve_dependency_closure(
        &root_id,
        lookup,
        already_enabled,
        &std::collections::HashSet::new(),
    ))
}

/// Load all plugins from a root directory with validation.
///
/// Returns a list of load results, one per discovered plugin.
pub fn load_all_plugins(
    root: &Path,
    options: &PluginLoadOptions,
) -> Result<Vec<PluginLoadResult>, LoaderError> {
    let bundles = discover_plugins(root)
        .map_err(|e| LoaderError::Io(std::io::Error::other(e.to_string())))?;

    let mut results = Vec::new();
    for bundle in bundles {
        let validation = if options.validate {
            Some(validate_plugin_bundle(&bundle))
        } else {
            None
        };

        let mut warnings = Vec::new();
        if let Some(ref report) = validation {
            warnings.extend(report.warnings.clone());
        }

        results.push(PluginLoadResult {
            bundle,
            validation,
            runtime_inspection: None,
            dependency_resolution: None,
            warnings,
        });
    }

    Ok(results)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn ok<T, E: std::fmt::Display>(result: Result<T, E>) -> T {
        match result {
            Ok(value) => value,
            Err(error) => panic!("unexpected error: {error}"),
        }
    }

    fn create_test_plugin(root: &Path, name: &str) {
        let manifest_dir = root.join(crate::PLUGIN_MANIFEST_DIR);
        fs::create_dir_all(&manifest_dir).expect("create dir");
        fs::write(
            manifest_dir.join(crate::PLUGIN_MANIFEST_FILE),
            format!(r#"{{"name":"{name}","version":"0.1.0"}}"#),
        )
        .expect("write manifest");
    }

    #[test]
    fn load_plugin_from_directory_basic() {
        let temp = ok(tempdir());
        create_test_plugin(temp.path(), "test-plugin");

        let options = PluginLoadOptions::default();
        let result = ok(load_plugin_from_directory(temp.path(), &options));

        assert_eq!(result.bundle.manifest.name, "test-plugin");
        assert!(result.validation.is_some());
        assert!(
            result.warnings.is_empty()
                || result
                    .validation
                    .as_ref()
                    .is_some_and(|v| v.warnings == result.warnings)
        );
    }

    #[test]
    fn load_plugin_from_directory_no_validation() {
        let temp = ok(tempdir());
        create_test_plugin(temp.path(), "test-plugin");

        let options = PluginLoadOptions {
            validate: false,
            ..PluginLoadOptions::default()
        };
        let result = ok(load_plugin_from_directory(temp.path(), &options));

        assert!(result.validation.is_none());
    }

    #[test]
    fn load_plugin_from_directory_strict_fails_on_errors() {
        let temp = ok(tempdir());
        let root = temp.path();
        let manifest_dir = root.join(crate::PLUGIN_MANIFEST_DIR);
        fs::create_dir_all(&manifest_dir).expect("create dir");
        // Empty name should cause a validation error
        fs::write(
            manifest_dir.join(crate::PLUGIN_MANIFEST_FILE),
            r#"{"name":"","version":"0.1.0","skills":"./nonexistent"}"#,
        )
        .expect("write manifest");

        let options = PluginLoadOptions {
            strict: true,
            ..PluginLoadOptions::default()
        };
        let result = load_plugin_from_directory(root, &options);
        assert!(
            result.is_err(),
            "strict mode should fail on validation errors"
        );
    }

    #[test]
    fn load_plugin_from_directory_not_found() {
        let temp = ok(tempdir());
        let options = PluginLoadOptions::default();
        let result = load_plugin_from_directory(temp.path(), &options);
        assert!(result.is_err());
    }

    #[test]
    fn load_all_plugins_discovers_multiple() {
        let temp = ok(tempdir());
        let alpha = temp.path().join("alpha");
        let beta = temp.path().join("beta");
        create_test_plugin(&alpha, "alpha");
        create_test_plugin(&beta, "beta");

        let options = PluginLoadOptions::default();
        let results = ok(load_all_plugins(temp.path(), &options));

        assert_eq!(results.len(), 2);
        let names: Vec<&str> = results
            .iter()
            .map(|r| r.bundle.manifest.name.as_str())
            .collect();
        assert!(names.contains(&"alpha"));
        assert!(names.contains(&"beta"));
    }

    #[test]
    fn plugin_load_options_default() {
        let opts = PluginLoadOptions::default();
        assert!(opts.validate);
        assert!(!opts.strict);
        assert!(!opts.resolve_deps);
        assert!(!opts.init_runtime);
        assert!(opts.host_info.is_none());
    }

    #[test]
    fn resolve_plugin_dependencies_basic() {
        let temp = ok(tempdir());
        create_test_plugin(temp.path(), "root-plugin");

        let result = ok(load_plugin_from_directory(
            temp.path(),
            &PluginLoadOptions::default(),
        ));

        // The lookup must return the root plugin itself so the resolver
        // can walk it.  Plugins with no dependencies resolve immediately.
        let lookup = |id: &str| -> Option<DependencyLookupResult> {
            if id == "root-plugin@mkt" || id == "root-plugin" {
                Some(DependencyLookupResult {
                    dependencies: Vec::new(),
                })
            } else {
                None
            }
        };
        let resolution = ok(resolve_plugin_dependencies(
            &result.bundle,
            Some("mkt"),
            &lookup,
            &std::collections::HashSet::new(),
        ));

        match resolution {
            ResolutionResult::Ok { closure } => {
                assert!(closure.contains(&"root-plugin@mkt".to_owned()));
            }
            _ => panic!("expected Ok, got {resolution:?}"),
        }
    }
}
