//! Cache headers and cache breakpoint management.
//!
//! Implements Anthropic prompt-caching markers (`cache_control: {"type":
//! "ephemeral"}`) for system prompts, tool definitions, and messages.
//!
//! Based on upstream Claude Code's `addCacheBreakpoints`, `getCacheControl`,
//! and `should1hCacheTTL` in `services/api/claude.ts`.

use serde_json::{Value, json};

// ---------------------------------------------------------------------------
// Cache control
// ---------------------------------------------------------------------------

/// Cache scope for prompt caching.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheScope {
    /// Request-local cache (default).
    Local,
    /// Globally shared cache across requests.
    Global,
}

/// Build a `cache_control` JSON object for the API request.
///
/// Returns `{"type": "ephemeral"}` with optional `ttl` and `scope` fields.
#[must_use]
pub fn get_cache_control(scope: Option<CacheScope>, use_1h_ttl: bool) -> Value {
    let mut control = json!({
        "type": "ephemeral"
    });

    if use_1h_ttl {
        control["ttl"] = json!("1h");
    }

    if scope == Some(CacheScope::Global) {
        control["scope"] = json!("global");
    }

    control
}

// ---------------------------------------------------------------------------
// 1-hour TTL eligibility
// ---------------------------------------------------------------------------

/// Determine whether the 1-hour cache TTL should be used.
///
/// The 1-hour TTL is applied when:
/// 1. The `ENABLE_PROMPT_CACHING_1H` environment variable is set to a truthy
///    value, OR
/// 2. The session has accumulated enough context to benefit (estimated tokens
///    > 10 000 and more than 2 turns).
///
/// Auto-enabling avoids the overhead of 1h cache breakpoints for new or very
/// short sessions where the KV prefix would be too small to reuse.
#[must_use]
pub fn should_1h_cache_ttl() -> bool {
    std::env::var("ENABLE_PROMPT_CACHING_1H")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// Determine whether the 1-hour cache TTL should be used based on session
/// characteristics.
///
/// This is an enhanced version of [`should_1h_cache_ttl`] that also considers
/// the estimated prompt token count and session turn depth.  It auto-enables
/// 1h TTL for sessions that are large enough to benefit from longer-lived
/// cache entries.
#[must_use]
pub fn should_use_1h_cache(estimated_tokens: u64, session_turns: u32) -> bool {
    if std::env::var("ENABLE_PROMPT_CACHING_1H")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
    {
        return true;
    }
    // Auto-enable for sessions with enough context to benefit.
    estimated_tokens > 10_000 && session_turns > 2
}

// ---------------------------------------------------------------------------
// Prompt caching toggle
// ---------------------------------------------------------------------------

/// Check whether prompt caching is enabled for the given model.
///
/// Respects both `CLAUDE_CODE_DISABLE_PROMPT_CACHING` and `DISABLE_PROMPT_CACHING`
/// environment variables (the prefixed form takes precedence).
#[must_use]
pub fn is_prompt_caching_enabled(model: &str) -> bool {
    // Check CLAUDE_CODE_-prefixed variant first (TS parity)
    if std::env::var("CLAUDE_CODE_DISABLE_PROMPT_CACHING")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
    {
        return false;
    }

    if std::env::var("DISABLE_PROMPT_CACHING")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
    {
        return false;
    }

    // Small/fast models may have caching disabled.
    let model_lower = model.to_ascii_lowercase();
    if std::env::var("DISABLE_PROMPT_CACHING_HAIKU")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
        && (model_lower.contains("haiku") || model_lower.contains("flash"))
    {
        return false;
    }

    true
}

// ---------------------------------------------------------------------------
// add_cache_breakpoints
// ---------------------------------------------------------------------------

/// Add cache breakpoints to the API request body.
///
/// Marks strategic positions with `cache_control: {"type": "ephemeral"}`:
///
/// 1. **System prompt** — last block gets a cache marker.
/// 2. **Tools** — last tool definition gets a cache marker.
/// 3. **Most recent user message** — last content block gets a cache marker.
///
/// This follows the Anthropic best practice of placing cache markers at
/// natural prefix boundaries so the server can reuse cached KV entries.
pub fn add_cache_breakpoints(body: &mut Value, use_1h_ttl: bool) {
    let cache_control = get_cache_control(None, use_1h_ttl);

    // 1. System prompt — ensure array format, mark last block.
    if let Some(system) = body.get_mut("system") {
        if system.is_string() {
            let text = system.as_str().unwrap_or("").to_owned();
            *system = json!([{
                "type": "text",
                "text": text,
                "cache_control": cache_control.clone(),
            }]);
        } else if let Some(system_arr) = system.as_array_mut()
            && let Some(last) = system_arr.last_mut()
        {
            last["cache_control"] = cache_control.clone();
        }
    }

    // 2. Tools — sort by name for deterministic cache keys, mark last.
    if let Some(tools) = body.get_mut("tools")
        && let Some(tools_arr) = tools.as_array_mut()
    {
        // Sort tools by name for deterministic ordering.
        tools_arr.sort_by(|a, b| {
            let name_a = a.get("name").and_then(Value::as_str).unwrap_or("");
            let name_b = b.get("name").and_then(Value::as_str).unwrap_or("");
            name_a.cmp(name_b)
        });
        if let Some(last_tool) = tools_arr.last_mut() {
            last_tool["cache_control"] = cache_control.clone();
        }
    }

    // 3. Most recent user message — mark last content block.
    if let Some(messages) = body.get_mut("messages")
        && let Some(msg_arr) = messages.as_array_mut()
    {
        for msg in msg_arr.iter_mut().rev() {
            if msg.get("role").and_then(Value::as_str) == Some("user") {
                if let Some(content) = msg.get_mut("content") {
                    if content.is_string() {
                        let text = content.as_str().unwrap_or("").to_owned();
                        *content = json!([{
                            "type": "text",
                            "text": text,
                            "cache_control": cache_control.clone(),
                        }]);
                    } else if let Some(content_arr) = content.as_array_mut()
                        && let Some(last_block) = content_arr.last_mut()
                    {
                        last_block["cache_control"] = cache_control.clone();
                    }
                }
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_cache_control_basic() {
        let cc = get_cache_control(None, false);
        assert_eq!(cc["type"], "ephemeral");
        assert!(cc.get("ttl").is_none());
        assert!(cc.get("scope").is_none());
    }

    #[test]
    fn get_cache_control_with_1h_ttl() {
        let cc = get_cache_control(None, true);
        assert_eq!(cc["type"], "ephemeral");
        assert_eq!(cc["ttl"], "1h");
    }

    #[test]
    fn get_cache_control_with_global_scope() {
        let cc = get_cache_control(Some(CacheScope::Global), false);
        assert_eq!(cc["scope"], "global");
    }

    #[test]
    fn add_cache_breakpoints_marks_system_string() {
        let mut body = json!({
            "system": "You are a helpful assistant.",
            "messages": [],
            "tools": [],
        });
        add_cache_breakpoints(&mut body, false);

        let system = body
            .get("system")
            .and_then(Value::as_array)
            .expect("system should be array");
        assert_eq!(system.len(), 1);
        assert_eq!(system[0]["cache_control"]["type"], "ephemeral");
    }

    #[test]
    fn add_cache_breakpoints_marks_system_array() {
        let mut body = json!({
            "system": [
                {"type": "text", "text": "block 1"},
                {"type": "text", "text": "block 2"},
            ],
            "messages": [],
            "tools": [],
        });
        add_cache_breakpoints(&mut body, false);

        let system = body
            .get("system")
            .and_then(Value::as_array)
            .expect("system should be array");
        // Only the last block should have cache_control.
        assert!(system[0].get("cache_control").is_none());
        assert_eq!(system[1]["cache_control"]["type"], "ephemeral");
    }

    #[test]
    fn add_cache_breakpoints_marks_tools() {
        let mut body = json!({
            "system": [],
            "messages": [],
            "tools": [
                {"name": "b_tool", "description": "second"},
                {"name": "a_tool", "description": "first"},
            ],
        });
        add_cache_breakpoints(&mut body, false);

        let tools = body
            .get("tools")
            .and_then(Value::as_array)
            .expect("tools should be array");
        // Tools should be sorted by name.
        assert_eq!(tools[0]["name"], "a_tool");
        assert_eq!(tools[1]["name"], "b_tool");
        // Only the last tool should have cache_control.
        assert!(tools[0].get("cache_control").is_none());
        assert_eq!(tools[1]["cache_control"]["type"], "ephemeral");
    }

    #[test]
    fn add_cache_breakpoints_marks_last_user_message() {
        let mut body = json!({
            "system": [],
            "messages": [
                {"role": "user", "content": "hello"},
                {"role": "assistant", "content": "hi"},
                {"role": "user", "content": "how are you?"},
            ],
            "tools": [],
        });
        add_cache_breakpoints(&mut body, false);

        let messages = body
            .get("messages")
            .and_then(Value::as_array)
            .expect("messages should be array");
        // First user message should NOT have cache_control.
        assert!(
            messages[0]
                .get("content")
                .and_then(Value::as_array)
                .is_none()
                || messages[0]
                    .get("content")
                    .and_then(|c| c.as_array().and_then(|a| a.first()))
                    .and_then(|b| b.get("cache_control"))
                    .is_none()
        );
        // Last user message should have cache_control.
        let last_user = &messages[2];
        let content = last_user
            .get("content")
            .and_then(Value::as_array)
            .expect("content should be array");
        assert_eq!(content[0]["cache_control"]["type"], "ephemeral");
    }
}
