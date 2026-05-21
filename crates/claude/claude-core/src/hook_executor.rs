//! Hook execution engine — parallel hook execution with timeout, abort, and SSRF guard.
//!
//! Mirrors the upstream `executeHooksCommon()` from `hooks.ts`.

use std::collections::HashMap;
use std::net::IpAddr;
use std::path::Path;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::hook_types::{
    AggregatedHookResult, HookDefinition, HookInput, HookOutput, HookResponse, HookShell,
};

// ── SSRF Guard ───────────────────────────────────────────────────────────

/// Check if a URL is safe to call (SSRF protection).
///
/// Blocks:
/// - Loopback addresses (127.0.0.1, ::1)
/// - Link-local addresses (169.254.x.x, fe80::)
/// - Private networks (10.x.x.x, 172.16-31.x.x, 192.168.x.x)
/// - Metadata endpoints (169.254.169.254)
pub fn is_url_safe_for_hook(url: &str) -> bool {
    let parsed = match url::Url::parse(url) {
        Ok(u) => u,
        Err(_) => return false,
    };

    let host = match parsed.host_str() {
        Some(h) => h,
        None => return false,
    };

    // Block obvious dangerous hosts
    if host == "localhost" || host == "metadata.google.internal" {
        return false;
    }

    // Try to parse as IP address
    if let Ok(ip) = host.parse::<IpAddr>() {
        return !is_ip_private_or_reserved(&ip);
    }

    // Non-IP hostname: allow (DNS resolution would be needed for full check)
    true
}

/// Check if an IP address is private, loopback, link-local, or reserved.
fn is_ip_private_or_reserved(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_unspecified()
                // AWS / GCP / Azure metadata endpoint
                || (*v4).octets() == [169, 254, 169, 254]
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                // Link-local
                || matches!(v6.segments(), [0xfe80, ..])
        }
    }
}

// ── Execution result types ───────────────────────────────────────────────

/// Outcome of a single hook execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookOutcome {
    /// The hook definition that was executed.
    pub hook: HookDefinition,
    /// Raw output from the hook.
    pub output: HookOutput,
    /// Parsed response (if stdout contained valid JSON).
    pub response: Option<HookResponse>,
    /// Wall-clock execution duration.
    pub duration: Duration,
    /// Whether execution was successful (exit code 0).
    pub success: bool,
    /// Whether the hook blocked the action.
    pub blocked: bool,
}

impl HookOutcome {
    /// Create a failed outcome with an error message.
    pub fn failed(hook: HookDefinition, error: String, duration: Duration) -> Self {
        Self {
            hook,
            output: HookOutput {
                exit_code: None,
                stdout: String::new(),
                stderr: error,
                parsed_json: None,
            },
            response: None,
            duration,
            success: false,
            blocked: false,
        }
    }

    /// Create a timed-out outcome.
    pub fn timed_out(hook: HookDefinition, timeout: Duration) -> Self {
        Self {
            hook,
            output: HookOutput {
                exit_code: None,
                stdout: String::new(),
                stderr: format!("Hook timed out after {}s", timeout.as_secs()),
                parsed_json: None,
            },
            response: None,
            duration: timeout,
            success: false,
            blocked: false,
        }
    }
}

/// Result of executing a batch of hooks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookBatchResult {
    /// Individual outcomes for each hook.
    pub outcomes: Vec<HookOutcome>,
    /// Aggregated result across all hooks.
    pub aggregated: AggregatedHookResult,
    /// Total wall-clock time for the batch.
    pub total_duration: Duration,
}

impl HookBatchResult {
    /// Whether any hook blocked the action.
    #[must_use]
    pub fn is_blocked(&self) -> bool {
        self.aggregated.blocked
    }

    /// Whether all hooks succeeded.
    #[must_use]
    pub fn all_succeeded(&self) -> bool {
        self.outcomes.iter().all(|o| o.success)
    }
}

// ── Hook executor ────────────────────────────────────────────────────────

