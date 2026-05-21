use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use anyhow::{Result, anyhow};
use claude_config::{RuntimeConfig, SettingSource};
use claude_skills::{DEFAULT_SKILL_LOCK_FILE, SkillMetadata, load_skill_lock_file};

use crate::cli::{SkillsCommand, SkillsIndexArgs, SkillsListArgs, SkillsLockArgs, SkillsShowArgs};

const DEFAULT_SKILL_INDEX_FILE: &str = ".skill-index.json";

#[derive(Debug, Clone, serde::Serialize)]
struct RuntimeSkillRecord {
    slug: String,
    title: String,
    summary: Option<String>,
    origin_kind: String,
    origin_name: String,
    path: PathBuf,
    root: PathBuf,
    tools: Vec<String>,
    triggers: Vec<String>,
    references: Vec<PathBuf>,
    scripts: Vec<PathBuf>,
    assets: Vec<PathBuf>,
}

#[derive(Debug, Clone, serde::Serialize)]
struct SkillsListOutput {
    warnings: Vec<String>,
    skills: Vec<RuntimeSkillRecord>,
}

#[derive(Debug, Clone, serde::Serialize)]
struct SkillShowOutput {
    warnings: Vec<String>,
    skill: RuntimeSkillRecord,
    #[serde(skip_serializing_if = "Option::is_none")]
    instructions: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
struct SkillsLockOutput {
    path: PathBuf,
    exists: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    lock: Option<claude_skills::SkillLockFile>,
}

#[derive(Debug, Clone, serde::Serialize)]
struct SkillsIndexOutput {
    warnings: Vec<String>,
    total_skills: usize,
    unique_slugs: usize,
    duplicate_slugs: Vec<String>,
    origin_counts: BTreeMap<String, usize>,
    with_references: usize,
    with_scripts: usize,
    with_assets: usize,
    lock_path: PathBuf,
    lock_exists: bool,
    lock_covered_slugs: usize,
    uncached_slugs: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_path: Option<PathBuf>,
    skills: Vec<RuntimeSkillRecord>,
}

pub(crate) fn run_skills(config: &RuntimeConfig, command: SkillsCommand) -> Result<()> {
    match command {
        SkillsCommand::List(args) => run_skills_list(config, args),
        SkillsCommand::Show(args) => run_skills_show(config, args),
        SkillsCommand::Lock(args) => run_skills_lock(config, args),
        SkillsCommand::Index(args) => run_skills_index(config, args),
    }
}

fn run_skills_list(config: &RuntimeConfig, args: SkillsListArgs) -> Result<()> {
    let output = build_skills_list_output(config, !args.no_plugins);
    if args.json {
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }

    if output.skills.is_empty() {
        println!("No skills found.");
        for warning in output.warnings {
            println!("  - {warning}");
        }
        return Ok(());
    }

    for warning in &output.warnings {
        println!("warning: {warning}");
    }
    for skill in &output.skills {
        println!(
            "{}  {}  {} ({})",
            skill.slug,
            skill.title,
            skill.origin_kind,
            skill.path.display()
        );
        if let Some(summary) = &skill.summary {
            println!("  summary: {summary}");
        }
        if !skill.triggers.is_empty() {
            println!("  triggers: {}", skill.triggers.join(", "));
        }
        if !skill.tools.is_empty() {
            println!("  tools: {}", skill.tools.join(", "));
        }
        if skill.origin_kind == "plugin" {
            println!("  plugin: {}", skill.origin_name);
        }
    }
    Ok(())
}

fn run_skills_show(config: &RuntimeConfig, args: SkillsShowArgs) -> Result<()> {
    let output = build_skill_show_output(
        config,
        &args.skill,
        !args.no_plugins,
        args.include_instructions,
    )?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }

    for warning in &output.warnings {
        println!("warning: {warning}");
    }
    let skill = &output.skill;
    println!("skill: {} ({})", skill.slug, skill.title);
    println!("origin: {} {}", skill.origin_kind, skill.origin_name);
    println!("path: {}", skill.path.display());
    println!("root: {}", skill.root.display());
    if let Some(summary) = &skill.summary {
        println!("summary: {summary}");
    }
    if !skill.triggers.is_empty() {
        println!("triggers: {}", skill.triggers.join(", "));
    }
    if !skill.tools.is_empty() {
        println!("tools: {}", skill.tools.join(", "));
    }
    println!("references: {}", skill.references.len());
    println!("scripts: {}", skill.scripts.len());
    println!("assets: {}", skill.assets.len());
    if let Some(instructions) = &output.instructions {
        println!("instructions:");
        for line in instructions.lines() {
            println!("  {line}");
        }
    }
    Ok(())
}

