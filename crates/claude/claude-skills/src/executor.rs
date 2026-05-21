//! Skill executor — loads, validates, and executes skills.
//!
//! The executor takes a [`SkillDocument`] and an [`SkillExecutionContext`],
//! builds a prompt from the skill's instructions and references, and returns
//! a [`SkillExecutionResult`] containing the constructed prompt and metadata.

use std::{collections::HashMap, fs, path::PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{SkillDocument, SkillMetadata};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors that can occur during skill execution.
#[derive(Debug, Error)]
pub enum ExecutorError {
    /// The skill document failed validation.
    #[error("skill validation failed: {reason}")]
    Validation { reason: String },
    /// A referenced file could not be read.
    #[error("failed to read reference file `{path}`")]
    ReadReference {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// The skill instructions are empty.
    #[error("skill `{slug}` has no instructions")]
    EmptyInstructions { slug: String },
    /// The skill slug is empty.
    #[error("skill slug is empty")]
    EmptySlug,
    /// The skill root directory does not exist.
    #[error("skill root directory does not exist: `{path}`")]
    MissingRoot { path: PathBuf },
    /// Environment variable interpolation failed.
    #[error("environment variable `{name}` not found")]
    EnvVarMissing { name: String },
}

// ---------------------------------------------------------------------------
// Execution Context
// ---------------------------------------------------------------------------

/// Context provided when executing a skill.
#[derive(Debug, Clone)]
pub struct SkillExecutionContext {
    /// The working directory for execution.
    pub working_dir: PathBuf,
    /// Environment variables available for interpolation.
    pub env_vars: HashMap<String, String>,
    /// Additional user-provided arguments.
    pub args: Vec<String>,
    /// Maximum prompt length (0 = unlimited).
    pub max_prompt_length: usize,
}

impl Default for SkillExecutionContext {
    fn default() -> Self {
        Self {
            working_dir: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            env_vars: HashMap::new(),
            args: Vec::new(),
            max_prompt_length: 0,
        }
    }
}

impl SkillExecutionContext {
    /// Create a new execution context with the given working directory.
    #[must_use]
    pub fn new(working_dir: impl Into<PathBuf>) -> Self {
        Self {
            working_dir: working_dir.into(),
            ..Self::default()
        }
    }

    /// Add an environment variable.
    #[must_use]
    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env_vars.insert(key.into(), value.into());
        self
    }

    /// Add arguments.
    #[must_use]
    pub fn with_args(mut self, args: Vec<String>) -> Self {
        self.args = args;
        self
    }

    /// Set the maximum prompt length.
    #[must_use]
    pub fn with_max_prompt_length(mut self, len: usize) -> Self {
        self.max_prompt_length = len;
        self
    }
}

// ---------------------------------------------------------------------------
// Execution Result
// ---------------------------------------------------------------------------

/// Result of executing a skill.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillExecutionResult {
    /// The slug of the executed skill.
    pub slug: String,
    /// The constructed prompt text.
    pub prompt: String,
    /// Number of reference files loaded.
    pub references_loaded: usize,
    /// Number of environment variables interpolated.
    pub env_vars_interpolated: usize,
    /// Whether validation passed.
    pub valid: bool,
    /// Any warnings generated during execution.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

// ---------------------------------------------------------------------------
// Skill Executor
// ---------------------------------------------------------------------------

/// Executes skills by loading instructions, resolving references,
/// interpolating environment variables, and constructing prompts.
#[derive(Debug, Clone, Default)]
pub struct SkillExecutor {
    /// Maximum prompt length (0 = unlimited).
    pub max_prompt_length: usize,
}

impl SkillExecutor {
    /// Create a new executor.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Create an executor with a maximum prompt length.
    #[must_use]
    pub fn with_max_prompt_length(max_prompt_length: usize) -> Self {
        Self { max_prompt_length }
    }

    /// Validate a skill document before execution.
    ///
    /// Checks that the slug is non-empty, instructions exist, and the root
    /// directory is present.
    pub fn validate_skill(&self, skill: &SkillDocument) -> Result<(), ExecutorError> {
        if skill.metadata.slug.is_empty() {
            return Err(ExecutorError::EmptySlug);
        }

        if skill.instructions.trim().is_empty() {
            return Err(ExecutorError::EmptyInstructions {
                slug: skill.metadata.slug.clone(),
            });
        }

        if !skill.metadata.root.exists() {
            return Err(ExecutorError::MissingRoot {
                path: skill.metadata.root.clone(),
            });
        }

        // Check that all referenced files exist (warning only, not error)
        Ok(())
    }

    /// Validate skill metadata only (lighter check).
    pub fn validate_metadata(&self, meta: &SkillMetadata) -> Result<(), ExecutorError> {
        if meta.slug.is_empty() {
            return Err(ExecutorError::EmptySlug);
        }
        if !meta.root.exists() {
            return Err(ExecutorError::MissingRoot {
                path: meta.root.clone(),
            });
        }
        Ok(())
    }

    /// Execute a skill, producing a constructed prompt.
    pub fn execute_skill(
        &self,
        skill: &SkillDocument,
        context: &SkillExecutionContext,
    ) -> Result<SkillExecutionResult, ExecutorError> {
        self.validate_skill(skill)?;

        let mut warnings = Vec::new();

        // Load reference files
        let (ref_contents, refs_loaded) = self.load_references(&skill.metadata)?;

        // Interpolate environment variables
        let (prompt_text, env_count) =
            self.interpolate_env_vars(&skill.instructions, &context.env_vars);

        // Build the full prompt
        let mut prompt = format!(
            "# Skill: {}\n\n## Instructions\n\n{}",
            skill.metadata.title, prompt_text
        );

        if !ref_contents.is_empty() {
            prompt.push_str("\n\n## References\n\n");
            for (name, content) in &ref_contents {
                prompt.push_str(&format!("### {name}\n\n{content}\n\n"));
            }
        }

        if !context.args.is_empty() {
            prompt.push_str(&format!("\n\n## Arguments\n\n{}", context.args.join(" ")));
        }

        // Truncate if needed
        let max_len = if self.max_prompt_length > 0 {
            self.max_prompt_length
        } else if context.max_prompt_length > 0 {
            context.max_prompt_length
        } else {
            0
        };

        if max_len > 0 && prompt.len() > max_len {
            prompt.truncate(max_len);
            warnings.push(format!(
                "Prompt truncated to {max_len} characters (was {})",
                prompt.len()
            ));
        }

        // Check for missing references
        for ref_path in &skill.metadata.references {
            if !ref_path.exists() {
                warnings.push(format!("Reference file not found: {}", ref_path.display()));
            }
        }

        Ok(SkillExecutionResult {
            slug: skill.metadata.slug.clone(),
            prompt,
            references_loaded: refs_loaded,
            env_vars_interpolated: env_count,
            valid: true,
            warnings,
        })
    }

    /// Load all reference file contents.
    fn load_references(
        &self,
        meta: &SkillMetadata,
    ) -> Result<(Vec<(String, String)>, usize), ExecutorError> {
        let mut contents = Vec::new();
        let mut loaded = 0;

        for ref_path in &meta.references {
            if !ref_path.exists() {
                continue;
            }
            let content =
                fs::read_to_string(ref_path).map_err(|source| ExecutorError::ReadReference {
                    path: ref_path.clone(),
                    source,
                })?;
            let name = ref_path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "unknown".to_owned());
            contents.push((name, content));
            loaded += 1;
        }

        Ok((contents, loaded))
    }

    /// Interpolate `{{ENV_VAR}}` patterns in text.
    fn interpolate_env_vars(
        &self,
        text: &str,
        env_vars: &HashMap<String, String>,
    ) -> (String, usize) {
        let mut result = text.to_owned();
        let mut count = 0;

        for (key, value) in env_vars {
            let pattern = format!("{{{{{key}}}}}");
            if result.contains(&pattern) {
                result = result.replace(&pattern, value);
                count += 1;
            }
        }

        (result, count)
    }

    /// Build a prompt for a skill without full execution.
    ///
    /// Useful for previewing what the prompt would look like.
    pub fn build_preview(&self, skill: &SkillDocument) -> String {
        format!(
            "# Skill: {}\n> {}\n\n{}",
            skill.metadata.title,
            skill.metadata.summary.as_deref().unwrap_or("(no summary)"),
            skill.instructions
        )
    }

    /// Check if a skill matches a query string.
    #[must_use]
    pub fn matches_query(skill: &SkillDocument, query: &str) -> bool {
        let lower = query.to_ascii_lowercase();
        skill.metadata.slug.to_ascii_lowercase().contains(&lower)
            || skill.metadata.title.to_ascii_lowercase().contains(&lower)
            || skill
                .metadata
                .summary
                .as_ref()
                .is_some_and(|s| s.to_ascii_lowercase().contains(&lower))
            || skill
                .metadata
                .triggers
                .iter()
                .any(|t| t.to_ascii_lowercase().contains(&lower))
    }
}