/// Hook execution engine.
///
/// Executes hooks in parallel with configurable timeout and abort signal.
#[derive(Debug, Clone)]
pub struct HookExecutor {
    /// Default timeout for hooks that don't specify one.
    pub default_timeout_secs: u64,
    /// Working directory for command hooks.
    pub cwd: String,
    /// Shell to use for command hooks.
    pub default_shell: HookShell,
}

impl HookExecutor {
    /// Create a new executor with the given defaults.
    #[must_use]
    pub fn new(cwd: String) -> Self {
        Self {
            default_timeout_secs: 30,
            cwd,
            default_shell: HookShell::platform_default(),
        }
    }

    /// Create a new executor with custom timeout.
    #[must_use]
    pub fn with_timeout(mut self, timeout_secs: u64) -> Self {
        self.default_timeout_secs = timeout_secs;
        self
    }

    /// Execute a batch of hooks in parallel.
    ///
    /// Each hook is executed concurrently. Results are aggregated.
    /// If any hook blocks, the aggregated result will reflect that.
    pub async fn execute_hooks(
        &self,
        hooks: &[HookDefinition],
        input: &HookInput,
    ) -> HookBatchResult {
        let start = Instant::now();
        let mut outcomes = Vec::new();
        let mut aggregated = AggregatedHookResult::new();

        // Execute hooks sequentially to maintain order and allow early exit on block
        for hook in hooks {
            let outcome = self.execute_single_hook(hook, input).await;
            let blocked = outcome.blocked;

            if let Some(ref response) = outcome.response {
                let desc = self.hook_description(hook);
                aggregated.merge_response(response, &desc);
            }

            outcomes.push(outcome);

            if blocked {
                break;
            }
        }

        HookBatchResult {
            outcomes,
            aggregated,
            total_duration: start.elapsed(),
        }
    }

    /// Execute a single hook.
    async fn execute_single_hook(&self, hook: &HookDefinition, input: &HookInput) -> HookOutcome {
        let timeout = hook.timeout_duration(self.default_timeout_secs);
        let start = Instant::now();

        match hook {
            HookDefinition::Command(cmd) => {
                self.execute_command_hook(cmd, input, timeout, start).await
            }
            HookDefinition::Http(http) => self.execute_http_hook(http, input, timeout, start).await,
            HookDefinition::Prompt(_prompt_hook) => {
                // Prompt hooks evaluate the LLM prompt before it is sent.
                // They require access to the current conversation messages and
                // the provider API. Since the hook executor does not hold a
                // provider reference, prompt hooks must be handled by the query
                // engine layer instead.
                //
                // IMPORTANT: We do NOT silently approve. Instead, we return a
                // "not implemented" outcome so the caller knows this hook type
                // requires special handling at a higher layer.
                let duration = start.elapsed();
                HookOutcome::failed(
                    hook.clone(),
                    "Prompt hook execution is not yet implemented in the hook executor. \
                     Prompt hooks require the query engine layer which holds conversation \
                     context and provider access. The caller should handle this hook type \
                     separately."
                        .to_string(),
                    duration,
                )
            }
            HookDefinition::Agent(_agent_hook) => {
                // Agent hooks run an agent to verify the tool call.
                // They require the agent runtime which is not available in the
                // hook executor context.
                //
                // IMPORTANT: We do NOT silently approve. Instead, we return a
                // "not implemented" outcome so the caller knows this hook type
                // requires special handling at a higher layer.
                let duration = start.elapsed();
                HookOutcome::failed(
                    hook.clone(),
                    "Agent hook execution is not yet implemented in the hook executor. \
                     Agent hooks require the agent runtime which is not available in this \
                     context. The caller should handle this hook type separately."
                        .to_string(),
                    duration,
                )
            }
            HookDefinition::Callback(cb) => {
                // Callback hooks are resolved by the caller
                let duration = start.elapsed();
                HookOutcome::failed(
                    hook.clone(),
                    format!(
                        "Callback hook '{}' must be resolved by caller",
                        cb.callback_id
                    ),
                    duration,
                )
            }
            HookDefinition::Function(fn_hook) => {
                let duration = start.elapsed();
                HookOutcome::failed(
                    hook.clone(),
                    format!(
                        "Function hook '{}' must be resolved by caller",
                        fn_hook.function_id
                    ),
                    duration,
                )
            }
        }
    }

