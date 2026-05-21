use serde_json::Value;

use crate::classifier::{extract_prompt_description, shell_prompt_rule_matches_command};
use crate::filesystem::{normalize_for_comparison, resolve_candidate_path};
use crate::{PermissionClass, PermissionRequest, classify_tool};

#[must_use]
pub fn rule_matches_request(pattern: &str, request: &PermissionRequest) -> bool {
    let (name_part, input_pattern) = split_pattern(pattern);
    if !name_matches(name_part, &request.tool_name) {
        return false;
    }
    let Some(input_pattern) = input_pattern else {
        return true;
    };

    if let Some(command) = extract_shell_command(&request.tool_input) {
        return shell_input_pattern_matches_command(input_pattern, command);
    }

    if matches!(
        classify_tool(&request.tool_name),
        PermissionClass::Read | PermissionClass::Edit
    ) {
        return file_rule_matches_request(input_pattern, request);
    }

    wildcard_match_values(input_pattern, &request.tool_input)
}

#[must_use]
pub fn rule_action_matches_request_action(
    pattern: &str,
    request: &PermissionRequest,
    action: crate::RuleAction,
) -> bool {
    let (name_part, input_pattern) = split_pattern(pattern);
    if !name_matches(name_part, &request.tool_name) {
        return false;
    }
    let Some(input_pattern) = input_pattern else {
        return true;
    };

    if let Some(command) = extract_shell_command(&request.tool_input) {
        return shell_input_pattern_matches_command(input_pattern, command);
    }

    if matches!(
        classify_tool(&request.tool_name),
        PermissionClass::Read | PermissionClass::Edit
    ) {
        return file_rule_matches_request_for_action(input_pattern, request, action);
    }

    wildcard_match_values(input_pattern, &request.tool_input)
}

fn split_pattern(pattern: &str) -> (&str, Option<&str>) {
    if let Some(open) = pattern.find('(') {
        let close = pattern.rfind(')').unwrap_or(pattern.len());
        let name = &pattern[..open];
        let sub = &pattern[open + 1..close];
        (name.trim(), Some(sub.trim()))
    } else {
        (pattern.trim(), None)
    }
}

fn name_matches(pattern: &str, tool_name: &str) -> bool {
    let normalized = pattern.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "bash" => {
            tool_name.eq_ignore_ascii_case("bash") || tool_name.eq_ignore_ascii_case("bash_command")
        }
        "powershell" => tool_name.eq_ignore_ascii_case("powershell"),
        "read" => classify_tool(tool_name) == PermissionClass::Read,
        "edit" => classify_tool(tool_name) == PermissionClass::Edit,
        "command" => classify_tool(tool_name) == PermissionClass::Bash,
        _ if pattern.contains('*') || pattern.contains('?') => {
            wildcard_match(&normalized, &tool_name.to_ascii_lowercase())
        }
        _ => pattern.eq_ignore_ascii_case(tool_name),
    }
}

fn extract_shell_command(input: &Value) -> Option<&str> {
    input.get("command").and_then(Value::as_str)
}

fn shell_input_pattern_matches_command(input_pattern: &str, command: &str) -> bool {
    if let Some(description) = extract_prompt_description(input_pattern) {
        return shell_prompt_rule_matches_command(command, description);
    }
    wildcard_match(input_pattern, command)
}

fn file_rule_matches_request(pattern: &str, request: &PermissionRequest) -> bool {
    file_rule_matches_request_for_action(pattern, request, crate::RuleAction::Allow)
}

fn file_rule_matches_request_for_action(
    pattern: &str,
    request: &PermissionRequest,
    action: crate::RuleAction,
) -> bool {
    let candidates = file_path_candidates_for_request(request);
    if candidates.is_empty() {
        return false;
    }
    let working_directory = request
        .working_directory
        .as_deref()
        .map(std::path::Path::new)
        .unwrap_or_else(|| std::path::Path::new("."));
    let normalized_pattern = normalize_permission_pattern(pattern, working_directory, action);

    candidates
        .iter()
        .any(|candidate| file_pattern_match(&normalized_pattern, candidate))
}

fn file_path_candidates_for_request(request: &PermissionRequest) -> Vec<String> {
    let mut candidates = Vec::new();
    let working_directory = request
        .working_directory
        .as_deref()
        .map(std::path::Path::new)
        .unwrap_or_else(|| std::path::Path::new("."));

    for raw in [
        request.blocked_path.as_deref(),
        request.tool_input.get("path").and_then(Value::as_str),
        request.tool_input.get("directory").and_then(Value::as_str),
    ]
    .into_iter()
    .flatten()
    {
        push_unique_candidate(&mut candidates, normalize_filesystem_value(raw));

        let absolute_path = resolve_candidate_path(working_directory, Some(raw));
        let absolute = normalize_for_comparison(&absolute_path);
        push_unique_candidate(&mut candidates, absolute);

        if let Ok(relative) = absolute_path.strip_prefix(working_directory) {
            push_unique_candidate(
                &mut candidates,
                normalize_filesystem_value(&relative.to_string_lossy()),
            );
        }
    }

    candidates
}

