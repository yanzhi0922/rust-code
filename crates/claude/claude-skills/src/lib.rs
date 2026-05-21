//! Skill discovery and SKILL.md parsing.
//!
//! Walks the file system to discover skill directories, parses SKILL.md files
//! with optional TOML front matter, and extracts metadata such as triggers,
//! descriptions, and reference file paths.

pub mod bundled;
pub mod executor;
pub mod search;

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use regex::Regex;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use walkdir::WalkDir;

/// File name used for skill definitions.
pub const SKILL_FILE_NAME: &str = "SKILL.md";
/// Default lock file name for installed skills.
pub const DEFAULT_SKILL_LOCK_FILE: &str = ".skill-lock.json";

/// Metadata extracted from a SKILL.md file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SkillMetadata {
    /// URL-friendly slug (derived from directory name or front matter).
    pub slug: String,
    /// Human-readable title.
    pub title: String,
    /// Optional short description.
    #[serde(default)]
    pub summary: Option<String>,
    /// Path to the SKILL.md file.
    pub path: PathBuf,
    /// Root directory of the skill.
    pub root: PathBuf,
    /// Tags for categorisation.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Tools this skill uses.
    #[serde(default)]
    pub tools: Vec<String>,
    /// Trigger phrases that activate this skill.
    #[serde(default)]
    pub triggers: Vec<String>,
    /// Referenced file paths within the skill directory.
    #[serde(default)]
    pub references: Vec<PathBuf>,
    /// Script file paths.
    #[serde(default)]
    pub scripts: Vec<PathBuf>,
    /// Asset file paths.
    #[serde(default)]
    pub assets: Vec<PathBuf>,
}

/// A fully loaded skill document (metadata + instructions).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillDocument {
    /// Skill metadata.
    pub metadata: SkillMetadata,
    /// Raw instruction text from the SKILL.md body.
    pub instructions: String,
}

/// Lock file tracking installed skills.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillLockFile {
    /// Lock file format version.
    pub version: u32,
    /// Map of skill slug → install record.
    #[serde(default)]
    pub skills: BTreeMap<String, InstalledSkillRecord>,
}

/// Record of a single installed skill in the lock file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledSkillRecord {
    /// Source identifier (e.g. GitHub repo URL).
    pub source: String,
    /// Source type.
    #[serde(rename = "sourceType")]
    pub source_type: SkillSourceKind,
    /// Source URL, if applicable.
    #[serde(rename = "sourceUrl")]
    pub source_url: Option<String>,
    /// Local path to the skill directory.
    #[serde(rename = "skillPath")]
    pub skill_path: PathBuf,
    /// Hash of the skill folder contents.
    #[serde(rename = "skillFolderHash")]
    pub skill_folder_hash: Option<String>,
    /// Installation timestamp.
    #[serde(rename = "installedAt")]
    pub installed_at: Option<String>,
    /// Last update timestamp.
    #[serde(rename = "updatedAt")]
    pub updated_at: Option<String>,
}

/// Origin type of an installed skill.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillSourceKind {
    /// Installed from a GitHub repository.
    Github,
    /// Installed from a local path.
    Local,
    /// Bundled with a plugin.
    Plugin,
    /// Installed from a directory.
    Directory,
    /// Unknown source.
    #[serde(other)]
    Unknown,
}

