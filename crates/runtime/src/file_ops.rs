use std::path::Path;

pub fn read_file(path: &Path) -> anyhow::Result<String> {
    if !path.exists() {
        anyhow::bail!("File not found: {}", path.display());
    }
    if path.is_dir() {
        let entries = std::fs::read_dir(path)?;
        let mut lines = Vec::new();
        for entry in entries {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            if entry.path().is_dir() {
                lines.push(format!("{}/", name));
            } else {
                lines.push(name);
            }
        }
        return Ok(lines.join("\n"));
    }
    Ok(std::fs::read_to_string(path)?)
}

pub fn read_file_lines(path: &Path, offset: u32, limit: u32) -> anyhow::Result<String> {
    let content = read_file(path)?;
    let lines: Vec<&str> = content.lines().collect();
    let start = offset.saturating_sub(1) as usize;
    let end = (start + limit as usize).min(lines.len());
    Ok(lines[start..end].join("\n"))
}

pub fn write_file(path: &Path, content: &str) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(std::fs::write(path, content)?)
}

pub fn edit_file(path: &Path, old_text: &str, new_text: &str) -> anyhow::Result<()> {
    let content = read_file(path)?;
    if let Some(idx) = content.find(old_text) {
        let mut new_content =
            String::with_capacity(content.len() - old_text.len() + new_text.len());
        new_content.push_str(&content[..idx]);
        new_content.push_str(new_text);
        new_content.push_str(&content[idx + old_text.len()..]);
        write_file(path, &new_content)
    } else {
        anyhow::bail!("oldString not found in file content")
    }
}

pub fn edit_file_all(path: &Path, old_text: &str, new_text: &str) -> anyhow::Result<()> {
    let content = read_file(path)?;
    let new_content = content.replace(old_text, new_text);
    if new_content == content {
        anyhow::bail!("oldString not found in file content");
    }
    write_file(path, &new_content)
}

pub fn glob_search(pattern: &str, path: &Path) -> anyhow::Result<Vec<String>> {
    let full_pattern = if pattern.starts_with('/') || pattern.starts_with('\\') {
        pattern.to_string()
    } else {
        format!("{}/{}", path.display(), pattern)
    };

    let entries: Vec<String> = glob::glob(&full_pattern)?
        .filter_map(|entry| entry.ok())
        .map(|p| p.display().to_string())
        .collect();

    Ok(entries)
}

pub fn grep_search(
    pattern: &str,
    path: &Path,
    include: Option<&str>,
) -> anyhow::Result<Vec<(String, u32, String)>> {
    let re = regex::Regex::new(pattern)?;
    let mut results = Vec::new();

    if path.is_file() {
        let content = std::fs::read_to_string(path)?;
        for (line_num, line) in content.lines().enumerate() {
            if re.is_match(line) {
                results.push((
                    path.display().to_string(),
                    line_num as u32 + 1,
                    line.to_string(),
                ));
            }
        }
    } else {
        for entry in walkdir::WalkDir::new(path)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if !entry.file_type().is_file() {
                continue;
            }
            if let Some(include_pattern) = include {
                if let Some(ext) = path.extension() {
                    let ext_str = ext.to_string_lossy();
                    if !ext_str.contains(include_pattern.trim_start_matches('*')) {
                        continue;
                    }
                }
                let file_name = entry.file_name().to_string_lossy();
                if !matches_glob(include_pattern, &file_name) {
                    continue;
                }
            }

            let content = std::fs::read_to_string(entry.path()).unwrap_or_default();
            for (line_num, line) in content.lines().enumerate() {
                if re.is_match(line) {
                    results.push((
                        entry.path().display().to_string(),
                        line_num as u32 + 1,
                        line.to_string(),
                    ));
                }
            }
        }
    }

    Ok(results)
}

fn matches_glob(pattern: &str, name: &str) -> bool {
    if pattern.starts_with("*.") {
        let ext = &pattern[1..];
        name.ends_with(ext)
    } else {
        name.contains(pattern)
    }
}
