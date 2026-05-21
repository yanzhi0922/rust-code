use claude_config::{RuntimeConfig, SettingSource};

pub fn dispatch(input: &str, config: &RuntimeConfig) {
    let remainder = input
        .trim()
        .strip_prefix("/skills")
        .unwrap_or_default()
        .trim();
    if remainder.is_empty() || remainder == "list" {
        render(config);
        return;
    }

    let mut parts = remainder.split_whitespace();
    match parts.next().unwrap_or_default() {
        "show" => {
            let Some(slug) = parts.next() else {
                println!("Usage: /skills [list|show <slug>|lock|index]");
                return;
            };
            render_skill(config, slug);
        }
        "lock" => render_lock(config),
        "index" => render_index(config),
        other => {
            println!("Unknown /skills subcommand '{other}'.");
            println!("Usage: /skills [list|show <slug>|lock|index]");
        }
    }
}

pub fn render(config: &RuntimeConfig) {
    let (warnings, mut skills) = discover_skills(config);
    if skills.is_empty() {
        println!("Skills: none discovered.");
        for warning in warnings {
            println!("  - {warning}");
        }
        return;
    }

    skills.sort_by(|left, right| {
        left.metadata
            .slug
            .cmp(&right.metadata.slug)
            .then_with(|| left.origin_kind.cmp(right.origin_kind))
            .then_with(|| left.origin_name.cmp(&right.origin_name))
    });

    println!("Skills ({}):", skills.len());
    for warning in &warnings {
        println!("  warning: {warning}");
    }
    for skill in &skills {
        println!(
            "  {}  {}  {}:{}",
            skill.metadata.slug, skill.metadata.title, skill.origin_kind, skill.origin_name
        );
        if let Some(summary) = &skill.metadata.summary {
            println!("    summary: {summary}");
        }
    }
    println!("Tip: /skills show <slug> or /skills lock");
}

fn render_skill(config: &RuntimeConfig, slug: &str) {
    let (warnings, skills) = discover_skills(config);
    let matches = skills
        .into_iter()
        .filter(|skill| skill.metadata.slug == slug)
        .collect::<Vec<_>>();

    if matches.is_empty() {
        println!("No skill named `{slug}` was found.");
        for warning in warnings {
            println!("  warning: {warning}");
        }
        return;
    }
    if matches.len() > 1 {
        println!("Skill `{slug}` is ambiguous across:");
        for skill in matches {
            println!(
                "  - {}:{} ({})",
                skill.origin_kind,
                skill.origin_name,
                skill.metadata.path.display()
            );
        }
        return;
    }

    let skill = &matches[0];
    println!("Skill: {} ({})", skill.metadata.slug, skill.metadata.title);
    println!("  origin: {}:{}", skill.origin_kind, skill.origin_name);
    println!("  path: {}", skill.metadata.path.display());
    println!("  root: {}", skill.metadata.root.display());
    if let Some(summary) = &skill.metadata.summary {
        println!("  summary: {summary}");
    }
    if !skill.metadata.triggers.is_empty() {
        println!("  triggers: {}", skill.metadata.triggers.join(", "));
    }
    if !skill.metadata.tools.is_empty() {
        println!("  tools: {}", skill.metadata.tools.join(", "));
    }
    println!("  references: {}", skill.metadata.references.len());
    println!("  scripts: {}", skill.metadata.scripts.len());
    println!("  assets: {}", skill.metadata.assets.len());
    if !warnings.is_empty() {
        for warning in warnings {
            println!("  warning: {warning}");
        }
    }
}

fn render_lock(config: &RuntimeConfig) {
    let path = config
        .paths
        .profile_dir
        .join(claude_skills::DEFAULT_SKILL_LOCK_FILE);
    if !path.exists() {
        println!("Skill lock: missing ({})", path.display());
        return;
    }
    match claude_skills::load_skill_lock_file(&path) {
        Ok(lock) => {
            println!("Skill lock: {}", path.display());
            println!("  version: {}", lock.version);
            println!("  installed skills: {}", lock.skills.len());
            for (slug, record) in lock.skills.iter().take(20) {
                println!(
                    "  - {}  {}  {}",
                    slug,
                    record.source,
                    record.skill_path.display()
                );
            }
        }
        Err(error) => eprintln!("Failed to load skill lock {}: {error}", path.display()),
    }
}

