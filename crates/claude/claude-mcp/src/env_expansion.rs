//! Environment variable expansion for MCP configuration values.
//!
//! Supports `${VAR}` and `${VAR:-default}` syntax for expanding environment
//! variables in configuration strings.

/// Result of environment variable expansion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpandedEnvResult {
    /// The expanded string with all variables resolved.
    pub expanded: String,
    /// Variables that were referenced but not set in the environment.
    pub missing_vars: Vec<String>,
}

/// Expand environment variables in a string using the system environment.
///
/// Supports two syntaxes:
/// - `${VAR}` — expands to the value of `VAR`, or empty string if not set.
/// - `${VAR:-default}` — expands to the value of `VAR`, or `default` if not set.
///
/// Returns the expanded string and a list of any variables that were not set
/// and had no default value.
pub fn expand_env_vars(value: &str) -> ExpandedEnvResult {
    expand_env_vars_with_lookup(value, env_lookup)
}

/// Concrete lookup function for environment variables.
fn env_lookup(key: &str) -> Option<String> {
    std::env::var(key).ok()
}

/// Expand environment variables in a string using a custom lookup function.
///
/// This is the testable core that accepts a variable lookup closure,
/// making it possible to test without modifying the process environment.
pub fn expand_env_vars_with_lookup<F>(value: &str, lookup: F) -> ExpandedEnvResult
where
    F: Fn(&str) -> Option<String>,
{
    let mut expanded = String::with_capacity(value.len());
    let mut missing_vars = Vec::new();
    let bytes = value.as_bytes();
    let len = bytes.len();
    let mut pos = 0;

    while pos < len {
        if bytes[pos] == b'$' && pos + 1 < len && bytes[pos + 1] == b'{' {
            // Find the closing brace
            let start = pos + 2;
            let mut depth = 1;
            let mut end = start;
            while end < len {
                if bytes[end] == b'{' {
                    depth += 1;
                } else if bytes[end] == b'}' {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                end += 1;
            }

            if end >= len || depth != 0 {
                // Unclosed brace — emit the rest as-is
                expanded.push_str(&value[pos..]);
                break;
            }

            let var_content = &value[start..end];

            // Parse VAR:-default or just VAR
            let (var_name, default) = if let Some(colon_pos) = var_content.find(":-") {
                let name = &var_content[..colon_pos];
                let default_val = &var_content[colon_pos + 2..];
                (name.to_owned(), Some(default_val.to_owned()))
            } else {
                (var_content.to_owned(), None)
            };

            match lookup(&var_name) {
                Some(val) => {
                    expanded.push_str(&val);
                }
                None => {
                    if let Some(default_val) = default {
                        expanded.push_str(&default_val);
                    } else {
                        missing_vars.push(var_name);
                    }
                }
            }

            pos = end + 1; // Skip past closing '}'
        } else {
            expanded.push(bytes[pos] as char);
            pos += 1;
        }
    }

    ExpandedEnvResult {
        expanded,
        missing_vars,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn mock_lookup(vars: &HashMap<String, String>) -> impl Fn(&str) -> Option<String> + '_ {
        move |key: &str| vars.get(key).cloned()
    }

    #[test]
    fn no_expansion_needed() {
        let vars = HashMap::new();
        let result = expand_env_vars_with_lookup("hello world", mock_lookup(&vars));
        assert_eq!(result.expanded, "hello world");
        assert!(result.missing_vars.is_empty());
    }

    #[test]
    fn expands_existing_var() {
        let vars = HashMap::from([("MY_VAR".to_owned(), "expanded_value".to_owned())]);
        let result = expand_env_vars_with_lookup("prefix_${MY_VAR}_suffix", mock_lookup(&vars));
        assert_eq!(result.expanded, "prefix_expanded_value_suffix");
        assert!(result.missing_vars.is_empty());
    }

    #[test]
    fn missing_var_no_default() {
        let vars = HashMap::new();
        let result = expand_env_vars_with_lookup("${MISSING_VAR}", mock_lookup(&vars));
        assert_eq!(result.expanded, "");
        assert!(result.missing_vars.contains(&"MISSING_VAR".to_owned()));
    }

    #[test]
    fn missing_var_with_default() {
        let vars = HashMap::new();
        let result = expand_env_vars_with_lookup("${MISSING_VAR:-fallback}", mock_lookup(&vars));
        assert_eq!(result.expanded, "fallback");
        assert!(result.missing_vars.is_empty());
    }

    #[test]
    fn multiple_vars() {
        let vars = HashMap::from([("VAR_A".to_owned(), "AAA".to_owned())]);
        let result = expand_env_vars_with_lookup("${VAR_A}_${MISSING_B:-BBB}", mock_lookup(&vars));
        assert_eq!(result.expanded, "AAA_BBB");
        assert!(result.missing_vars.is_empty());
    }

    #[test]
    fn empty_default() {
        let vars = HashMap::new();
        let result = expand_env_vars_with_lookup("${MISSING_EMPTY:-}", mock_lookup(&vars));
        assert_eq!(result.expanded, "");
        assert!(result.missing_vars.is_empty());
    }

    #[test]
    fn dollar_without_brace_unchanged() {
        let vars = HashMap::new();
        let result = expand_env_vars_with_lookup("$HOME/path", mock_lookup(&vars));
        assert_eq!(result.expanded, "$HOME/path");
    }

    #[test]
    fn unclosed_brace_emits_as_is() {
        let vars = HashMap::new();
        let result = expand_env_vars_with_lookup("${UNCLOSED", mock_lookup(&vars));
        assert_eq!(result.expanded, "${UNCLOSED");
    }

    #[test]
    fn adjacent_expansions() {
        let vars = HashMap::from([
            ("VAR_X".to_owned(), "X".to_owned()),
            ("VAR_Y".to_owned(), "Y".to_owned()),
        ]);
        let result = expand_env_vars_with_lookup("${VAR_X}${VAR_Y}", mock_lookup(&vars));
        assert_eq!(result.expanded, "XY");
    }
}
