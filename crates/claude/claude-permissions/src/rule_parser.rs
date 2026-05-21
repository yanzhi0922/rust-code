use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::rule::PermissionRuleValue;
use crate::rules::{RuleAction, RuleSource, SourceAwarePermissionRule};

#[derive(Debug, Default, Deserialize)]
struct PermissionDocument {
    #[serde(default)]
    permissions: PermissionLists,
}

#[derive(Debug, Default, Deserialize)]
struct PermissionLists {
    #[serde(default)]
    allow: Vec<String>,
    #[serde(default)]
    ask: Vec<String>,
    #[serde(default)]
    deny: Vec<String>,
}

pub fn discover_permission_rule_files(
    cwd: &Path,
    profile_dir: &Path,
    settings_files: &[PathBuf],
    cli_settings_files: &[PathBuf],
) -> Vec<(PathBuf, RuleSource)> {
    let explicit_cli_paths = cli_settings_files
        .iter()
        .map(|path| normalize_rule_path(cwd, path))
        .collect::<Vec<_>>();
    let mut files = Vec::new();
    files.extend(settings_files.iter().map(|path| {
        (
            path.clone(),
            classify_settings_rule_source(path, cwd, profile_dir, &explicit_cli_paths),
        )
    }));

    for candidate in [
        cwd.join(".remote-code-rust").join("settings.toml"),
        cwd.join(".remote-code-rust").join("settings.json"),
    ] {
        if candidate.exists() && !files.iter().any(|(path, _)| path == &candidate) {
            files.push((candidate, RuleSource::Project));
        }
    }

    for candidate in [
        profile_dir.join("settings.toml"),
        profile_dir.join("settings.json"),
    ] {
        if candidate.exists() && !files.iter().any(|(path, _)| path == &candidate) {
            files.push((candidate, RuleSource::User));
        }
    }
    files
}

fn classify_settings_rule_source(
    path: &Path,
    cwd: &Path,
    profile_dir: &Path,
    explicit_cli_paths: &[PathBuf],
) -> RuleSource {
    if explicit_cli_paths
        .iter()
        .any(|candidate| candidate == &normalize_rule_path(cwd, path))
    {
        return RuleSource::Cli;
    }

    let project_paths = [
        cwd.join(".remote-code").join("settings.json"),
        cwd.join(".remote-code").join("settings.local.json"),
        cwd.join(".remote-code-rust").join("settings.toml"),
        cwd.join(".remote-code-rust").join("settings.json"),
    ];
    if project_paths.iter().any(|candidate| candidate == path) {
        return RuleSource::Project;
    }

    let user_paths = [
        profile_dir.join("settings.toml"),
        profile_dir.join("settings.json"),
        profile_dir
            .join("profiles")
            .join("legacy-import")
            .join("settings.json"),
    ];
    if user_paths.iter().any(|candidate| candidate == path) {
        return RuleSource::User;
    }

    RuleSource::Cli
}

fn normalize_rule_path(cwd: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

pub fn load_permission_rules_from_file(
    path: &Path,
    source: RuleSource,
) -> Result<Vec<SourceAwarePermissionRule>> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read permission settings {}", path.display()))?;
    let parsed = parse_permission_document(path, &raw)?;
    Ok(materialize_rules(parsed.permissions, source))
}

fn parse_permission_document(path: &Path, raw: &str) -> Result<PermissionDocument> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "json" => serde_json::from_str(raw)
            .with_context(|| format!("failed to parse JSON settings {}", path.display())),
        "toml" => toml::from_str(raw)
            .with_context(|| format!("failed to parse TOML settings {}", path.display())),
        _ => toml::from_str(raw)
            .or_else(|_| serde_json::from_str(raw))
            .with_context(|| format!("failed to parse permission settings {}", path.display())),
    }
}

fn materialize_rules(
    permissions: PermissionLists,
    source: RuleSource,
) -> Vec<SourceAwarePermissionRule> {
    let mut rules = Vec::new();
    rules.extend(
        permissions
            .allow
            .into_iter()
            .map(|tool_pattern| SourceAwarePermissionRule {
                tool_pattern,
                action: RuleAction::Allow,
                source,
            }),
    );
    rules.extend(
        permissions
            .ask
            .into_iter()
            .map(|tool_pattern| SourceAwarePermissionRule {
                tool_pattern,
                action: RuleAction::Ask,
                source,
            }),
    );
    rules.extend(
        permissions
            .deny
            .into_iter()
            .map(|tool_pattern| SourceAwarePermissionRule {
                tool_pattern,
                action: RuleAction::Deny,
                source,
            }),
    );
    rules
}