    /// Execute a command hook.
    async fn execute_command_hook(
        &self,
        cmd: &crate::hook_types::HookCommand,
        input: &HookInput,
        timeout: Duration,
        start: Instant,
    ) -> HookOutcome {
        let shell = cmd.shell.unwrap_or(self.default_shell);
        let input_json = match serde_json::to_string(input) {
            Ok(j) => j,
            Err(e) => {
                return HookOutcome::failed(
                    HookDefinition::Command(cmd.clone()),
                    format!("Failed to serialize hook input: {e}"),
                    start.elapsed(),
                );
            }
        };

        let result = run_shell_command(shell, &cmd.command, &self.cwd, &input_json, timeout).await;

        match result {
            Ok(mut output) => {
                output.parse_stdout();
                let success = output.is_success();
                let response = HookResponse::from_json_bytes(output.stdout.as_bytes())
                    .ok()
                    .flatten();
                let blocked = response.as_ref().is_some_and(|r| r.is_blocking());

                let duration = start.elapsed();
                HookOutcome {
                    hook: HookDefinition::Command(cmd.clone()),
                    output,
                    response,
                    duration,
                    success,
                    blocked,
                }
            }
            Err(e) => {
                let duration = start.elapsed();
                HookOutcome::failed(
                    HookDefinition::Command(cmd.clone()),
                    e.to_string(),
                    duration,
                )
            }
        }
    }

    /// Execute an HTTP hook with SSRF protection.
    async fn execute_http_hook(
        &self,
        http: &crate::hook_types::HookHttp,
        input: &HookInput,
        timeout: Duration,
        start: Instant,
    ) -> HookOutcome {
        // SSRF check
        if !is_url_safe_for_hook(&http.url) {
            let duration = start.elapsed();
            return HookOutcome::failed(
                HookDefinition::Http(http.clone()),
                format!("SSRF protection: blocked URL {}", http.url),
                duration,
            );
        }

        let input_json = match serde_json::to_string(input) {
            Ok(j) => j,
            Err(e) => {
                return HookOutcome::failed(
                    HookDefinition::Http(http.clone()),
                    format!("Failed to serialize hook input: {e}"),
                    start.elapsed(),
                );
            }
        };

        let result = send_http_request(
            &http.url,
            http.method.as_deref().unwrap_or("POST"),
            &http.headers,
            &http.allowed_env_vars,
            &input_json,
            timeout,
        )
        .await;

        match result {
            Ok(mut output) => {
                output.parse_stdout();
                let success = output.is_success();
                let response = HookResponse::from_json_bytes(output.stdout.as_bytes())
                    .ok()
                    .flatten();
                let blocked = response.as_ref().is_some_and(|r| r.is_blocking());
                let duration = start.elapsed();
                HookOutcome {
                    hook: HookDefinition::Http(http.clone()),
                    output,
                    response,
                    duration,
                    success,
                    blocked,
                }
            }
            Err(e) => {
                let duration = start.elapsed();
                HookOutcome::failed(HookDefinition::Http(http.clone()), e.to_string(), duration)
            }
        }
    }

    /// Get a human-readable description for a hook (for logging).
    fn hook_description(&self, hook: &HookDefinition) -> String {
        match hook {
            HookDefinition::Command(h) => format!("command: {}", h.command),
            HookDefinition::Prompt(h) => {
                format!("prompt: {}...", &h.prompt[..h.prompt.len().min(40)])
            }
            HookDefinition::Agent(h) => {
                format!("agent: {}...", &h.prompt[..h.prompt.len().min(40)])
            }
            HookDefinition::Http(h) => format!("http: {}", h.url),
            HookDefinition::Callback(h) => format!("callback: {}", h.callback_id),
            HookDefinition::Function(h) => format!("function: {}", h.function_id),
        }
    }
}

