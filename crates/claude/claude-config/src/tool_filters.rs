use std::collections::BTreeSet;

/// Normalize a tool-filter list by trimming, lowercasing, and de-duplicating.
#[must_use]
pub fn normalize_tool_filters(filters: &[String]) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut normalized = Vec::new();
    for filter in filters {
        let candidate = filter.trim().to_ascii_lowercase();
        if candidate.is_empty() || !seen.insert(candidate.clone()) {
            continue;
        }
        normalized.push(candidate);
    }
    normalized
}

/// Merge tool filters from a lower-priority layer and a higher-priority layer.
#[must_use]
pub fn merge_tool_filters(base: &[String], override_filters: &[String]) -> Vec<String> {
    let mut merged = Vec::new();
    merged.extend(normalize_tool_filters(base));
    merged.extend(normalize_tool_filters(override_filters));
    normalize_tool_filters(&merged)
}

/// Return whether a tool is allowed by the given allow/deny filters.
#[must_use]
pub fn tool_allowed(tool_name: &str, allowed: &[String], disallowed: &[String]) -> bool {
    let normalized_name = tool_name.trim().to_ascii_lowercase();
    let allowed = normalize_tool_filters(allowed);
    let disallowed = normalize_tool_filters(disallowed);

    if !allowed.is_empty() && !allowed.contains(&normalized_name) {
        return false;
    }
    !disallowed.contains(&normalized_name)
}

#[cfg(test)]
mod tests {
    use super::{merge_tool_filters, normalize_tool_filters, tool_allowed};

    #[test]
    fn normalize_filters_deduplicates_and_lowercases() {
        let filters = vec![
            " Bash_Command ".to_owned(),
            "bash_command".to_owned(),
            String::new(),
            "Read_File".to_owned(),
        ];
        assert_eq!(
            normalize_tool_filters(&filters),
            vec!["bash_command".to_owned(), "read_file".to_owned()]
        );
    }

    #[test]
    fn merge_filters_preserves_unique_values() {
        let base = vec!["read_file".to_owned(), "bash_command".to_owned()];
        let override_filters = vec!["BASH_COMMAND".to_owned(), "edit_file".to_owned()];
        assert_eq!(
            merge_tool_filters(&base, &override_filters),
            vec![
                "read_file".to_owned(),
                "bash_command".to_owned(),
                "edit_file".to_owned()
            ]
        );
    }

    #[test]
    fn allow_and_deny_filters_are_applied_together() {
        let allowed = vec!["read_file".to_owned(), "bash_command".to_owned()];
        let disallowed = vec!["bash_command".to_owned()];
        assert!(tool_allowed("read_file", &allowed, &disallowed));
        assert!(!tool_allowed("bash_command", &allowed, &disallowed));
        assert!(!tool_allowed("edit_file", &allowed, &disallowed));
    }
}