fn render_index(config: &RuntimeConfig) {
    let (warnings, skills) = discover_skills(config);
    let lock_path = config
        .paths
        .profile_dir
        .join(claude_skills::DEFAULT_SKILL_LOCK_FILE);
    let lock = if lock_path.exists() {
        match claude_skills::load_skill_lock_file(&lock_path) {
            Ok(lock) => Some(lock),
            Err(error) => {
                eprintln!("Failed to load skill lock {}: {error}", lock_path.display());
                None
            }
        }
    } else {
        None
    };

    let mut slug_counts = std::collections::BTreeMap::new();
    let mut origin_counts = std::collections::BTreeMap::new();
    let mut with_references = 0usize;
    let mut with_scripts = 0usize;
    let mut with_assets = 0usize;
    for skill in &skills {
        *slug_counts
            .entry(skill.metadata.slug.clone())
            .or_insert(0usize) += 1;
        *origin_counts
            .entry(skill.origin_kind.to_owned())
            .or_insert(0usize) += 1;
        if !skill.metadata.references.is_empty() {
            with_references += 1;
        }
        if !skill.metadata.scripts.is_empty() {
            with_scripts += 1;
        }
        if !skill.metadata.assets.is_empty() {
            with_assets += 1;
        }
    }

    let duplicates = slug_counts
        .iter()
        .filter_map(|(slug, count)| (*count > 1).then_some(slug.clone()))
        .collect::<Vec<_>>();
    let uncached = skills
        .iter()
        .filter(|skill| {
            lock.as_ref()
                .is_none_or(|lock| !lock.skills.contains_key(&skill.metadata.slug))
        })
        .map(|skill| skill.metadata.slug.clone())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();

    println!(
        "Skill index: total={} unique={} duplicates={}",
        skills.len(),
        slug_counts.len(),
        duplicates.len()
    );
    for warning in &warnings {
        println!("  warning: {warning}");
    }
    if !origin_counts.is_empty() {
        println!(
            "  origins: {}",
            origin_counts
                .iter()
                .map(|(origin, count)| format!("{origin}={count}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    println!(
        "  coverage: refs={} scripts={} assets={}",
        with_references, with_scripts, with_assets
    );
    println!("  lock: {}", lock_path.display());
    println!("  lock exists: {}", lock.is_some());
    println!("  uncached slugs: {}", uncached.len());
    if !duplicates.is_empty() {
        println!("  duplicate slugs: {}", duplicates.join(", "));
    }
    if !uncached.is_empty() {
        println!("  uncached: {}", uncached.join(", "));
    }
}

#[derive(Debug, Clone)]
struct SkillView {
    origin_kind: &'static str,
    origin_name: String,
    metadata: claude_skills::SkillMetadata,
}

fn discover_skills(config: &RuntimeConfig) -> (Vec<String>, Vec<SkillView>) {
    let mut warnings = Vec::new();
    let mut skills = Vec::new();
    let user_sources_enabled = setting_source_enabled(config, SettingSource::User);

    if user_sources_enabled && config.paths.skills_dir.exists() {
        match claude_skills::discover_skills(&config.paths.skills_dir) {
            Ok(discovered) => {
                for skill in discovered {
                    skills.push(SkillView {
                        origin_kind: "profile",
                        origin_name: "profile".to_owned(),
                        metadata: skill.metadata,
                    });
                }
            }
            Err(error) => warnings.push(format!(
                "Failed to discover profile skills in {}: {error}",
                config.paths.skills_dir.display()
            )),
        }
    }

    if user_sources_enabled && config.paths.plugins_dir.exists() {
        match claude_plugins::discover_plugins(&config.paths.plugins_dir) {
            Ok(plugins) => {
                for plugin in plugins {
                    match plugin.discover_bundled_skills() {
                        Ok(discovered) => {
                            for skill in discovered {
                                skills.push(SkillView {
                                    origin_kind: "plugin",
                                    origin_name: plugin.manifest.name.clone(),
                                    metadata: skill.metadata,
                                });
                            }
                        }
                        Err(error) => warnings.push(format!(
                            "Failed to discover plugin skills for {}: {error}",
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

    (warnings, skills)
}

fn setting_source_enabled(config: &RuntimeConfig, source: SettingSource) -> bool {
    config.allowed_setting_sources.contains(&source)
}

pub(crate) fn discovered_skill_count(config: &RuntimeConfig) -> usize {
    discover_skills(config).1.len()
}