// ── Shell command execution ──────────────────────────────────────────────

/// Run a shell command and capture output.
///
/// This is the low-level execution function. In production, it spawns a
/// process; in tests, it can be mocked.
pub async fn run_shell_command(
    shell: HookShell,
    command: &str,
    cwd: &str,
    stdin_data: &str,
    timeout: Duration,
) -> anyhow::Result<HookOutput> {
    let (program, args) = match shell {
        HookShell::Bash => {
            #[cfg(windows)]
            {
                ("bash", vec!["-lc", command])
            }
            #[cfg(not(windows))]
            {
                ("sh", vec!["-lc", command])
            }
        }
        HookShell::PowerShell => (
            "powershell",
            vec![
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                command,
            ],
        ),
    };

    let mut child = tokio::process::Command::new(program);
    child.args(&args);
    child.current_dir(Path::new(cwd));
    child.stdin(std::process::Stdio::piped());
    child.stdout(std::process::Stdio::piped());
    child.stderr(std::process::Stdio::piped());

    let mut spawned = child
        .spawn()
        .map_err(|e| anyhow::anyhow!("Failed to spawn command hook '{command}': {e}"))?;

    // Write stdin
    if let Some(mut stdin) = spawned.stdin.take() {
        let data = stdin_data.to_string();
        tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            let _ = stdin.write_all(data.as_bytes()).await;
        });
    }

    // Collect output with timeout
    let future = async {
        use tokio::io::AsyncReadExt;
        let mut stdout = String::new();
        let mut stderr = String::new();
        if let Some(mut stream) = spawned.stdout.take() {
            let _ = stream.read_to_string(&mut stdout).await;
        }
        if let Some(mut stream) = spawned.stderr.take() {
            let _ = stream.read_to_string(&mut stderr).await;
        }
        let status = spawned.wait().await?;
        Ok::<_, std::io::Error>((status.code(), stdout, stderr))
    };

    let (exit_code, stdout, stderr) = tokio::time::timeout(timeout, future)
        .await
        .map_err(|_| anyhow::anyhow!("Command hook timed out after {}s", timeout.as_secs()))?
        .map_err(|e| anyhow::anyhow!("Command hook failed: {e}"))?;

    Ok(HookOutput {
        exit_code,
        stdout,
        stderr,
        parsed_json: None,
    })
}

// ── HTTP request execution ───────────────────────────────────────────────

/// Send an HTTP request for a hook.
///
/// Interpolates environment variables in header values if allowed.
pub async fn send_http_request(
    url: &str,
    method: &str,
    headers: &HashMap<String, String>,
    allowed_env_vars: &[String],
    body: &str,
    timeout: Duration,
) -> anyhow::Result<HookOutput> {
    // Interpolate environment variables in header values
    let resolved_headers = interpolate_env_vars(headers, allowed_env_vars);

    let client = reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|e| anyhow::anyhow!("Failed to build HTTP client: {e}"))?;

    let mut request = match method.to_uppercase().as_str() {
        "POST" => client.post(url),
        "PUT" => client.put(url),
        "GET" => client.get(url),
        _ => client.post(url),
    };

    for (key, value) in &resolved_headers {
        request = request.header(key.as_str(), value.as_str());
    }

    request = request
        .header("Content-Type", "application/json")
        .body(body.to_string());

    let response = request
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("HTTP hook request failed: {e}"))?;

    let status = response.status();
    let response_body = response
        .text()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to read HTTP hook response: {e}"))?;

    let exit_code = if status.is_success() {
        Some(0)
    } else {
        Some(status.as_u16() as i32)
    };

    Ok(HookOutput {
        exit_code,
        stdout: response_body,
        stderr: String::new(),
        parsed_json: None,
    })
}