pub fn normalize_legacy_tool_name(name: &str) -> &str {
    match name {
        "Task" => "Agent",
        "KillShell" => "TaskStop",
        "AgentOutputTool" => "TaskOutput",
        "BashOutputTool" => "TaskOutput",
        "Brief" => "Brief",
        _ => name,
    }
}

pub fn find_first_unescaped_char(s: &str, ch: char) -> Option<usize> {
    for (i, c) in s.char_indices() {
        if c == ch {
            let bytes_before = &s[..i];
            let backslash_count = bytes_before
                .chars()
                .rev()
                .take_while(|&c| c == '\\')
                .count();
            if backslash_count % 2 == 0 {
                return Some(i);
            }
        }
    }
    None
}

pub fn find_last_unescaped_char(s: &str, ch: char) -> Option<usize> {
    let mut last: Option<usize> = None;
    for (i, c) in s.char_indices() {
        if c == ch {
            let bytes_before = &s[..i];
            let backslash_count = bytes_before
                .chars()
                .rev()
                .take_while(|&c| c == '\\')
                .count();
            if backslash_count % 2 == 0 {
                last = Some(i);
            }
        }
    }
    last
}

pub fn escape_rule_content(content: &str) -> String {
    let mut out = String::with_capacity(content.len());
    for c in content.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '(' => out.push_str("\\("),
            ')' => out.push_str("\\)"),
            _ => out.push(c),
        }
    }
    out
}

pub fn unescape_rule_content(content: &str) -> String {
    let mut out = String::with_capacity(content.len());
    let mut chars = content.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.peek() {
                Some('\\') | Some('(') | Some(')') => {
                    out.push(chars.next().expect("peeked escape character must exist"));
                }
                _ => out.push(c),
            }
        } else {
            out.push(c);
        }
    }
    out
}

pub fn parse_permission_rule_value(input: &str) -> Option<PermissionRuleValue> {
    let open = find_first_unescaped_char(input, '(');
    let close = find_last_unescaped_char(input, ')');

    match (open, close) {
        (None, _) | (_, None) => {
            if input.is_empty() {
                return None;
            }
            Some(PermissionRuleValue::tool_only(normalize_legacy_tool_name(
                input,
            )))
        }
        (Some(oi), Some(ci)) => {
            if ci <= oi || ci != input.len() - 1 {
                if input.is_empty() {
                    return None;
                }
                return Some(PermissionRuleValue::tool_only(normalize_legacy_tool_name(
                    input,
                )));
            }
            let tool_name = &input[..oi];
            let raw_content = &input[oi + 1..ci];
            if tool_name.is_empty() {
                if input.is_empty() {
                    return None;
                }
                return Some(PermissionRuleValue::tool_only(normalize_legacy_tool_name(
                    input,
                )));
            }
            if raw_content.is_empty() || raw_content == "*" {
                return Some(PermissionRuleValue::tool_only(normalize_legacy_tool_name(
                    tool_name,
                )));
            }
            let rule_content = unescape_rule_content(raw_content);
            Some(PermissionRuleValue::new(
                normalize_legacy_tool_name(tool_name),
                Some(rule_content),
            ))
        }
    }
}

