//! Tab completion for slash commands, tool names, and file paths.

use crate::commands::command_names;

/// Complete a partial slash command input.
pub fn complete_slash_command(partial: &str) -> Vec<String> {
    command_names()
        .iter()
        .filter(|cmd| cmd.starts_with(partial))
        .map(|cmd| cmd.to_string())
        .collect::<Vec<_>>()
}

/// Get tool name completions matching a prefix.
pub fn get_tool_completions(prefix: &str) -> Vec<String> {
    let specs = claude_tools::runtime_builtin_tool_specs();
    specs
        .iter()
        .filter(|s| s.name.starts_with(prefix))
        .map(|s| s.name.clone())
        .collect()
}

/// Get file path completions for a partial path.
#[allow(dead_code)]
pub fn get_file_completions(partial: &str, cwd: &std::path::Path) -> Vec<String> {
    if partial.is_empty() {
        return Vec::new();
    }
    let path = std::path::Path::new(partial);
    let (dir, file_prefix) = if partial.ends_with('/') || partial.ends_with('\\') {
        (cwd.join(partial), "")
    } else if let Some(parent) = path.parent() {
        if parent.as_os_str().is_empty() {
            (
                cwd.to_path_buf(),
                path.file_name().and_then(|n| n.to_str()).unwrap_or(""),
            )
        } else {
            (
                cwd.join(parent),
                path.file_name().and_then(|n| n.to_str()).unwrap_or(""),
            )
        }
    } else {
        (cwd.to_path_buf(), partial)
    };

    let mut results = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with(file_prefix) {
                let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                let suffix = if is_dir { "/" } else { "" };
                // Reconstruct the path relative to cwd.
                let base = if partial.contains('/') || partial.contains('\\') {
                    let parent_str = if let Some(p) = path.parent() {
                        p.to_string_lossy().to_string()
                    } else {
                        String::new()
                    };
                    if parent_str.is_empty() {
                        format!("{name}{suffix}")
                    } else {
                        format!("{parent_str}/{name}{suffix}")
                    }
                } else {
                    format!("{name}{suffix}")
                };
                results.push(base);
            }
        }
    }
    results.sort();
    results.truncate(20); // Limit results.
    results
}

/// Update search results based on the current query.
#[allow(dead_code)]
pub fn update_search_results(history: &[String], query: &str, results: &mut Vec<usize>) {
    results.clear();
    if query.is_empty() {
        return;
    }
    // Search from newest to oldest.
    for (i, entry) in history.iter().enumerate().rev() {
        if entry.contains(query) {
            results.push(i);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_slash_command_matches_prefix() {
        let results = complete_slash_command("/h");
        assert!(results.contains(&"/help".to_owned()));
        assert!(!results.contains(&"/quit".to_owned()));
    }

    #[test]
    fn complete_slash_command_includes_management_surfaces() {
        let results = complete_slash_command("/p");
        assert!(results.contains(&"/plugins".to_owned()));

        let results = complete_slash_command("/m");
        assert!(results.contains(&"/mcp".to_owned()));

        let results = complete_slash_command("/s");
        assert!(results.contains(&"/skills".to_owned()));
    }

    #[test]
    fn complete_slash_command_empty_prefix_returns_all() {
        let results = complete_slash_command("");
        assert!(results.len() >= 10);
    }

    #[test]
    fn complete_slash_command_no_match_returns_empty() {
        let results = complete_slash_command("/xyz");
        assert!(results.is_empty());
    }

    #[test]
    fn get_tool_completions_returns_matching_tools() {
        let results = get_tool_completions("bash");
        assert!(results.iter().any(|t| t == "bash_command"));
    }

    #[test]
    fn get_tool_completions_empty_prefix_returns_all() {
        let results = get_tool_completions("");
        assert!(!results.is_empty());
    }

    #[test]
    fn update_search_results_finds_matches_newest_first() {
        let history = vec![
            "first command".to_owned(),
            "second command".to_owned(),
            "third command".to_owned(),
        ];
        let mut results = Vec::new();
        update_search_results(&history, "command", &mut results);
        assert_eq!(results, vec![2, 1, 0]);
    }

    #[test]
    fn update_search_results_empty_query_clears() {
        let mut results = vec![0, 1];
        update_search_results(&[], "query", &mut results);
        assert!(results.is_empty());
    }

    #[test]
    fn update_search_results_no_match_returns_empty() {
        let history = vec!["hello".to_owned()];
        let mut results = Vec::new();
        update_search_results(&history, "xyz", &mut results);
        assert!(results.is_empty());
    }
}