/// Errors that can occur during skill discovery and loading.
#[derive(Debug, Error)]
pub enum SkillError {
    #[error("failed to read skill file `{path}`")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("skill `{path}` uses an unterminated TOML front matter block")]
    UnterminatedFrontMatter { path: PathBuf },
    #[error("failed to parse TOML front matter in `{path}`")]
    FrontMatter {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("failed to read skill lock file `{path}`")]
    ReadLock {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse skill lock file `{path}`")]
    ParseLock {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

#[derive(Debug, Deserialize, Default)]
struct SkillFrontMatter {
    name: Option<String>,
    title: Option<String>,
    summary: Option<String>,
    description: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    tools: Vec<String>,
    #[serde(default)]
    triggers: Vec<String>,
}

pub fn discover_skills(root: &Path) -> Result<Vec<SkillDocument>, SkillError> {
    let mut skills = WalkDir::new(root)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .filter(|entry| entry.file_name() == SKILL_FILE_NAME)
        .map(|entry| load_skill(entry.path()))
        .collect::<Result<Vec<_>, _>>()?;

    skills.sort_by(|left, right| left.metadata.slug.cmp(&right.metadata.slug));
    Ok(skills)
}

pub fn load_skill(path: impl AsRef<Path>) -> Result<SkillDocument, SkillError> {
    let path = path.as_ref();
    let instructions = fs::read_to_string(path).map_err(|source| SkillError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let (front_matter, body) = split_front_matter(path, &instructions)?;
    let root = path
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    let slug = root.file_name().map_or_else(
        || "unknown-skill".to_owned(),
        |value| value.to_string_lossy().into_owned(),
    );
    let title = front_matter
        .title
        .clone()
        .or(front_matter.name.clone())
        .or_else(|| extract_heading(body))
        .unwrap_or_else(|| slug.clone());
    let summary = front_matter
        .summary
        .clone()
        .or(front_matter.description.clone())
        .or_else(|| extract_summary(body));
    let triggers = if front_matter.triggers.is_empty() {
        extract_triggers(body)
    } else {
        front_matter.triggers.clone()
    };
    let references = discover_reference_paths(&root, body);
    let scripts = discover_files(root.join("scripts"));
    let assets = discover_files(root.join("assets"));

    Ok(SkillDocument {
        metadata: SkillMetadata {
            slug,
            title,
            summary,
            path: path.to_path_buf(),
            root,
            tags: front_matter.tags,
            tools: front_matter.tools,
            triggers,
            references,
            scripts,
            assets,
        },
        instructions,
    })
}

pub fn load_skill_lock_file(path: impl AsRef<Path>) -> Result<SkillLockFile, SkillError> {
    let path = path.as_ref();
    let content = fs::read_to_string(path).map_err(|source| SkillError::ReadLock {
        path: path.to_path_buf(),
        source,
    })?;
    serde_json::from_str(&content).map_err(|source| SkillError::ParseLock {
        path: path.to_path_buf(),
        source,
    })
}

fn split_front_matter<'a>(
    path: &Path,
    content: &'a str,
) -> Result<(SkillFrontMatter, &'a str), SkillError> {
    if !content.starts_with("+++\n") {
        return Ok((SkillFrontMatter::default(), content));
    }

    let Some(rest) = content.strip_prefix("+++\n") else {
        return Ok((SkillFrontMatter::default(), content));
    };
    let Some((front_matter, body)) = rest.split_once("\n+++\n") else {
        return Err(SkillError::UnterminatedFrontMatter {
            path: path.to_path_buf(),
        });
    };
    let front_matter = toml::from_str(front_matter).map_err(|source| SkillError::FrontMatter {
        path: path.to_path_buf(),
        source,
    })?;
    Ok((front_matter, body))
}

fn extract_heading(body: &str) -> Option<String> {
    body.lines()
        .map(str::trim)
        .find(|line| line.starts_with("# "))
        .map(|line| line.trim_start_matches("# ").trim().to_owned())
}

fn extract_summary(body: &str) -> Option<String> {
    let mut paragraph = Vec::new();

    for line in body.lines().map(str::trim) {
        if line.is_empty() {
            if !paragraph.is_empty() {
                break;
            }
            continue;
        }
        if paragraph.is_empty()
            && (line.starts_with('#')
                || line.starts_with("```")
                || line.starts_with('-')
                || line.starts_with('*')
                || line.starts_with('|')
                || line.starts_with('>'))
        {
            continue;
        }
        paragraph.push(line);
    }

    if paragraph.is_empty() {
        None
    } else {
        Some(paragraph.join(" "))
    }
}

fn extract_triggers(body: &str) -> Vec<String> {
    let prefixes = ["use when:", "when:", "triggers:"];
    body.lines()
        .map(str::trim)
        .filter_map(|line| {
            let lower = line.to_ascii_lowercase();
            prefixes.iter().find_map(|prefix| {
                lower
                    .strip_prefix(prefix)
                    .map(|_| split_trigger_line(&line[prefix.len()..]))
            })
        })
        .flatten()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn split_trigger_line(line: &str) -> Vec<String> {
    line.split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_owned)
        .collect()
}

fn discover_reference_paths(root: &Path, body: &str) -> Vec<PathBuf> {
    let mut references = discover_files(root.join("references"))
        .into_iter()
        .collect::<BTreeSet<_>>();

    let pattern = match Regex::new(r"\[[^\]]+\]\(([^)]+)\)") {
        Ok(pattern) => pattern,
        Err(_) => return references.into_iter().collect(),
    };

    for capture in pattern.captures_iter(body) {
        let Some(matched) = capture.get(1) else {
            continue;
        };
        let target = matched.as_str().trim();
        if target.is_empty()
            || target.starts_with('#')
            || target.starts_with("http://")
            || target.starts_with("https://")
            || target.starts_with("mailto:")
        {
            continue;
        }
        references.insert(root.join(target));
    }

    references.into_iter().collect()
}

fn discover_files(root: PathBuf) -> Vec<PathBuf> {
    if !root.exists() {
        return Vec::new();
    }

    let mut files = WalkDir::new(root)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .map(walkdir::DirEntry::into_path)
        .collect::<Vec<_>>();
    files.sort();
    files
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn ok<T, E: std::fmt::Display>(result: Result<T, E>) -> T {
        match result {
            Ok(value) => value,
            Err(error) => panic!("unexpected error: {error}"),
        }
    }

    #[test]
    fn loads_skill_with_heading_summary_and_assets() {
        let temp = ok(tempdir());
        let root = temp.path().join("skills").join("demo-skill");
        ok(fs::create_dir_all(root.join("references")));
        ok(fs::create_dir_all(root.join("scripts")));
        ok(fs::create_dir_all(root.join("assets")));
        ok(fs::write(root.join("references").join("guide.md"), "guide"));
        ok(fs::write(
            root.join("scripts").join("run.ps1"),
            "Write-Host hi",
        ));
        ok(fs::write(root.join("assets").join("logo.txt"), "logo"));
        ok(fs::write(
            root.join(SKILL_FILE_NAME),
            "# Demo Skill\n\nA practical skill for demos.\n\nUse when: demo work, paired sessions\n\nSee [Guide](references/guide.md).\n",
        ));

        let skill = ok(load_skill(root.join(SKILL_FILE_NAME)));

        assert_eq!(skill.metadata.slug, "demo-skill");
        assert_eq!(skill.metadata.title, "Demo Skill");
        assert_eq!(
            skill.metadata.summary.as_deref(),
            Some("A practical skill for demos.")
        );
        assert_eq!(
            skill.metadata.triggers,
            vec!["demo work".to_owned(), "paired sessions".to_owned()]
        );
        assert_eq!(skill.metadata.references.len(), 1);
        assert_eq!(skill.metadata.scripts.len(), 1);
        assert_eq!(skill.metadata.assets.len(), 1);
    }

    #[test]
    fn front_matter_overrides_derived_metadata() {
        let temp = ok(tempdir());
        let root = temp.path().join("frontmatter-skill");
        ok(fs::create_dir_all(&root));
        ok(fs::write(
            root.join(SKILL_FILE_NAME),
            "+++\nname = \"Front Matter Skill\"\nsummary = \"Front summary\"\ntags = [\"alpha\", \"beta\"]\ntools = [\"shell\"]\ntriggers = [\"front matter\"]\n+++\n# Ignored Heading\n\nIgnored paragraph.\n",
        ));

        let skill = ok(load_skill(root.join(SKILL_FILE_NAME)));

        assert_eq!(skill.metadata.title, "Front Matter Skill");
        assert_eq!(skill.metadata.summary.as_deref(), Some("Front summary"));
        assert_eq!(
            skill.metadata.tags,
            vec!["alpha".to_owned(), "beta".to_owned()]
        );
        assert_eq!(skill.metadata.tools, vec!["shell".to_owned()]);
        assert_eq!(skill.metadata.triggers, vec!["front matter".to_owned()]);
    }

    #[test]
    fn discovers_multiple_skills_sorted_by_slug() {
        let temp = ok(tempdir());
        let alpha = temp.path().join("alpha");
        let zeta = temp.path().join("nested").join("zeta");
        ok(fs::create_dir_all(&alpha));
        ok(fs::create_dir_all(&zeta));
        ok(fs::write(
            alpha.join(SKILL_FILE_NAME),
            "# Alpha\n\nAlpha summary.\n",
        ));
        ok(fs::write(
            zeta.join(SKILL_FILE_NAME),
            "# Zeta\n\nZeta summary.\n",
        ));

        let skills = ok(discover_skills(temp.path()));

        assert_eq!(skills.len(), 2);
        assert_eq!(skills[0].metadata.slug, "alpha");
        assert_eq!(skills[1].metadata.slug, "zeta");
    }

    #[test]
    fn parses_skill_lock_file() {
        let temp = ok(tempdir());
        let lock_path = temp.path().join(".skill-lock.json");
        ok(fs::write(
            &lock_path,
            r#"{
                "version": 3,
                "skills": {
                    "demo": {
                        "source": "example/demo",
                        "sourceType": "github",
                        "sourceUrl": "https://example.com/demo.git",
                        "skillPath": "skills/demo/SKILL.md",
                        "skillFolderHash": "abc123",
                        "installedAt": "2026-04-07T00:00:00Z",
                        "updatedAt": "2026-04-07T01:00:00Z"
                    }
                }
            }"#,
        ));

        let lock_file = ok(load_skill_lock_file(lock_path));
        let record = match lock_file.skills.get("demo") {
            Some(record) => record,
            None => panic!("missing demo record"),
        };

        assert_eq!(lock_file.version, 3);
        assert_eq!(record.source_type, SkillSourceKind::Github);
        assert_eq!(record.skill_path, PathBuf::from("skills/demo/SKILL.md"));
    }
}
