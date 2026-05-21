//! Bundled (built-in) skill registry.
//!
//! Provides an enumeration of all skills that ship with the binary by default,
//! together with helpers for resolving a skill name to either a bundled or
//! user-discovered skill.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{SkillDocument, SkillMetadata, discover_skills};

// ---------------------------------------------------------------------------
// BundledSkill enum
// ---------------------------------------------------------------------------

/// Enumeration of all bundled (built-in) skills.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BundledSkill {
    /// Update runtime configuration.
    UpdateConfig,
    /// One-click commit.
    Commit,
    /// Commit, push, and create PR.
    CommitPushPr,
    /// Simplify code.
    Simplify,
    /// Code review.
    Review,
    /// Run tests.
    Test,
    /// Tmux session management.
    Tmux,
    /// Git workflow.
    Git,
    /// Security audit.
    SecurityAudit,
    /// Performance profiling.
    Profile,
    /// Documentation generation.
    Docs,
    /// Refactoring.
    Refactor,
    /// Debugging assistance.
    Debug,
    /// Deployment.
    Deploy,
    /// Migration assistance.
    Migrate,
}

/// All bundled skill names as string slices.
pub const BUNDLED_SKILL_NAMES: &[&str] = &[
    "update-config",
    "commit",
    "commit-push-pr",
    "simplify",
    "review",
    "test",
    "tmux",
    "git",
    "security-audit",
    "profile",
    "docs",
    "refactor",
    "debug",
    "deploy",
    "migrate",
];

impl BundledSkill {
    /// Return all bundled skill variants.
    #[must_use]
    pub fn all() -> &'static [BundledSkill] {
        &[
            BundledSkill::UpdateConfig,
            BundledSkill::Commit,
            BundledSkill::CommitPushPr,
            BundledSkill::Simplify,
            BundledSkill::Review,
            BundledSkill::Test,
            BundledSkill::Tmux,
            BundledSkill::Git,
            BundledSkill::SecurityAudit,
            BundledSkill::Profile,
            BundledSkill::Docs,
            BundledSkill::Refactor,
            BundledSkill::Debug,
            BundledSkill::Deploy,
            BundledSkill::Migrate,
        ]
    }

    /// Return the kebab-case name of this bundled skill.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::UpdateConfig => "update-config",
            Self::Commit => "commit",
            Self::CommitPushPr => "commit-push-pr",
            Self::Simplify => "simplify",
            Self::Review => "review",
            Self::Test => "test",
            Self::Tmux => "tmux",
            Self::Git => "git",
            Self::SecurityAudit => "security-audit",
            Self::Profile => "profile",
            Self::Docs => "docs",
            Self::Refactor => "refactor",
            Self::Debug => "debug",
            Self::Deploy => "deploy",
            Self::Migrate => "migrate",
        }
    }

    /// Return a short description of this bundled skill.
    #[must_use]
    pub fn description(self) -> &'static str {
        match self {
            Self::UpdateConfig => "Update runtime configuration settings",
            Self::Commit => "One-click git commit with smart message",
            Self::CommitPushPr => "Commit, push, and create a pull request",
            Self::Simplify => "Simplify and clean up code",
            Self::Review => "Perform a code review",
            Self::Test => "Run and analyze test results",
            Self::Tmux => "Manage tmux sessions and panes",
            Self::Git => "Git workflow operations",
            Self::SecurityAudit => "Audit code for security vulnerabilities",
            Self::Profile => "Performance profiling and optimization",
            Self::Docs => "Generate documentation",
            Self::Refactor => "Refactor code for clarity and maintainability",
            Self::Debug => "Debugging assistance and diagnosis",
            Self::Deploy => "Deployment operations",
            Self::Migrate => "Code and infrastructure migration",
        }
    }

    /// Parse a bundled skill from a string.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "update-config" => Some(Self::UpdateConfig),
            "commit" => Some(Self::Commit),
            "commit-push-pr" => Some(Self::CommitPushPr),
            "simplify" => Some(Self::Simplify),
            "review" => Some(Self::Review),
            "test" => Some(Self::Test),
            "tmux" => Some(Self::Tmux),
            "git" => Some(Self::Git),
            "security-audit" => Some(Self::SecurityAudit),
            "profile" => Some(Self::Profile),
            "docs" => Some(Self::Docs),
            "refactor" => Some(Self::Refactor),
            "debug" => Some(Self::Debug),
            "deploy" => Some(Self::Deploy),
            "migrate" => Some(Self::Migrate),
            _ => None,
        }
    }

    /// Convert to a synthetic [`SkillMetadata`].
    #[must_use]
    pub fn to_metadata(self) -> SkillMetadata {
        SkillMetadata {
            slug: self.name().to_owned(),
            title: self
                .name()
                .split('-')
                .map(|s| {
                    let mut c = s.chars();
                    match c.next() {
                        Some(f) => f.to_ascii_uppercase().to_string() + c.as_str(),
                        None => String::new(),
                    }
                })
                .collect::<Vec<_>>()
                .join(" "),
            summary: Some(self.description().to_owned()),
            path: PathBuf::from(format!("bundled://{}", self.name())),
            root: PathBuf::from(format!("bundled://{}", self.name())),
            tags: vec!["bundled".to_owned()],
            tools: Vec::new(),
            triggers: vec![self.name().to_owned()],
            references: Vec::new(),
            scripts: Vec::new(),
            assets: Vec::new(),
        }
    }

    /// Convert to a synthetic [`SkillDocument`].
    #[must_use]
    pub fn to_document(self) -> SkillDocument {
        let instructions = format!(
            "# {}\n\n{}\n\nThis is a bundled skill. It is available by default.",
            self.name(),
            self.description()
        );
        SkillDocument {
            metadata: self.to_metadata(),
            instructions,
        }
    }
}