fn run_skills_lock(config: &RuntimeConfig, args: SkillsLockArgs) -> Result<()> {
    let path = config.paths.profile_dir.join(DEFAULT_SKILL_LOCK_FILE);
    let output = if path.exists() {
        SkillsLockOutput {
            path: path.clone(),
            exists: true,
            lock: Some(load_skill_lock_file(&path)?),
        }
    } else {
        SkillsLockOutput {
            path,
            exists: false,
            lock: None,
        }
    };

    if args.json {
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }

    println!("lock path: {}", output.path.display());
    println!("exists: {}", output.exists);
    if let Some(lock) = output.lock {
        println!("version: {}", lock.version);
        println!("installed skills: {}", lock.skills.len());
        for (slug, record) in lock.skills.iter().take(20) {
            println!(
                "  {}  {}  {}",
                slug,
                record.source,
                record.skill_path.display()
            );
        }
    }
    Ok(())
}

fn run_skills_index(config: &RuntimeConfig, args: SkillsIndexArgs) -> Result<()> {
    let mut output = build_skills_index_output(config, !args.no_plugins)?;
    let cache_path = args.output.clone().or_else(|| {
        args.write_cache
            .then(|| config.paths.profile_dir.join(DEFAULT_SKILL_INDEX_FILE))
    });
    if let Some(path) = &cache_path {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        output.cache_path = Some(path.clone());
        std::fs::write(path, serde_json::to_vec_pretty(&output)?)?;
    }

    if args.json {
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }

    for warning in &output.warnings {
        println!("warning: {warning}");
    }
    println!(
        "skills: total={} unique={} duplicates={}",
        output.total_skills,
        output.unique_slugs,
        output.duplicate_slugs.len()
    );
    if !output.origin_counts.is_empty() {
        println!(
            "origins: {}",
            output
                .origin_counts
                .iter()
                .map(|(origin, count)| format!("{origin}={count}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    println!(
        "coverage: refs={} scripts={} assets={}",
        output.with_references, output.with_scripts, output.with_assets
    );
    println!(
        "lock: {} covered={} missing={}",
        output.lock_path.display(),
        output.lock_covered_slugs,
        output.uncached_slugs.len()
    );
    if !output.duplicate_slugs.is_empty() {
        println!("duplicate slugs: {}", output.duplicate_slugs.join(", "));
    }
    if !output.uncached_slugs.is_empty() {
        println!("uncached slugs: {}", output.uncached_slugs.join(", "));
    }
    if let Some(path) = &output.cache_path {
        println!("cache: {}", path.display());
    }
    Ok(())
}

fn build_skills_list_output(config: &RuntimeConfig, include_plugins: bool) -> SkillsListOutput {
    let mut warnings = Vec::new();
    let mut skills = Vec::new();
    let mut seen = BTreeSet::new();
    let user_sources_enabled = setting_source_enabled(config, SettingSource::User);

    if user_sources_enabled && config.paths.skills_dir.exists() {
        match claude_skills::discover_skills(&config.paths.skills_dir) {
            Ok(discovered) => {
                for skill in discovered {
                    let record = skill_record("profile", "profile", &skill.metadata);
                    seen.insert(record.slug.clone());
                    skills.push(record);
                }
            }
            Err(error) => warnings.push(format!(
                "Failed to discover profile skills in {}: {error}",
                config.paths.skills_dir.display()
            )),
        }
    }

    if include_plugins && user_sources_enabled && config.paths.plugins_dir.exists() {
        match claude_plugins::discover_plugins(&config.paths.plugins_dir) {
            Ok(plugins) => {
                for plugin in plugins {
                    match plugin.discover_bundled_skills() {
                        Ok(discovered) => {
                            for skill in discovered {
                                if seen.contains(&skill.metadata.slug) {
                                    warnings.push(format!(
                                        "Duplicate skill slug `{}` discovered in plugin {}",
                                        skill.metadata.slug, plugin.manifest.name
                                    ));
                                }
                                skills.push(skill_record(
                                    "plugin",
                                    &plugin.manifest.name,
                                    &skill.metadata,
                                ));
                            }
                        }
                        Err(error) => warnings.push(format!(
                            "Failed to discover skills in plugin {}: {error}",
                            plugin.manifest.name
                        )),
                    }
                }
            }
            Err(error) => warnings.push(format!(
                "Failed to discover plugins in {}: {error}",
                config.paths.plugins_dir.display()
            )),
        }
    }

    skills.sort_by(|left, right| {
        left.slug
            .cmp(&right.slug)
            .then_with(|| left.origin_kind.cmp(&right.origin_kind))
            .then_with(|| left.origin_name.cmp(&right.origin_name))
    });

    SkillsListOutput { warnings, skills }
}

fn build_skills_index_output(
    config: &RuntimeConfig,
    include_plugins: bool,
) -> Result<SkillsIndexOutput> {
    let list_output = build_skills_list_output(config, include_plugins);
    let lock_path = config.paths.profile_dir.join(DEFAULT_SKILL_LOCK_FILE);
    let lock = if lock_path.exists() {
        Some(load_skill_lock_file(&lock_path)?)
    } else {
        None
    };

    let mut slug_counts = BTreeMap::new();
    let mut origin_counts = BTreeMap::new();
    let mut with_references = 0;
    let mut with_scripts = 0;
    let mut with_assets = 0;

    for skill in &list_output.skills {
        *slug_counts.entry(skill.slug.clone()).or_insert(0usize) += 1;
        *origin_counts
            .entry(skill.origin_kind.clone())
            .or_insert(0usize) += 1;
        if !skill.references.is_empty() {
            with_references += 1;
        }
        if !skill.scripts.is_empty() {
            with_scripts += 1;
        }
        if !skill.assets.is_empty() {
            with_assets += 1;
        }
    }

    let duplicate_slugs = slug_counts
        .iter()
        .filter_map(|(slug, count)| (*count > 1).then_some(slug.clone()))
        .collect::<Vec<_>>();
    let uncached_slugs = match &lock {
        Some(lock) => list_output
            .skills
            .iter()
            .filter(|skill| !lock.skills.contains_key(&skill.slug))
            .map(|skill| skill.slug.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>(),
        None => list_output
            .skills
            .iter()
            .map(|skill| skill.slug.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>(),
    };
    let lock_covered_slugs = list_output
        .skills
        .iter()
        .map(|skill| skill.slug.as_str())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|slug| {
            lock.as_ref()
                .is_some_and(|lock| lock.skills.contains_key(*slug))
        })
        .count();

    Ok(SkillsIndexOutput {
        warnings: list_output.warnings,
        total_skills: list_output.skills.len(),
        unique_slugs: slug_counts.len(),
        duplicate_slugs,
        origin_counts,
        with_references,
        with_scripts,
        with_assets,
        lock_path,
        lock_exists: lock.is_some(),
        lock_covered_slugs,
        uncached_slugs,
        cache_path: None,
        skills: list_output.skills,
    })
}

fn build_skill_show_output(
    config: &RuntimeConfig,
    slug: &str,
    include_plugins: bool,
    include_instructions: bool,
) -> Result<SkillShowOutput> {
    let mut warnings = Vec::new();
    let mut matches = Vec::new();
    let user_sources_enabled = setting_source_enabled(config, SettingSource::User);

    if user_sources_enabled && config.paths.skills_dir.exists() {
        match claude_skills::discover_skills(&config.paths.skills_dir) {
            Ok(discovered) => {
                matches.extend(
                    discovered
                        .into_iter()
                        .filter(|skill| skill.metadata.slug == slug)
                        .map(|skill| ("profile".to_owned(), "profile".to_owned(), skill)),
                );
            }
            Err(error) => warnings.push(format!("Failed to discover profile skills: {error}")),
        }
    }

    if include_plugins && user_sources_enabled && config.paths.plugins_dir.exists() {
        match claude_plugins::discover_plugins(&config.paths.plugins_dir) {
            Ok(plugins) => {
                for plugin in plugins {
                    match plugin.discover_bundled_skills() {
                        Ok(discovered) => {
                            matches.extend(
                                discovered
                                    .into_iter()
                                    .filter(|skill| skill.metadata.slug == slug)
                                    .map(|skill| {
                                        ("plugin".to_owned(), plugin.manifest.name.clone(), skill)
                                    }),
                            );
                        }
                        Err(error) => warnings.push(format!(
                            "Failed to discover skills in plugin {}: {error}",
                            plugin.manifest.name
                        )),
                    }
                }
            }
            Err(error) => warnings.push(format!("Failed to discover plugins: {error}")),
        }
    }

    match matches.len() {
        0 => Err(anyhow!("No skill named `{slug}` was found")),
        1 => {
            let (origin_kind, origin_name, skill) = matches.pop().expect("single skill");
            Ok(SkillShowOutput {
                warnings,
                skill: skill_record(&origin_kind, &origin_name, &skill.metadata),
                instructions: include_instructions.then_some(skill.instructions),
            })
        }
        _ => Err(anyhow!(
            "Skill `{slug}` is ambiguous across: {}",
            matches
                .into_iter()
                .map(|(origin_kind, origin_name, skill)| format!(
                    "{}:{} ({})",
                    origin_kind,
                    origin_name,
                    skill.metadata.path.display()
                ))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

fn skill_record(
    origin_kind: &str,
    origin_name: &str,
    metadata: &SkillMetadata,
) -> RuntimeSkillRecord {
    RuntimeSkillRecord {
        slug: metadata.slug.clone(),
        title: metadata.title.clone(),
        summary: metadata.summary.clone(),
        origin_kind: origin_kind.to_owned(),
        origin_name: origin_name.to_owned(),
        path: metadata.path.clone(),
        root: metadata.root.clone(),
        tools: metadata.tools.clone(),
        triggers: metadata.triggers.clone(),
        references: metadata.references.clone(),
        scripts: metadata.scripts.clone(),
        assets: metadata.assets.clone(),
    }
}

fn setting_source_enabled(config: &RuntimeConfig, source: SettingSource) -> bool {
    config.allowed_setting_sources.contains(&source)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use claude_config::{ProviderOverrides, RuntimeOverrides, SettingSource, load_runtime_config};
    use claude_skills::DEFAULT_SKILL_LOCK_FILE;
    use tempfile::tempdir;

    use super::{build_skill_show_output, build_skills_index_output, build_skills_list_output};

    fn test_config() -> (tempfile::TempDir, claude_config::RuntimeConfig) {
        let tempdir = tempdir().expect("tempdir");
        let cwd = tempdir.path().join("workspace");
        let profile = tempdir.path().join(".remote-code-rust");
        fs::create_dir_all(cwd.join(".")).expect("cwd");
        fs::create_dir_all(profile.join("skills").join("demo")).expect("skills");
        fs::create_dir_all(profile.join("plugins").join("sample").join(".codex-plugin"))
            .expect("plugin dir");
        fs::create_dir_all(
            profile
                .join("plugins")
                .join("sample")
                .join("skills")
                .join("extra"),
        )
        .expect("plugin skills");
        fs::write(
            profile.join("skills").join("demo").join("SKILL.md"),
            "# Demo\n\nSummary.\n",
        )
        .expect("write skill");
        fs::write(
            profile
                .join("plugins")
                .join("sample")
                .join(".codex-plugin")
                .join("plugin.json"),
            r#"{"name":"sample","version":"0.1.0","skills":"./skills"}"#,
        )
        .expect("write plugin manifest");
        fs::write(
            profile
                .join("plugins")
                .join("sample")
                .join("skills")
                .join("extra")
                .join("SKILL.md"),
            "# Extra\n\nPlugin summary.\n",
        )
        .expect("write plugin skill");
        let config = load_runtime_config(
            Some(cwd),
            Some(profile),
            None,
            claude_core::PermissionMode::Default,
            claude_core::InputFormat::Text,
            claude_core::OutputFormat::Text,
            false,
            false,
            false,
            false,
            4,
            ProviderOverrides::default(),
            RuntimeOverrides::default(),
        )
        .expect("config");
        (tempdir, config)
    }

    #[test]
    fn skills_list_includes_profile_skill() {
        let (_tempdir, config) = test_config();
        let output = build_skills_list_output(&config, false);
        assert_eq!(output.skills.len(), 1);
        assert_eq!(output.skills[0].slug, "demo");
    }

    #[test]
    fn skills_show_returns_single_skill() {
        let (_tempdir, config) = test_config();
        let output = build_skill_show_output(&config, "demo", false, true).expect("skill show");
        assert_eq!(output.skill.slug, "demo");
        assert!(output.instructions.is_some());
        assert_eq!(output.skill.references.len(), 0);
        assert_eq!(output.skill.scripts.len(), 0);
        assert_eq!(output.skill.assets.len(), 0);
    }

    #[test]
    fn skills_outputs_respect_user_setting_sources() {
        let (_tempdir, mut config) = test_config();
        let output = build_skills_list_output(&config, true);
        assert_eq!(output.skills.len(), 2);

        config.allowed_setting_sources = vec![SettingSource::Local];
        let output = build_skills_list_output(&config, true);
        assert!(output.skills.is_empty());
        assert!(build_skill_show_output(&config, "demo", true, false).is_err());
    }

    #[test]
    fn skills_index_reports_lock_and_duplicate_state() {
        let (_tempdir, config) = test_config();
        fs::write(
            config.paths.profile_dir.join(DEFAULT_SKILL_LOCK_FILE),
            r#"{
                "version": 1,
                "skills": {
                    "demo": {
                        "source": "local/demo",
                        "sourceType": "local",
                        "sourceUrl": null,
                        "skillPath": "skills/demo/SKILL.md",
                        "skillFolderHash": null,
                        "installedAt": null,
                        "updatedAt": null
                    }
                }
            }"#,
        )
        .expect("write lock");

        let output = build_skills_index_output(&config, false).expect("skills index");
        assert_eq!(output.total_skills, 1);
        assert_eq!(output.unique_slugs, 1);
        assert!(output.lock_exists);
        assert_eq!(output.lock_covered_slugs, 1);
        assert!(output.uncached_slugs.is_empty());
    }
}