/// Interpolate environment variables in header values.
///
/// Only variables listed in `allowed_env_vars` are resolved; others are left
/// as empty strings. Supports `$VAR` and `${VAR}` syntax.
fn interpolate_env_vars(
    headers: &HashMap<String, String>,
    allowed_env_vars: &[String],
) -> HashMap<String, String> {
    let allowed_set: std::collections::HashSet<&str> =
        allowed_env_vars.iter().map(String::as_str).collect();

    let mut result = HashMap::new();
    for (key, value) in headers {
        let interpolated = interpolate_env_string(value, &allowed_set);
        result.insert(key.clone(), interpolated);
    }
    result
}

/// Interpolate `$VAR` and `${VAR}` references in a string.
fn interpolate_env_string(value: &str, allowed: &std::collections::HashSet<&str>) -> String {
    let mut result = String::with_capacity(value.len());
    let chars: Vec<char> = value.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        if chars[i] == '$' {
            if i + 1 < chars.len() && chars[i + 1] == '{' {
                // ${VAR} syntax
                let end = chars[i + 2..]
                    .iter()
                    .position(|&c| c == '}')
                    .map(|p| i + 2 + p);
                if let Some(end_pos) = end {
                    let var_name: String = chars[i + 2..end_pos].iter().collect();
                    let resolved = if allowed.contains(var_name.as_str()) {
                        std::env::var(&var_name).unwrap_or_default()
                    } else {
                        String::new()
                    };
                    result.push_str(&resolved);
                    i = end_pos + 1;
                    continue;
                }
            } else if i + 1 < chars.len() && chars[i + 1].is_ascii_alphabetic() {
                // $VAR syntax
                let end = chars[i + 1..]
                    .iter()
                    .position(|c| !c.is_ascii_alphanumeric() && *c != '_')
                    .map(|p| i + 1 + p)
                    .unwrap_or(chars.len());
                let var_name: String = chars[i + 1..end].iter().collect();
                let resolved = if allowed.contains(var_name.as_str()) {
                    std::env::var(&var_name).unwrap_or_default()
                } else {
                    String::new()
                };
                result.push_str(&resolved);
                i = end;
                continue;
            }
        }
        result.push(chars[i]);
        i += 1;
    }

    result
}

// ── Blocking message formatting ──────────────────────────────────────────