// ---------------------------------------------------------------------------
// Bundled skills registry
// ---------------------------------------------------------------------------

/// Return all bundled skills as [`SkillDocument`] values.
#[must_use]
pub fn get_bundled_skills() -> Vec<SkillDocument> {
    BundledSkill::all()
        .iter()
        .map(|skill| skill.to_document())
        .collect()
}

/// Resolve a skill by name: first check bundled skills, then discover
/// user-defined skills in the given directory.
pub fn resolve_skill(
    name: &str,
    search_dirs: &[&Path],
) -> Result<Option<SkillDocument>, crate::SkillError> {
    // Check bundled skills first
    if let Some(bundled) = BundledSkill::from_name(name) {
        return Ok(Some(bundled.to_document()));
    }

    // Search user-defined skill directories
    for dir in search_dirs {
        if !dir.exists() {
            continue;
        }
        let skills = discover_skills(dir)?;
        for skill in skills {
            if skill.metadata.slug == name {
                return Ok(Some(skill));
            }
        }
    }

    Ok(None)
}

/// List all available skills (bundled + user-defined).
pub fn list_all_skills(search_dirs: &[&Path]) -> Result<Vec<SkillDocument>, crate::SkillError> {
    let mut all = get_bundled_skills();

    for dir in search_dirs {
        if !dir.exists() {
            continue;
        }
        let discovered = discover_skills(dir)?;
        all.extend(discovered);
    }

    all.sort_by(|a, b| a.metadata.slug.cmp(&b.metadata.slug));
    Ok(all)
}

/// Check if a skill name refers to a bundled skill.
#[must_use]
pub fn is_bundled(name: &str) -> bool {
    BundledSkill::from_name(name).is_some()
}

/// Count bundled skills.
#[must_use]
pub fn bundled_skill_count() -> usize {
    BundledSkill::all().len()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_skill_names_match_variants() {
        assert_eq!(BUNDLED_SKILL_NAMES.len(), BundledSkill::all().len());
        for (variant, name) in BundledSkill::all().iter().zip(BUNDLED_SKILL_NAMES.iter()) {
            assert_eq!(variant.name(), *name);
        }
    }

    #[test]
    fn from_name_roundtrip() {
        for skill in BundledSkill::all() {
            let name = skill.name();
            assert_eq!(BundledSkill::from_name(name), Some(*skill));
        }
    }

    #[test]
    fn from_name_unknown() {
        assert_eq!(BundledSkill::from_name("nonexistent"), None);
        assert_eq!(BundledSkill::from_name(""), None);
    }

    #[test]
    fn description_not_empty() {
        for skill in BundledSkill::all() {
            assert!(!skill.description().is_empty());
        }
    }

    #[test]
    fn to_metadata_has_bundled_tag() {
        for skill in BundledSkill::all() {
            let meta = skill.to_metadata();
            assert!(meta.tags.contains(&"bundled".to_owned()));
            assert!(!meta.slug.is_empty());
            assert!(meta.summary.is_some());
        }
    }

    #[test]
    fn to_document_has_instructions() {
        for skill in BundledSkill::all() {
            let doc = skill.to_document();
            assert!(!doc.instructions.is_empty());
            assert!(doc.instructions.contains("bundled skill"));
        }
    }

    #[test]
    fn get_bundled_skills_count() {
        let skills = get_bundled_skills();
        assert_eq!(skills.len(), BUNDLED_SKILL_NAMES.len());
    }

    #[test]
    fn is_bundled_true() {
        assert!(is_bundled("commit"));
        assert!(is_bundled("review"));
        assert!(is_bundled("test"));
    }

    #[test]
    fn is_bundled_false() {
        assert!(!is_bundled("my-custom-skill"));
        assert!(!is_bundled(""));
    }

    #[test]
    fn bundled_skill_count_matches() {
        assert_eq!(bundled_skill_count(), BUNDLED_SKILL_NAMES.len());
        assert_eq!(bundled_skill_count(), 15);
    }

    #[test]
    fn resolve_skill_bundled() {
        let result = resolve_skill("commit", &[]).expect("resolve");
        assert!(result.is_some());
        let skill = result.expect("skill");
        assert_eq!(skill.metadata.slug, "commit");
    }

    #[test]
    fn resolve_skill_not_found() {
        let result = resolve_skill("nonexistent", &[]).expect("resolve");
        assert!(result.is_none());
    }

    #[test]
    fn resolve_skill_user_defined() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("my-skill");
        std::fs::create_dir_all(&root).expect("dir");
        std::fs::write(root.join("SKILL.md"), "# My Skill\n\nCustom skill.").expect("write");

        let result = resolve_skill("my-skill", &[temp.path()]).expect("resolve");
        assert!(result.is_some());
        let skill = result.expect("skill");
        assert_eq!(skill.metadata.slug, "my-skill");
    }

    #[test]
    fn list_all_skills_includes_bundled() {
        let skills = list_all_skills(&[]).expect("list");
        assert!(skills.len() >= BUNDLED_SKILL_NAMES.len());
    }

    #[test]
    fn list_all_skills_sorted() {
        let skills = list_all_skills(&[]).expect("list");
        for window in skills.windows(2) {
            assert!(window[0].metadata.slug <= window[1].metadata.slug);
        }
    }

    #[test]
    fn bundled_skill_serialization_roundtrip() {
        for skill in BundledSkill::all() {
            let json = serde_json::to_string(skill).expect("serialize");
            let deserialized: BundledSkill = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(*skill, deserialized);
        }
    }
}