fn normalize_permission_pattern(
    pattern: &str,
    working_directory: &std::path::Path,
    action: crate::RuleAction,
) -> String {
    let mut rendered = pattern.trim().replace('\\', "/");
    if rendered.starts_with("~/") {
        if let Some(home) = home_dir() {
            rendered = format!("{}/{}", home.replace('\\', "/"), &rendered[2..]);
        }
    } else if let Some(rest) = rendered.strip_prefix("//") {
        if rest.len() >= 2 && rest.as_bytes()[1] == b'/' && rest.as_bytes()[0].is_ascii_alphabetic()
        {
            let drive = char::from(rest.as_bytes()[0]).to_ascii_lowercase();
            rendered = format!("{drive}:{}", &rest[1..]);
        } else {
            rendered = format!("/{rest}");
        }
    } else if rendered.starts_with('/') {
        rendered = format!(
            "{}{}",
            normalize_for_comparison(working_directory),
            rendered
        );
    } else if rendered.starts_with("./") {
        rendered = format!(
            "{}/{}",
            normalize_for_comparison(working_directory),
            rendered.trim_start_matches("./")
        );
    }

    rendered = rendered.to_ascii_lowercase();

    if action == crate::RuleAction::Allow && (rendered.ends_with("/**") || rendered.ends_with("/"))
    {
        rendered
    } else {
        rendered.trim_end_matches('/').to_owned()
    }
}

fn file_pattern_match(pattern: &str, path: &str) -> bool {
    let normalized_path = path.trim_end_matches('/');
    if let Some(root) = pattern.strip_suffix("/**") {
        let root = root.trim_end_matches('/');
        return normalized_path == root || normalized_path.starts_with(&format!("{root}/"));
    }
    if pattern.ends_with('/') {
        let root = pattern.trim_end_matches('/');
        return normalized_path == root || normalized_path.starts_with(&format!("{root}/"));
    }
    wildcard_match(pattern, normalized_path)
}

fn normalize_filesystem_value(value: &str) -> String {
    value
        .replace('\\', "/")
        .to_ascii_lowercase()
        .trim_end_matches('/')
        .to_owned()
}

fn push_unique_candidate(candidates: &mut Vec<String>, candidate: String) {
    if !candidate.is_empty() && !candidates.iter().any(|existing| existing == &candidate) {
        candidates.push(candidate);
    }
}

fn home_dir() -> Option<String> {
    std::env::var("HOME")
        .ok()
        .or_else(|| std::env::var("USERPROFILE").ok())
}

fn wildcard_match_values(pattern: &str, value: &Value) -> bool {
    match value {
        Value::String(s) => wildcard_match(pattern, s),
        Value::Array(values) => values
            .iter()
            .any(|value| wildcard_match_values(pattern, value)),
        Value::Object(object) => object
            .values()
            .any(|value| wildcard_match_values(pattern, value)),
        _ => false,
    }
}

#[must_use]
pub fn wildcard_match(pattern: &str, text: &str) -> bool {
    let pattern = pattern.as_bytes();
    let text = text.as_bytes();
    let mut dp = vec![vec![false; text.len() + 1]; pattern.len() + 1];
    dp[0][0] = true;

    for i in 1..=pattern.len() {
        if pattern[i - 1] == b'*' {
            dp[i][0] = dp[i - 1][0];
        } else {
            break;
        }
    }

    for i in 1..=pattern.len() {
        for j in 1..=text.len() {
            if pattern[i - 1] == b'*' {
                dp[i][j] = dp[i - 1][j] || dp[i][j - 1];
            } else if pattern[i - 1] == b'?' || pattern[i - 1] == text[j - 1] {
                dp[i][j] = dp[i - 1][j - 1];
            }
        }
    }

    dp[pattern.len()][text.len()]
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::PermissionRequest;

    use super::rule_matches_request;

    fn request(tool_name: &str, input: serde_json::Value) -> PermissionRequest {
        PermissionRequest {
            tool_name: tool_name.to_owned(),
            permission_class: None,
            tool_input: input,
            working_directory: None,
            tool_use_id: None,
            title: None,
            description: None,
            blocked_path: None,
            permission_suggestions: Vec::new(),
        }
    }

    #[test]
    fn read_class_rule_matches_read_tools() {
        assert!(rule_matches_request(
            "Read",
            &request("read_file", json!({"path":"a"}))
        ));
    }

    #[test]
    fn bash_alias_matches_shell_command_content() {
        assert!(rule_matches_request(
            "Bash(git *)",
            &request("bash_command", json!({"command":"git status"}))
        ));
        assert!(!rule_matches_request(
            "Bash(git *)",
            &request("bash_command", json!({"command":"cargo test"}))
        ));
    }

    #[test]
    fn prompt_rules_match_semantic_bash_intents() {
        assert!(rule_matches_request(
            "Bash(prompt: run tests)",
            &request("bash_command", json!({"command":"cargo test --workspace"}))
        ));
        assert!(!rule_matches_request(
            "Bash(prompt: run tests)",
            &request("bash_command", json!({"command":"cargo build"}))
        ));
    }

    #[test]
    fn file_rules_match_only_path_like_inputs() {
        assert!(!rule_matches_request(
            "Read(src/**)",
            &request("read_file", json!({"pattern":"src/main.rs"}))
        ));
        assert!(rule_matches_request(
            "Read(src/**)",
            &request("read_file", json!({"path":"src/main.rs"}))
        ));
    }

    #[test]
    fn file_rules_match_blocked_path_with_working_directory_root() {
        let mut request = request("read_file", json!({"path":"ignored.txt"}));
        request.working_directory = Some("C:/repo".to_owned());
        request.blocked_path = Some("C:/repo/src/main.rs".to_owned());
        assert!(rule_matches_request("Read(/src/**)", &request));
    }
}