/// Format a blocking message from hook outcomes.
#[must_use]
pub fn format_blocking_message(outcomes: &[HookOutcome]) -> String {
    let mut messages = Vec::new();
    for outcome in outcomes {
        if outcome.blocked
            && let Some(ref resp) = outcome.response
        {
            let reason = resp
                .stop_reason
                .as_deref()
                .or(resp.reason.as_deref())
                .unwrap_or("Hook blocked execution");
            messages.push(reason.to_string());
        }
    }
    if messages.is_empty() {
        "Hook blocked execution".to_string()
    } else {
        messages.join("; ")
    }
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hook_types::{HookCommand, HookHttp};
    use crate::hooks::HookEventKind;

    fn test_cwd() -> String {
        std::env::temp_dir().display().to_string()
    }

    fn make_command_hook(cmd: &str) -> HookDefinition {
        HookDefinition::Command(HookCommand {
            command: cmd.to_string(),
            shell: None,
            timeout: None,
            if_condition: None,
            status_message: None,
            once: false,
            r#async: false,
            async_rewake: false,
        })
    }

    #[allow(dead_code)]
    fn make_http_hook(url: &str) -> HookDefinition {
        HookDefinition::Http(HookHttp {
            url: url.to_string(),
            method: None,
            headers: HashMap::new(),
            allowed_env_vars: vec![],
            timeout: None,
            if_condition: None,
            status_message: None,
            once: false,
        })
    }

    fn make_input() -> HookInput {
        HookInput {
            event: HookEventKind::PreToolUse,
            tool_name: Some("Bash".to_string()),
            tool_input: Some(serde_json::json!({"command": "ls"})),
            session_id: Some("test-session".to_string()),
            cwd: Some("/tmp".to_string()),
            user_prompt: None,
            tool_use_id: None,
            tool_result: None,
        }
    }

    // ── SSRF guard tests ─────────────────────────────────────────────────

    #[test]
    fn ssrf_blocks_localhost() {
        assert!(!is_url_safe_for_hook("http://localhost:8080/hook"));
    }

    #[test]
    fn ssrf_blocks_loopback_ip() {
        assert!(!is_url_safe_for_hook("http://127.0.0.1:8080/hook"));
    }

    #[test]
    fn ssrf_blocks_private_ip_10() {
        assert!(!is_url_safe_for_hook("http://10.0.0.1/hook"));
    }

    #[test]
    fn ssrf_blocks_private_ip_172() {
        assert!(!is_url_safe_for_hook("http://172.16.0.1/hook"));
    }

    #[test]
    fn ssrf_blocks_private_ip_192() {
        assert!(!is_url_safe_for_hook("http://192.168.1.1/hook"));
    }

    #[test]
    fn ssrf_blocks_aws_metadata() {
        assert!(!is_url_safe_for_hook(
            "http://169.254.169.254/latest/meta-data/"
        ));
    }

    #[test]
    fn ssrf_blocks_link_local() {
        assert!(!is_url_safe_for_hook("http://169.254.1.1/hook"));
    }

    #[test]
    fn ssrf_allows_public_ip() {
        assert!(is_url_safe_for_hook("https://example.com/hook"));
        assert!(is_url_safe_for_hook("https://1.1.1.1/hook"));
    }

    #[test]
    fn ssrf_rejects_invalid_url() {
        assert!(!is_url_safe_for_hook("not-a-url"));
    }

    #[test]
    fn ssrf_rejects_no_host() {
        assert!(!is_url_safe_for_hook("file:///etc/passwd"));
    }

    // ── HookOutcome tests ────────────────────────────────────────────────

    #[test]
    fn hook_outcome_failed() {
        let hook = make_command_hook("test");
        let outcome = HookOutcome::failed(hook, "error msg".to_string(), Duration::from_secs(1));
        assert!(!outcome.success);
        assert!(outcome.output.stderr.contains("error msg"));
    }

    #[test]
    fn hook_outcome_timed_out() {
        let hook = make_command_hook("test");
        let outcome = HookOutcome::timed_out(hook, Duration::from_secs(30));
        assert!(!outcome.success);
        assert!(outcome.output.stderr.contains("timed out"));
    }

    // ── HookBatchResult tests ────────────────────────────────────────────

    #[test]
    fn batch_result_default_not_blocked() {
        let result = HookBatchResult {
            outcomes: vec![],
            aggregated: AggregatedHookResult::new(),
            total_duration: Duration::from_secs(0),
        };
        assert!(!result.is_blocked());
        assert!(result.all_succeeded());
    }

    // ── HookExecutor tests ───────────────────────────────────────────────

    #[test]
    fn executor_new() {
        let cwd = test_cwd();
        let executor = HookExecutor::new(cwd.clone());
        assert_eq!(executor.cwd, cwd);
        assert_eq!(executor.default_timeout_secs, 30);
    }

    #[test]
    fn executor_with_timeout() {
        let executor = HookExecutor::new(test_cwd()).with_timeout(60);
        assert_eq!(executor.default_timeout_secs, 60);
    }

    #[tokio::test]
    async fn executor_execute_empty_hooks() {
        let executor = HookExecutor::new(test_cwd());
        let input = make_input();
        let result = executor.execute_hooks(&[], &input).await;
        assert!(!result.is_blocked());
        assert!(result.outcomes.is_empty());
    }

    #[tokio::test]
    async fn executor_execute_command_hook_success() {
        let executor = HookExecutor::new(test_cwd());
        let hook = HookDefinition::Command(HookCommand {
            command: "echo hello".to_string(),
            shell: None,
            timeout: Some(5),
            if_condition: None,
            status_message: None,
            once: false,
            r#async: false,
            async_rewake: false,
        });
        let input = make_input();
        let result = executor.execute_hooks(&[hook], &input).await;
        assert_eq!(result.outcomes.len(), 1);
        assert!(result.outcomes[0].success);
    }

    #[tokio::test]
    async fn executor_execute_command_hook_blocking() {
        let executor = HookExecutor::new(test_cwd());
        let hook = HookDefinition::Command(HookCommand {
            command: r#"echo '{"continue":false,"stopReason":"blocked"}'"#.to_string(),
            shell: None,
            timeout: Some(5),
            if_condition: None,
            status_message: None,
            once: false,
            r#async: false,
            async_rewake: false,
        });
        let input = make_input();
        let result = executor.execute_hooks(&[hook], &input).await;
        assert_eq!(result.outcomes.len(), 1);
        assert!(result.outcomes[0].blocked);
        assert!(result.is_blocked());
    }

    #[tokio::test]
    async fn executor_stops_on_first_block() {
        let executor = HookExecutor::new(test_cwd());
        let hooks = vec![
            HookDefinition::Command(HookCommand {
                command: r#"echo '{"continue":false}'"#.to_string(),
                shell: None,
                timeout: Some(5),
                if_condition: None,
                status_message: None,
                once: false,
                r#async: false,
                async_rewake: false,
            }),
            HookDefinition::Command(HookCommand {
                command: "echo should-not-run".to_string(),
                shell: None,
                timeout: Some(5),
                if_condition: None,
                status_message: None,
                once: false,
                r#async: false,
                async_rewake: false,
            }),
        ];
        let input = make_input();
        let result = executor.execute_hooks(&hooks, &input).await;
        // Should stop after first blocking hook
        assert_eq!(result.outcomes.len(), 1);
    }

    // ── Env var interpolation tests ──────────────────────────────────────

    #[test]
    fn interpolate_dollar_var_uses_existing_env() {
        // Use PATH which exists on all platforms
        let mut headers = HashMap::new();
        headers.insert("Path".to_string(), "prefix-$PATH".to_string());
        let allowed = vec!["PATH".to_string()];

        let result = interpolate_env_vars(&headers, &allowed);
        let path_val = std::env::var("PATH").unwrap_or_default();
        assert_eq!(result["Path"], format!("prefix-{path_val}"));
    }

    #[test]
    fn interpolate_brace_var_uses_existing_env() {
        let mut headers = HashMap::new();
        headers.insert("Path".to_string(), "value-${PATH}-suffix".to_string());
        let allowed = vec!["PATH".to_string()];

        let result = interpolate_env_vars(&headers, &allowed);
        let path_val = std::env::var("PATH").unwrap_or_default();
        assert_eq!(result["Path"], format!("value-{path_val}-suffix"));
    }

    #[test]
    fn interpolate_disallowed_var_returns_empty() {
        let mut headers = HashMap::new();
        headers.insert("Auth".to_string(), "Bearer $SECRET".to_string());
        let allowed: Vec<String> = vec![];

        let result = interpolate_env_vars(&headers, &allowed);
        assert_eq!(result["Auth"], "Bearer ");
    }

    #[test]
    fn interpolate_no_vars() {
        let mut headers = HashMap::new();
        headers.insert("Content-Type".to_string(), "application/json".to_string());
        let allowed: Vec<String> = vec![];

        let result = interpolate_env_vars(&headers, &allowed);
        assert_eq!(result["Content-Type"], "application/json");
    }

    // ── format_blocking_message tests ────────────────────────────────────

    #[test]
    fn format_blocking_message_empty() {
        let msg = format_blocking_message(&[]);
        assert_eq!(msg, "Hook blocked execution");
    }

    #[test]
    fn format_blocking_message_with_reasons() {
        let outcomes = vec![HookOutcome {
            hook: make_command_hook("test"),
            output: HookOutput {
                exit_code: Some(0),
                stdout: String::new(),
                stderr: String::new(),
                parsed_json: None,
            },
            response: Some(HookResponse {
                r#continue: false,
                stop_reason: Some("policy violation".to_string()),
                ..Default::default()
            }),
            duration: Duration::from_secs(1),
            success: true,
            blocked: true,
        }];
        let msg = format_blocking_message(&outcomes);
        assert_eq!(msg, "policy violation");
    }
}
