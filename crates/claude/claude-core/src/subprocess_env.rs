//! Subprocess environment variable stripping.
//!
//! Mirrors `utils/subprocessEnv.ts` from the upstream TS codebase.
//! Strips dangerous env vars (API keys, secrets) from child processes
//! to prevent accidental leakage through tool execution.

/// Environment variables that must be stripped from child processes.
///
/// Mirrors the list from `subprocessEnv.ts`.
pub const STRIPPED_ENV_VARS: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "CLAUDE_API_KEY",
    "OPENAI_API_KEY",
    "OPENAI_AUTH_TOKEN",
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
    "GOOGLE_API_KEY",
    "GOOGLE_APPLICATION_CREDENTIALS",
    "AZURE_API_KEY",
    "AZURE_CLIENT_SECRET",
    "GEMINI_API_KEY",
    "VERTEX_API_KEY",
    "VERTEX_PROJECT",
    "BEDROCK_AWS_ACCESS_KEY_ID",
    "BEDROCK_AWS_SECRET_ACCESS_KEY",
];

/// Environment variable that gates subprocess env stripping.
/// When set to any truthy value, child processes will have secrets stripped.
pub const SUBPROCESS_ENV_SCRUB_VAR: &str = "CLAUDE_CODE_SUBPROCESS_ENV_SCRUB";

/// Check if subprocess env scrubbing is enabled.
pub fn is_subprocess_env_scrub_enabled() -> bool {
    std::env::var(SUBPROCESS_ENV_SCRUB_VAR)
        .map(|v| !v.is_empty() && v != "0" && v.to_lowercase() != "false")
        .unwrap_or(false)
}

/// Build a clean environment for a child process.
///
/// If scrubbing is enabled, strips dangerous variables. Otherwise returns
/// the full current environment.
pub fn build_subprocess_env() -> Vec<(String, String)> {
    let current_env: Vec<(String, String)> = std::env::vars().collect();

    if !is_subprocess_env_scrub_enabled() {
        return current_env;
    }

    let stripped_set: std::collections::HashSet<String> =
        STRIPPED_ENV_VARS.iter().map(|s| s.to_string()).collect();

    current_env
        .into_iter()
        .filter(|(key, _)| !stripped_set.contains(key))
        .collect()
}

/// Strip dangerous env vars from an existing environment map.
pub fn strip_env_vars(env: &mut Vec<(String, String)>) {
    if !is_subprocess_env_scrub_enabled() {
        return;
    }

    let stripped_set: std::collections::HashSet<String> =
        STRIPPED_ENV_VARS.iter().map(|s| s.to_string()).collect();

    env.retain(|(key, _)| !stripped_set.contains(key));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stripped_env_vars_contains_anthropic_key() {
        assert!(STRIPPED_ENV_VARS.contains(&"ANTHROPIC_API_KEY"));
    }

    #[test]
    fn stripped_env_vars_contains_aws_secret() {
        assert!(STRIPPED_ENV_VARS.contains(&"AWS_SECRET_ACCESS_KEY"));
    }

    #[test]
    fn build_subprocess_env_returns_env_when_disabled() {
        // By default, scrubbing is disabled
        let env = build_subprocess_env();
        // Should return the full environment
        assert!(!env.is_empty());
    }

    #[test]
    fn strip_removes_matching_keys() {
        let mut env = vec![
            ("ANTHROPIC_API_KEY".to_string(), "sk-test".to_string()),
            ("PATH".to_string(), "/usr/bin".to_string()),
            ("HOME".to_string(), "/home/user".to_string()),
        ];

        // Temporarily enable scrubbing
        // SAFETY: test-only mutation of a process-local env var
        #[allow(unsafe_code)]
        unsafe {
            std::env::set_var(SUBPROCESS_ENV_SCRUB_VAR, "1");
        }
        strip_env_vars(&mut env);
        #[allow(unsafe_code)]
        unsafe {
            std::env::remove_var(SUBPROCESS_ENV_SCRUB_VAR);
        }

        assert!(!env.iter().any(|(k, _)| k == "ANTHROPIC_API_KEY"));
        assert!(env.iter().any(|(k, _)| k == "PATH"));
        assert!(env.iter().any(|(k, _)| k == "HOME"));
    }
}