// ---------------------------------------------------------------------------
// Batch validation
// ---------------------------------------------------------------------------

/// Validate multiple skills and return a list of results.
pub fn validate_skills(
    skills: &[SkillDocument],
) -> Vec<(&SkillDocument, Result<(), ExecutorError>)> {
    let executor = SkillExecutor::new();
    skills
        .iter()
        .map(|skill| (skill, executor.validate_skill(skill)))
        .collect()
}

/// Filter skills by a query string.
pub fn filter_skills_by_query<'a>(
    skills: &'a [SkillDocument],
    query: &str,
) -> Vec<&'a SkillDocument> {
    skills
        .iter()
        .filter(|skill| SkillExecutor::matches_query(skill, query))
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SkillDocument, SkillMetadata};
    use std::fs;
    use tempfile::tempdir;

    fn ok<T, E: std::fmt::Display>(result: Result<T, E>) -> T {
        match result {
            Ok(value) => value,
            Err(error) => panic!("unexpected error: {error}"),
        }
    }

    fn make_skill(temp: &std::path::Path, slug: &str, instructions: &str) -> SkillDocument {
        let root = temp.join(slug);
        fs::create_dir_all(&root).expect("create dir");
        let skill_path = root.join("SKILL.md");
        fs::write(&skill_path, instructions).expect("write skill");
        SkillDocument {
            metadata: SkillMetadata {
                slug: slug.to_owned(),
                title: slug.to_owned(),
                summary: Some(format!("Summary for {slug}")),
                path: skill_path,
                root: root.clone(),
                tags: vec![],
                tools: vec![],
                triggers: vec![],
                references: vec![],
                scripts: vec![],
                assets: vec![],
            },
            instructions: instructions.to_owned(),
        }
    }

    fn make_skill_with_refs(
        temp: &std::path::Path,
        slug: &str,
        instructions: &str,
        refs: &[(&str, &str)],
    ) -> SkillDocument {
        let root = temp.join(slug);
        fs::create_dir_all(root.join("references")).expect("create dir");
        let skill_path = root.join("SKILL.md");
        fs::write(&skill_path, instructions).expect("write skill");

        let mut ref_paths = Vec::new();
        for (name, content) in refs {
            let ref_path = root.join("references").join(name);
            fs::write(&ref_path, content).expect("write ref");
            ref_paths.push(ref_path);
        }

        SkillDocument {
            metadata: SkillMetadata {
                slug: slug.to_owned(),
                title: slug.to_owned(),
                summary: Some(format!("Summary for {slug}")),
                path: skill_path,
                root,
                tags: vec![],
                tools: vec![],
                triggers: vec![],
                references: ref_paths,
                scripts: vec![],
                assets: vec![],
            },
            instructions: instructions.to_owned(),
        }
    }

    // --- validate_skill ---

    #[test]
    fn validate_skill_ok() {
        let temp = ok(tempdir());
        let skill = make_skill(temp.path(), "demo", "# Demo\n\nHello");
        let executor = SkillExecutor::new();
        ok(executor.validate_skill(&skill));
    }

    #[test]
    fn validate_skill_empty_slug_fails() {
        let temp = ok(tempdir());
        let mut skill = make_skill(temp.path(), "x", "# X\n\nHello");
        skill.metadata.slug = String::new();
        let executor = SkillExecutor::new();
        let err = executor
            .validate_skill(&skill)
            .expect_err("empty slug should fail validation");
        assert!(matches!(err, ExecutorError::EmptySlug));
    }

    #[test]
    fn validate_skill_empty_instructions_fails() {
        let temp = ok(tempdir());
        let mut skill = make_skill(temp.path(), "empty", "");
        skill.metadata.slug = "empty".to_owned();
        let executor = SkillExecutor::new();
        let err = executor
            .validate_skill(&skill)
            .expect_err("empty instructions should fail validation");
        assert!(matches!(err, ExecutorError::EmptyInstructions { .. }));
    }

    #[test]
    fn validate_skill_missing_root_fails() {
        let mut skill = make_skill_with_refs(
            std::env::temp_dir().as_path(),
            "ghost",
            "# Ghost\n\nBoo",
            &[],
        );
        skill.metadata.root = PathBuf::from("/nonexistent/path/ghost");
        let executor = SkillExecutor::new();
        let err = executor
            .validate_skill(&skill)
            .expect_err("missing root should fail validation");
        assert!(matches!(err, ExecutorError::MissingRoot { .. }));
    }

    // --- validate_metadata ---

    #[test]
    fn validate_metadata_ok() {
        let temp = ok(tempdir());
        let skill = make_skill(temp.path(), "meta", "# Meta\n\nContent");
        let executor = SkillExecutor::new();
        ok(executor.validate_metadata(&skill.metadata));
    }

    #[test]
    fn validate_metadata_empty_slug_fails() {
        let temp = ok(tempdir());
        let mut skill = make_skill(temp.path(), "meta", "# Meta\n\nContent");
        skill.metadata.slug = String::new();
        let executor = SkillExecutor::new();
        let err = executor
            .validate_metadata(&skill.metadata)
            .expect_err("empty slug metadata should fail validation");
        assert!(matches!(err, ExecutorError::EmptySlug));
    }

    // --- execute_skill ---

    #[test]
    fn execute_skill_basic() {
        let temp = ok(tempdir());
        let skill = make_skill(temp.path(), "basic", "# Basic\n\nDo something.");
        let executor = SkillExecutor::new();
        let ctx = SkillExecutionContext::new(temp.path());
        let result = ok(executor.execute_skill(&skill, &ctx));
        assert_eq!(result.slug, "basic");
        assert!(result.prompt.contains("Do something."));
        assert!(result.valid);
        assert_eq!(result.references_loaded, 0);
    }

    #[test]
    fn execute_skill_with_references() {
        let temp = ok(tempdir());
        let skill = make_skill_with_refs(
            temp.path(),
            "ref-skill",
            "# Ref Skill\n\nSee references.",
            &[
                ("guide.md", "This is the guide."),
                ("tips.md", "Helpful tips."),
            ],
        );
        let executor = SkillExecutor::new();
        let ctx = SkillExecutionContext::new(temp.path());
        let result = ok(executor.execute_skill(&skill, &ctx));
        assert_eq!(result.references_loaded, 2);
        assert!(result.prompt.contains("This is the guide."));
        assert!(result.prompt.contains("Helpful tips."));
    }

    #[test]
    fn execute_skill_with_env_interpolation() {
        let temp = ok(tempdir());
        let skill = make_skill(
            temp.path(),
            "env-skill",
            "Hello {{NAME}}, welcome to {{PROJECT}}!",
        );
        let executor = SkillExecutor::new();
        let ctx = SkillExecutionContext::new(temp.path())
            .with_env("NAME", "Alice")
            .with_env("PROJECT", "RemoteCode");
        let result = ok(executor.execute_skill(&skill, &ctx));
        assert_eq!(result.env_vars_interpolated, 2);
        assert!(
            result
                .prompt
                .contains("Hello Alice, welcome to RemoteCode!")
        );
    }

    #[test]
    fn execute_skill_with_args() {
        let temp = ok(tempdir());
        let skill = make_skill(temp.path(), "args-skill", "# Args\n\nRun with args.");
        let executor = SkillExecutor::new();
        let ctx = SkillExecutionContext::new(temp.path())
            .with_args(vec!["--verbose".to_owned(), "target".to_owned()]);
        let result = ok(executor.execute_skill(&skill, &ctx));
        assert!(result.prompt.contains("--verbose"));
        assert!(result.prompt.contains("target"));
    }

    #[test]
    fn execute_skill_truncation() {
        let temp = ok(tempdir());
        let long_content = "x".repeat(2000);
        let skill = make_skill(temp.path(), "long", &long_content);
        let executor = SkillExecutor::with_max_prompt_length(100);
        let ctx = SkillExecutionContext::new(temp.path());
        let result = ok(executor.execute_skill(&skill, &ctx));
        assert!(result.prompt.len() <= 100);
        assert!(!result.warnings.is_empty());
    }

    #[test]
    fn execute_skill_missing_reference_warns() {
        let temp = ok(tempdir());
        let root = temp.path().join("warn-skill");
        fs::create_dir_all(&root).expect("dir");
        fs::write(root.join("SKILL.md"), "# Warn\n\nContent").expect("write");

        let missing_ref = root.join("references").join("missing.md");
        let skill = SkillDocument {
            metadata: SkillMetadata {
                slug: "warn-skill".to_owned(),
                title: "Warn Skill".to_owned(),
                summary: None,
                path: root.join("SKILL.md"),
                root: root.clone(),
                tags: vec![],
                tools: vec![],
                triggers: vec![],
                references: vec![missing_ref],
                scripts: vec![],
                assets: vec![],
            },
            instructions: "# Warn\n\nContent".to_owned(),
        };

        let executor = SkillExecutor::new();
        let ctx = SkillExecutionContext::new(temp.path());
        let result = ok(executor.execute_skill(&skill, &ctx));
        assert!(result.warnings.iter().any(|w| w.contains("not found")));
        assert_eq!(result.references_loaded, 0);
    }

    // --- build_preview ---

    #[test]
    fn build_preview_contains_title_and_summary() {
        let temp = ok(tempdir());
        let skill = make_skill(temp.path(), "preview", "# Preview\n\nContent here.");
        let executor = SkillExecutor::new();
        let preview = executor.build_preview(&skill);
        assert!(preview.contains("# Skill: preview"));
        assert!(preview.contains("Summary for preview"));
        assert!(preview.contains("Content here."));
    }

    // --- matches_query ---

    #[test]
    fn matches_query_by_slug() {
        let temp = ok(tempdir());
        let skill = make_skill(temp.path(), "deploy", "# Deploy\n\nDeploy stuff.");
        assert!(SkillExecutor::matches_query(&skill, "deploy"));
        assert!(SkillExecutor::matches_query(&skill, "DEP"));
    }

    #[test]
    fn matches_query_by_trigger() {
        let temp = ok(tempdir());
        let mut skill = make_skill(temp.path(), "git-skill", "# Git\n\nGit ops.");
        skill.metadata.triggers = vec!["git operations".to_owned(), "version control".to_owned()];
        assert!(SkillExecutor::matches_query(&skill, "git"));
        assert!(SkillExecutor::matches_query(&skill, "version"));
    }

    #[test]
    fn matches_query_negative() {
        let temp = ok(tempdir());
        let skill = make_skill(temp.path(), "docker", "# Docker\n\nContainer ops.");
        assert!(!SkillExecutor::matches_query(&skill, "kubernetes"));
    }

    // --- validate_skills (batch) ---

    #[test]
    fn validate_skills_batch() {
        let temp = ok(tempdir());
        let good = make_skill(temp.path(), "good", "# Good\n\nOK");
        let mut bad = make_skill(temp.path(), "bad", "");
        bad.metadata.slug = "bad".to_owned();

        let skills = [good, bad];
        let results = validate_skills(&skills);
        assert_eq!(results.len(), 2);
        assert!(results[0].1.is_ok());
        assert!(results[1].1.is_err());
    }

    // --- filter_skills_by_query ---

    #[test]
    fn filter_skills_by_query_works() {
        let temp = ok(tempdir());
        let s1 = make_skill(temp.path(), "deploy", "# Deploy\n\nDeploy");
        let s2 = make_skill(temp.path(), "review", "# Review\n\nReview");
        let s3 = make_skill(temp.path(), "deploy-prod", "# Deploy Prod\n\nProd deploy");

        let skills = [s1, s2, s3];
        let filtered = filter_skills_by_query(&skills, "deploy");
        assert_eq!(filtered.len(), 2);
    }

    // --- SkillExecutionContext ---

    #[test]
    fn execution_context_default() {
        let ctx = SkillExecutionContext::default();
        assert!(!ctx.working_dir.as_os_str().is_empty());
        assert!(ctx.env_vars.is_empty());
        assert!(ctx.args.is_empty());
        assert_eq!(ctx.max_prompt_length, 0);
    }

    #[test]
    fn execution_context_builder() {
        let ctx = SkillExecutionContext::new("/tmp")
            .with_env("KEY", "value")
            .with_args(vec!["arg1".to_owned()])
            .with_max_prompt_length(500);
        assert_eq!(ctx.working_dir, PathBuf::from("/tmp"));
        assert_eq!(ctx.env_vars.get("KEY"), Some(&"value".to_owned()));
        assert_eq!(ctx.args, vec!["arg1".to_owned()]);
        assert_eq!(ctx.max_prompt_length, 500);
    }

    // --- SkillExecutionResult serialization ---

    #[test]
    fn execution_result_serialization_roundtrip() {
        let result = SkillExecutionResult {
            slug: "test".to_owned(),
            prompt: "prompt text".to_owned(),
            references_loaded: 3,
            env_vars_interpolated: 1,
            valid: true,
            warnings: vec!["warn1".to_owned()],
        };
        let json = serde_json::to_string(&result).expect("serialize");
        let deserialized: SkillExecutionResult = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(result, deserialized);
    }

    // --- interpolation edge cases ---

    #[test]
    fn interpolation_no_match() {
        let executor = SkillExecutor::new();
        let (text, count) = executor.interpolate_env_vars("Hello {{UNKNOWN}}", &HashMap::new());
        assert_eq!(count, 0);
        assert_eq!(text, "Hello {{UNKNOWN}}");
    }

    #[test]
    fn interpolation_multiple_same_var() {
        let executor = SkillExecutor::new();
        let mut env = HashMap::new();
        env.insert("NAME".to_owned(), "Bob".to_owned());
        let (text, count) = executor.interpolate_env_vars("Hi {{NAME}}, bye {{NAME}}!", &env);
        assert_eq!(count, 1);
        assert_eq!(text, "Hi Bob, bye Bob!");
    }
}