pub fn permission_rule_value_to_string(value: &PermissionRuleValue) -> String {
    match &value.rule_content {
        Some(content) => {
            let escaped = escape_rule_content(content);
            format!("{}({escaped})", value.tool_name)
        }
        None => value.tool_name.clone(),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use crate::rule::PermissionRuleValue;
    use crate::rules::{RuleAction, RuleSource};

    use super::{
        classify_settings_rule_source, discover_permission_rule_files, escape_rule_content,
        find_first_unescaped_char, find_last_unescaped_char, load_permission_rules_from_file,
        normalize_legacy_tool_name, parse_permission_rule_value, permission_rule_value_to_string,
        unescape_rule_content,
    };

    #[test]
    fn loads_json_permission_rules() {
        let tempdir = tempdir().expect("tempdir");
        let path = tempdir.path().join("settings.json");
        fs::write(
            &path,
            r#"{"permissions":{"allow":["Bash(git *)"],"ask":["Edit"],"deny":["Bash(rm -rf *)"]}}"#,
        )
        .expect("write settings");

        let rules = load_permission_rules_from_file(&path, RuleSource::Cli).expect("load rules");
        assert_eq!(rules.len(), 3);
        assert_eq!(rules[0].action, RuleAction::Allow);
        assert_eq!(rules[1].action, RuleAction::Ask);
        assert_eq!(rules[2].action, RuleAction::Deny);
    }

    #[test]
    fn discovers_cli_project_and_user_files() {
        let tempdir = tempdir().expect("tempdir");
        let cwd = tempdir.path().join("workspace");
        let profile = tempdir.path().join("profile");
        fs::create_dir_all(cwd.join(".remote-code-rust")).expect("project dir");
        fs::create_dir_all(&profile).expect("profile dir");
        let cli = tempdir.path().join("cli.toml");
        fs::write(&cli, "").expect("cli");
        fs::write(cwd.join(".remote-code-rust").join("settings.json"), "{}").expect("project");
        fs::write(profile.join("settings.toml"), "").expect("user");

        let files = discover_permission_rule_files(&cwd, &profile, &[cli], &[]);
        assert_eq!(files.len(), 3);
    }

    #[test]
    fn classifies_runtime_settings_paths_by_scope() {
        let tempdir = tempdir().expect("tempdir");
        let cwd = tempdir.path().join("workspace");
        let profile = tempdir.path().join("profile");
        let cli = tempdir.path().join("custom.json");

        assert_eq!(
            classify_settings_rule_source(
                &cwd.join(".remote-code").join("settings.json"),
                &cwd,
                &profile,
                &[]
            ),
            RuleSource::Project
        );
        assert_eq!(
            classify_settings_rule_source(
                &cwd.join(".remote-code").join("settings.local.json"),
                &cwd,
                &profile,
                &[]
            ),
            RuleSource::Project
        );
        assert_eq!(
            classify_settings_rule_source(&profile.join("settings.json"), &cwd, &profile, &[]),
            RuleSource::User
        );
        assert_eq!(
            classify_settings_rule_source(
                &profile
                    .join("profiles")
                    .join("legacy-import")
                    .join("settings.json"),
                &cwd,
                &profile,
                &[]
            ),
            RuleSource::User
        );
        assert_eq!(
            classify_settings_rule_source(&cli, &cwd, &profile, &[]),
            RuleSource::Cli
        );
    }

    #[test]
    fn explicit_cli_settings_paths_keep_cli_priority_even_for_standard_locations() {
        let tempdir = tempdir().expect("tempdir");
        let cwd = tempdir.path().join("workspace");
        let profile = tempdir.path().join("profile");
        let project_settings = cwd.join(".remote-code").join("settings.local.json");

        assert_eq!(
            classify_settings_rule_source(
                &project_settings,
                &cwd,
                &profile,
                std::slice::from_ref(&project_settings)
            ),
            RuleSource::Cli
        );
    }

    #[test]
    fn normalize_legacy_aliases() {
        assert_eq!(normalize_legacy_tool_name("Task"), "Agent");
        assert_eq!(normalize_legacy_tool_name("KillShell"), "TaskStop");
        assert_eq!(normalize_legacy_tool_name("AgentOutputTool"), "TaskOutput");
        assert_eq!(normalize_legacy_tool_name("BashOutputTool"), "TaskOutput");
        assert_eq!(normalize_legacy_tool_name("Bash"), "Bash");
        assert_eq!(normalize_legacy_tool_name("Read"), "Read");
    }

    #[test]
    fn find_first_unescaped_finds_unescaped() {
        assert_eq!(find_first_unescaped_char("abc(def", '('), Some(3));
        assert_eq!(find_first_unescaped_char("abc\\(def", '('), None);
        assert_eq!(find_first_unescaped_char("abc\\\\(def", '('), Some(5));
        assert_eq!(find_first_unescaped_char("abcdef", '('), None);
    }

    #[test]
    fn find_last_unescaped_finds_unescaped() {
        assert_eq!(find_last_unescaped_char("a)b)c)", ')'), Some(5));
        assert_eq!(find_last_unescaped_char("a)b\\)c)", ')'), Some(6));
        assert_eq!(find_last_unescaped_char("a)b\\\\)c", ')'), Some(5));
        assert_eq!(find_last_unescaped_char("abcdef", ')'), None);
    }

    #[test]
    fn escape_rule_content_escapes_special_chars() {
        assert_eq!(escape_rule_content("hello"), "hello");
        assert_eq!(escape_rule_content("a(b)c"), "a\\(b\\)c");
        assert_eq!(escape_rule_content("a\\b"), "a\\\\b");
        assert_eq!(escape_rule_content("a\\(b)"), "a\\\\\\(b\\)");
    }

    #[test]
    fn unescape_rule_content_reverses_escape() {
        assert_eq!(unescape_rule_content("hello"), "hello");
        assert_eq!(unescape_rule_content("a\\(b\\)c"), "a(b)c");
        assert_eq!(unescape_rule_content("a\\\\b"), "a\\b");
    }

    #[test]
    fn escape_unescape_roundtrip() {
        let cases = [
            "simple",
            "with(backslash)parens",
            "a\\b\\c",
            "a(b)c)d\\e(f",
            "",
        ];
        for case in cases {
            assert_eq!(unescape_rule_content(&escape_rule_content(case)), case);
        }
    }

    #[test]
    fn parse_tool_name_only() {
        let v = parse_permission_rule_value("Bash").expect("parse tool-only rule");
        assert_eq!(v.tool_name, "Bash");
        assert!(v.rule_content.is_none());
    }

    #[test]
    fn parse_tool_name_with_content() {
        let v = parse_permission_rule_value("Bash(npm install)").expect("parse rule content");
        assert_eq!(v.tool_name, "Bash");
        assert_eq!(v.rule_content.as_deref(), Some("npm install"));
    }

    #[test]
    fn parse_empty_content_treated_as_tool_only() {
        let v = parse_permission_rule_value("Bash()").expect("parse empty rule content");
        assert_eq!(v.tool_name, "Bash");
        assert!(v.rule_content.is_none());
    }

    #[test]
    fn parse_wildcard_content_treated_as_tool_only() {
        let v = parse_permission_rule_value("Bash(*)").expect("parse wildcard rule content");
        assert_eq!(v.tool_name, "Bash");
        assert!(v.rule_content.is_none());
    }

    #[test]
    fn parse_escaped_parens_in_content() {
        let v = parse_permission_rule_value("Bash(python -c \"print\\(1\\)\")")
            .expect("parse escaped parentheses");
        assert_eq!(v.tool_name, "Bash");
        assert_eq!(v.rule_content.as_deref(), Some("python -c \"print(1)\""));
    }

    #[test]
    fn parse_legacy_tool_name_is_normalized() {
        let v = parse_permission_rule_value("Task").expect("parse legacy task rule");
        assert_eq!(v.tool_name, "Agent");
        let v2 = parse_permission_rule_value("KillShell(foo)").expect("parse legacy shell rule");
        assert_eq!(v2.tool_name, "TaskStop");
        assert_eq!(v2.rule_content.as_deref(), Some("foo"));
    }

    #[test]
    fn parse_empty_input_returns_none() {
        assert!(parse_permission_rule_value("").is_none());
    }

    #[test]
    fn parse_malformed_no_close_paren() {
        let v = parse_permission_rule_value("Bash(nope").expect("parse malformed open paren");
        assert_eq!(v.tool_name, "Bash(nope");
        assert!(v.rule_content.is_none());
    }

    #[test]
    fn parse_malformed_trailing_chars() {
        let v = parse_permission_rule_value("Bash(foo)extra").expect("parse trailing chars");
        assert_eq!(v.tool_name, "Bash(foo)extra");
        assert!(v.rule_content.is_none());
    }

    #[test]
    fn parse_empty_tool_name_treated_as_tool_name() {
        let v = parse_permission_rule_value("(foo)").expect("parse empty tool name");
        assert_eq!(v.tool_name, "(foo)");
        assert!(v.rule_content.is_none());
    }

    #[test]
    fn to_string_tool_only() {
        let v = PermissionRuleValue::tool_only("Bash");
        assert_eq!(permission_rule_value_to_string(&v), "Bash");
    }

    #[test]
    fn to_string_with_content() {
        let v = PermissionRuleValue::new("Bash", Some("npm install".to_string()));
        assert_eq!(permission_rule_value_to_string(&v), "Bash(npm install)");
    }

    #[test]
    fn to_string_escapes_special_chars() {
        let v = PermissionRuleValue::new("Bash", Some("python -c \"print(1)\"".to_string()));
        assert_eq!(
            permission_rule_value_to_string(&v),
            "Bash(python -c \"print\\(1\\)\")"
        );
    }

    #[test]
    fn to_string_escapes_backslash() {
        let v = PermissionRuleValue::new("Bash", Some("echo \\\\n".to_string()));
        assert_eq!(permission_rule_value_to_string(&v), "Bash(echo \\\\\\\\n)");
    }

    #[test]
    fn parse_to_string_roundtrip() {
        let cases = ["Bash", "Bash(npm install)", "Read(src/**)", "Bash(git *)"];
        for case in cases {
            let parsed = parse_permission_rule_value(case).expect("parse roundtrip case");
            let back = permission_rule_value_to_string(&parsed);
            assert_eq!(back, case);
        }
    }

    #[test]
    fn parse_to_string_roundtrip_with_special_chars() {
        let original = "python -c \"print(1)\"";
        let v = PermissionRuleValue::new("Bash", Some(original.to_string()));
        let s = permission_rule_value_to_string(&v);
        let back = parse_permission_rule_value(&s).expect("parse escaped roundtrip");
        assert_eq!(back.rule_content.as_deref(), Some(original));
    }
}
