//! Message normalization for the Anthropic API.
//!
//! Mirrors `normalizeMessagesForAPI()` from the TS reference
//! (`src/utils/messages.ts`).  Runs a multi-pass pipeline to ensure the
//! conversation array satisfies the Anthropic API contract:
//!
//! 0.  **filterVirtualMessages** — remove messages marked `is_virtual` / `isVirtual`.
//! 0b. **reorderAttachmentsForAPI** — bubble attachment messages up to stopping points.
//! 0c. **convertSystemLocalCommandToUser** — convert system/local_command messages to user.
//! 1.  **ensureToolResultPairing** — every `tool_use` has a matching `tool_result`.
//! 1b. **sanitizeErrorToolResultContent** — replace image/document in error tool_results.
//! 1c. **stripErrorTriggeredAttachments** — strip document/image blocks from meta user
//!     messages that preceded PDF/image/request-too-large API errors.
//! 2.  **mergeConsecutiveSameRole** — consecutive user or assistant messages merged.
//! 3.  **stripEmptyContentBlocks** — remove text blocks with empty content.
//! 3b. **stripToolReferenceBlocks** — filter tool_reference blocks from tool_result content
//!     (gated by `tool_search_enabled` / `available_tool_names`).
//! 4.  **filterOrphanedThinking / trailingThinking / whitespace** — clean artifacts.
//! 5.  **ensureFirstMessageIsUser** — prepend placeholder if first message isn't user.
//! 6.  **validateContentBlockTypes** — replace unknown block types, normalize tool_use fields.
//! 6b. **validateImagesForAPI** — replace oversized image blocks.
//! 6c. **filterUnavailableToolUseBlocks** — remove tool_use blocks for tools not in the
//!     available set (gated by `available_tool_names`).

use serde_json::{Value, json};
use std::collections::HashSet;

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Configuration for tool-search-aware normalization passes.
#[derive(Default)]
pub struct NormalizeConfig<'a> {
    /// Whether the tool search beta is enabled.  When `false`, all
    /// `tool_reference` blocks are stripped from `tool_result` content.
    /// When `true`, only references to *unavailable* tools are stripped.
    pub tool_search_enabled: bool,
    /// The set of tool names currently available in the session.
    /// When `Some`, tool_reference blocks referencing tools not in this set
    /// are removed, and tool_use blocks for unavailable tools are filtered out.
    pub available_tool_names: Option<&'a HashSet<String>>,
}

/// Normalize a messages array for the Anthropic Messages API.
///
/// `messages` is a mutable slice of JSON objects, each with a `"role"` field
/// (`"user"` or `"assistant"`) and a `"content"` field (string or array of
/// content blocks).
pub fn normalize_messages_for_api(messages: &mut Vec<Value>) {
    normalize_messages_for_api_with_config(messages, NormalizeConfig::default());
}

/// Normalize a messages array for the Anthropic Messages API with tool-search
/// configuration.
pub fn normalize_messages_for_api_with_config(
    messages: &mut Vec<Value>,
    config: NormalizeConfig<'_>,
) {
    // Pass 0: filter virtual messages (is_virtual / isVirtual)
    filter_virtual_messages(messages);
    // Pass 0b: reorder attachments — bubble them up to stopping points
    reorder_attachments_for_api(messages);
    // Pass 0c: convert system/local_command messages to user role
    convert_system_local_command_to_user(messages);
    // Pass 1: tool_use / tool_result pairing
    ensure_tool_result_pairing(messages);
    // Pass 1b: sanitize error tool_result content (remove images/docs from errors)
    sanitize_error_tool_result_content(messages);
    // Pass 1c: strip document/image blocks from meta user messages that triggered API errors
    strip_error_triggered_attachments(messages);
    // Pass 2: merge consecutive same-role messages
    merge_consecutive_same_role(messages);
    // Pass 3: strip empty text content blocks
    strip_empty_content_blocks(messages);
    // Pass 3b: strip tool_reference blocks from tool_result content
    strip_tool_reference_blocks(
        messages,
        config.tool_search_enabled,
        config.available_tool_names,
    );
    // Pass 3c: strip advisor blocks (server_tool_use with name "advisor" and advisor_tool_result)
    // The API rejects these unless the advisor beta header is present.
    strip_advisor_blocks(messages);
    // Pass 4: filter artifacts (orphaned thinking, trailing thinking, whitespace)
    filter_orphaned_thinking_only(messages);
    filter_trailing_thinking_from_last_assistant(messages);
    filter_whitespace_only_assistant_messages(messages);
    // Filters may create consecutive same-role messages — re-merge
    merge_consecutive_same_role(messages);
    ensure_non_empty_assistant_content(messages);
    // Pass 5: ensure first message is user role
    ensure_first_message_is_user(messages);
    // Pass 5b: smoosh system-reminder siblings into the last tool_result
    smoosh_system_reminder_siblings(messages);
    // Pass 6: validate content block types (also normalizes tool_use inputs)
    validate_content_block_types(messages);
    // Pass 6b: validate image sizes for API (replace oversized images)
    validate_images_for_api(messages);
    // Pass 6c: filter tool_use blocks for unavailable tools
    if let Some(tool_names) = config.available_tool_names {
        filter_unavailable_tool_use_blocks(messages, tool_names);
    }
}

// ---------------------------------------------------------------------------
// 1. Tool-use / tool-result pairing
// ---------------------------------------------------------------------------

/// Ensure every `tool_use` block in assistant messages has a corresponding
/// `tool_result` block in the following user message.
///
/// If a `tool_result` is missing, a synthetic one is injected.
/// Mirrors `ensureToolResultPairing()` from the TS reference.
fn ensure_tool_result_pairing(messages: &mut Vec<Value>) {
    let mut insertions: Vec<(usize, Value)> = Vec::new();

    for i in 0..messages.len() {
        let msg = &messages[i];
        if msg["role"].as_str() != Some("assistant") {
            continue;
        }

        let Some(blocks) = msg["content"].as_array() else {
            continue;
        };

        let tool_use_ids: Vec<String> = blocks
            .iter()
            .filter_map(|b| {
                if b["type"].as_str() == Some("tool_use") {
                    b["id"].as_str().map(|s| s.to_owned())
                } else {
                    None
                }
            })
            .collect();

        if tool_use_ids.is_empty() {
            continue;
        }

        // Collect tool_result IDs from the next user message
        let mut covered = std::collections::HashSet::new();
        if let Some(next) = messages.get(i + 1)
            && next["role"].as_str() == Some("user")
            && let Some(content) = next["content"].as_array()
        {
            for block in content {
                if block["type"].as_str() == Some("tool_result")
                    && let Some(id) = block["tool_use_id"].as_str()
                {
                    covered.insert(id.to_owned());
                }
            }
        }

        let missing: Vec<&str> = tool_use_ids
            .iter()
            .filter(|id| !covered.contains(id.as_str()))
            .map(|id| id.as_str())
            .collect();

        if missing.is_empty() {
            continue;
        }

        // Build synthetic tool_result blocks for missing IDs
        let synthetic_blocks: Vec<Value> = missing
            .into_iter()
            .map(|id| {
                json!({
                    "type": "tool_result",
                    "tool_use_id": id,
                    "content": "Tool execution was interrupted. Please try again if needed.",
                    "is_error": true
                })
            })
            .collect();

        // Check if the next message is a user message we can merge into
        if let Some(next) = messages.get(i + 1) {
            if next["role"].as_str() == Some("user") {
                // Merge synthetic blocks into existing user message
                let next_msg = &mut messages[i + 1];
                if let Some(content) = next_msg["content"].as_array_mut() {
                    // Prepend synthetic blocks so they appear before other content
                    let mut new_blocks = synthetic_blocks;
                    new_blocks.append(content);
                    next_msg["content"] = Value::Array(new_blocks);
                } else {
                    // Content is a string — convert to array with synthetic blocks
                    let text = next_msg["content"].take();
                    let mut new_blocks = synthetic_blocks;
                    if let Some(t) = text.as_str()
                        && !t.is_empty()
                    {
                        new_blocks.push(json!({"type": "text", "text": t}));
                    }
                    next_msg["content"] = Value::Array(new_blocks);
                }
            } else {
                // Insert a new user message with synthetic blocks
                insertions.push((
                    i + 1,
                    json!({
                        "role": "user",
                        "content": synthetic_blocks,
                    }),
                ));
            }
        } else {
            // Last message is assistant with orphaned tool_use — append user msg
            insertions.push((
                i + 1,
                json!({
                    "role": "user",
                    "content": synthetic_blocks,
                }),
            ));
        }
    }

    // Apply insertions in reverse order so indices remain valid
    for (idx, msg) in insertions.into_iter().rev() {
        messages.insert(idx, msg);
    }
}

// ---------------------------------------------------------------------------
// 2. Merge consecutive same-role messages
// ---------------------------------------------------------------------------

/// Merge consecutive messages of the same role into a single message.
/// The API requires strict user/assistant alternation.
fn merge_consecutive_same_role(messages: &mut Vec<Value>) {
    if messages.len() <= 1 {
        return;
    }

    let mut result: Vec<Value> = Vec::with_capacity(messages.len());
    result.push(messages[0].take());

    let mut i = 1;
    while i < messages.len() {
        let current = &mut messages[i];
        let prev_role = result.last().and_then(|m| m["role"].as_str());
        let curr_role = current["role"].as_str();

        if prev_role == curr_role && prev_role.is_some() {
            // Same role — merge content
            let prev = result.last_mut().expect("just checked result is non-empty");
            let prev_content = prev["content"].take();
            let curr_content = current["content"].take();

            let mut merged = content_to_array(prev_content);
            merged.extend(content_to_array(curr_content));

            // Remove duplicate text blocks (exact duplicates within merged content)
            merged = dedup_content_blocks(&merged);

            prev["content"] = if merged.len() == 1 && merged[0]["type"].as_str() == Some("text") {
                // Single text block — use string form
                merged.remove(0)["text"].take()
            } else {
                Value::Array(merged)
            };
        } else {
            result.push(current.clone());
        }
        i += 1;
    }

    *messages = result;
}

// ---------------------------------------------------------------------------
// 3. Filter orphaned thinking-only assistant messages
// ---------------------------------------------------------------------------

/// Remove assistant messages that contain *only* thinking blocks and no
/// other content, **unless** they share a `message.id` with a sibling
/// assistant message that has non-thinking content (these will be merged
/// later by `normalizeMessagesForAPI`).
///
/// Mirrors `filterOrphanedThinking()` in `messages.ts` (two-pass algorithm).
fn filter_orphaned_thinking_only(messages: &mut Vec<Value>) {
    // Pass 1: collect message IDs from assistant messages that have non-thinking content.
    let mut non_thinking_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    for msg in messages.iter() {
        if msg["role"].as_str() != Some("assistant") {
            continue;
        }
        let Some(blocks) = msg["content"].as_array() else {
            continue;
        };
        let has_non_thinking = blocks.iter().any(|b| {
            let btype = b["type"].as_str().unwrap_or("");
            btype != "thinking" && btype != "redacted_thinking"
        });
        if has_non_thinking && let Some(id) = msg.get("id").and_then(|v| v.as_str()) {
            non_thinking_ids.insert(id.to_string());
        }
    }

    // Pass 2: remove thinking-only messages that don't have a sibling with non-thinking content.
    messages.retain(|msg| {
        if msg["role"].as_str() != Some("assistant") {
            return true;
        }
        let Some(blocks) = msg["content"].as_array() else {
            return true;
        };
        if blocks.is_empty() {
            return false;
        }
        // Check if there's any non-thinking block
        let has_non_thinking = blocks.iter().any(|b| {
            let btype = b["type"].as_str().unwrap_or("");
            btype != "thinking" && btype != "redacted_thinking"
        });
        if has_non_thinking {
            return true;
        }
        // Thinking-only: keep if there's a sibling message with the same ID
        if let Some(id) = msg.get("id").and_then(|v| v.as_str()) {
            return non_thinking_ids.contains(id);
        }
        false
    });
}

// ---------------------------------------------------------------------------
// 4. Filter trailing thinking from last assistant
// ---------------------------------------------------------------------------

/// Strip thinking blocks from the very last assistant message.
/// The API rejects trailing thinking blocks with a mismatched signature.
fn filter_trailing_thinking_from_last_assistant(messages: &mut [Value]) {
    // Find the last assistant message
    let last_assistant_idx = messages
        .iter()
        .rposition(|m| m["role"].as_str() == Some("assistant"));

    let Some(idx) = last_assistant_idx else {
        return;
    };

    let Some(blocks) = messages[idx]["content"].as_array_mut() else {
        return;
    };

    // Remove trailing thinking/redacted_thinking blocks
    while blocks
        .last()
        .is_some_and(|b| matches!(b["type"].as_str(), Some("thinking" | "redacted_thinking")))
    {
        blocks.pop();
    }

    // If all blocks were removed, leave at least one text block
    if blocks.is_empty() {
        blocks.push(json!({"type": "text", "text": ""}));
    }
}

// ---------------------------------------------------------------------------
// 5. Filter whitespace-only assistant messages
// ---------------------------------------------------------------------------

/// Remove assistant messages whose content is entirely whitespace text.
fn filter_whitespace_only_assistant_messages(messages: &mut Vec<Value>) {
    let mut i = 0;
    while i < messages.len() {
        if messages[i]["role"].as_str() == Some("assistant")
            && is_whitespace_only_assistant(&messages[i])
        {
            messages.remove(i);
            continue;
        }
        i += 1;
    }
}

fn is_whitespace_only_assistant(msg: &Value) -> bool {
    let Some(blocks) = msg["content"].as_array() else {
        // String content
        return msg["content"].as_str().is_some_and(|s| s.trim().is_empty());
    };

    if blocks.is_empty() {
        return true;
    }

    // Check if all blocks are whitespace-only text or thinking
    blocks.iter().all(|b| match b["type"].as_str() {
        Some("text") => b["text"].as_str().is_some_and(|s| s.trim().is_empty()),
        Some("thinking" | "redacted_thinking") => true,
        _ => false,
    })
}

// ---------------------------------------------------------------------------
// 6. Ensure non-empty assistant content
// ---------------------------------------------------------------------------

/// Guarantee every assistant message has at least one content block,
/// **except** the last assistant message which may be empty for API prefill.
///
/// Mirrors `ensureNonEmptyAssistantContent()` in `messages.ts`.
const NO_CONTENT_MESSAGE: &str = "No content.";

fn ensure_non_empty_assistant_content(messages: &mut [Value]) {
    let len = messages.len();
    for (idx, msg) in messages.iter_mut().enumerate() {
        if msg["role"].as_str() != Some("assistant") {
            continue;
        }

        // Skip the last message — it may be empty for prefill
        if idx == len - 1 {
            continue;
        }

        match &mut msg["content"] {
            Value::String(s) if s.is_empty() => {
                *msg = json!({
                    "role": "assistant",
                    "content": [{"type": "text", "text": NO_CONTENT_MESSAGE}]
                });
            }
            Value::Array(blocks) if blocks.is_empty() => {
                blocks.push(json!({"type": "text", "text": NO_CONTENT_MESSAGE}));
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Convert message content (string or array) to an array of content blocks.
fn content_to_array(content: Value) -> Vec<Value> {
    match content {
        Value::String(s) => {
            if s.is_empty() {
                Vec::new()
            } else {
                vec![json!({"type": "text", "text": s})]
            }
        }
        Value::Array(blocks) => blocks,
        _ => Vec::new(),
    }
}

/// Remove duplicate content blocks (exact text/tool_use duplicates).
fn dedup_content_blocks(blocks: &[Value]) -> Vec<Value> {
    let mut seen_text = std::collections::HashSet::new();
    let mut seen_tool_result = std::collections::HashSet::new();
    let mut result = Vec::with_capacity(blocks.len());

    for block in blocks {
        match block["type"].as_str() {
            Some("text") => {
                if let Some(text) = block["text"].as_str() {
                    if !seen_text.contains(text) {
                        seen_text.insert(text.to_owned());
                        result.push(block.clone());
                    }
                } else {
                    result.push(block.clone());
                }
            }
            Some("tool_result") => {
                if let Some(id) = block["tool_use_id"].as_str() {
                    if !seen_tool_result.contains(id) {
                        seen_tool_result.insert(id.to_owned());
                        result.push(block.clone());
                    }
                } else {
                    result.push(block.clone());
                }
            }
            _ => {
                result.push(block.clone());
            }
        }
    }

    result
}

// ---------------------------------------------------------------------------
// 7. Strip empty text content blocks
// ---------------------------------------------------------------------------

/// Remove content blocks whose text is empty.  These waste tokens and
/// provide no semantic value.
fn strip_empty_content_blocks(messages: &mut [Value]) {
    for msg in messages.iter_mut() {
        if let Some(content) = msg.get_mut("content").and_then(|c| c.as_array_mut()) {
            content.retain(|block| {
                if block["type"].as_str() == Some("text") {
                    block["text"].as_str().is_none_or(|s| !s.is_empty())
                } else {
                    true
                }
            });
        }
    }
}

// ---------------------------------------------------------------------------
// 8. Ensure first message is user role
// ---------------------------------------------------------------------------

/// The Anthropic Messages API requires the conversation to start with a user
/// message.  If the first message is not a user message, insert a placeholder.
fn ensure_first_message_is_user(messages: &mut Vec<Value>) {
    if let Some(first) = messages.first()
        && first["role"].as_str() != Some("user")
    {
        messages.insert(
            0,
            json!({
                "role": "user",
                "content": [{"type": "text", "text": "[Conversation start]"}]
            }),
        );
    }
}

// ---------------------------------------------------------------------------
// 9. Validate content block types
// ---------------------------------------------------------------------------

/// Ensure all content blocks have a recognized type.  Unknown block types are
/// replaced with a text placeholder so the API doesn't reject the request.
fn validate_content_block_types(messages: &mut [Value]) {
    let valid_types = [
        "text",
        "tool_use",
        "tool_result",
        "thinking",
        "image",
        "redacted_thinking",
        "server_tool_use",
    ];
    for msg in messages.iter_mut() {
        if let Some(content) = msg.get_mut("content").and_then(|c| c.as_array_mut()) {
            for block in content.iter_mut() {
                if let Some(btype) = block.get("type").and_then(|t| t.as_str()) {
                    if !valid_types.contains(&btype) {
                        *block = json!({
                            "type": "text",
                            "text": format!("[Unsupported content block type: {}]", btype)
                        });
                    } else if btype == "tool_use" {
                        // Normalize tool_use blocks: keep only standard fields
                        let id = block.get("id").cloned().unwrap_or(Value::Null);
                        let name = block.get("name").cloned().unwrap_or(Value::Null);
                        let input = normalize_tool_input_for_api(
                            name.as_str().unwrap_or(""),
                            block.get("input").cloned().unwrap_or(json!({})),
                        );
                        *block = json!({
                            "type": "tool_use",
                            "id": id,
                            "name": name,
                            "input": input,
                        });
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 10. Filter virtual messages
// ---------------------------------------------------------------------------

/// Remove messages marked as virtual (`is_virtual == true` or `isVirtual == true`).
/// Virtual messages are internal-only and should never be sent to the API.
fn filter_virtual_messages(messages: &mut Vec<Value>) {
    messages.retain(|msg| {
        !msg.get("is_virtual")
            .or_else(|| msg.get("isVirtual"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    });
}

// ---------------------------------------------------------------------------
// 11. Sanitize error tool_result content
// ---------------------------------------------------------------------------

/// For any `tool_result` block where `is_error == true`, replace image and
/// document content blocks inside it with a text placeholder.  This prevents
/// the API from rejecting the request with a 400 error when error tool results
/// contain binary media.
fn sanitize_error_tool_result_content(messages: &mut [Value]) {
    for msg in messages.iter_mut() {
        if let Some(content) = msg.get_mut("content").and_then(|c| c.as_array_mut()) {
            for block in content.iter_mut() {
                if block["type"].as_str() == Some("tool_result")
                    && block["is_error"].as_bool() == Some(true)
                    && let Some(inner) = block.get_mut("content").and_then(|c| c.as_array_mut())
                {
                    for inner_block in inner.iter_mut() {
                        let btype = inner_block["type"].as_str().unwrap_or("");
                        if btype == "image" || btype == "document" {
                            *inner_block = json!({
                                "type": "text",
                                "text": format!("[{} content removed from error result]", btype)
                            });
                        }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Pass 0b — reorderAttachmentsForAPI
// ---------------------------------------------------------------------------

/// Reorder attachment messages so they bubble up to "stopping points".
///
/// In the TS reference, attachment messages (`type == "attachment"`) are
/// rearranged so that they appear just after the nearest preceding stopping
/// point (an assistant message or a user message whose first content block is
/// `tool_result`).  Any attachments that bubble past all stopping points end up
/// at the top of the message list.
///
/// In the Rust message model the messages have already been converted to
/// `role`/`content` JSON, so "attachment" messages are identified by
/// `role == "attachment"`.
fn reorder_attachments_for_api(messages: &mut Vec<Value>) {
    if messages.is_empty() {
        return;
    }

    // Build result backwards (push) then reverse — O(N) instead of O(N²) unshift.
    let mut result: Vec<Value> = Vec::with_capacity(messages.len());
    let mut pending_attachments: Vec<Value> = Vec::new();

    // Scan from the bottom up
    for i in (0..messages.len()).rev() {
        let msg = &messages[i];

        if msg["role"].as_str() == Some("attachment") {
            // Collect attachment to bubble up
            pending_attachments.push(msg.clone());
        } else {
            // Check if this is a stopping point
            let is_stopping_point = msg["role"].as_str() == Some("assistant")
                || (msg["role"].as_str() == Some("user")
                    && msg["content"].as_array().is_some_and(|c| {
                        c.first()
                            .is_some_and(|b| b["type"].as_str() == Some("tool_result"))
                    }));

            if is_stopping_point && !pending_attachments.is_empty() {
                // Hit a stopping point — attachments stop here (go after the stopping point).
                // pending_attachments are already reversed (pushed in reverse scan order);
                // after the final result.reverse() they will appear in original order right
                // after `msg`.
                for att in &pending_attachments {
                    result.push(att.clone());
                }
                result.push(msg.clone());
                pending_attachments.clear();
            } else {
                // Regular message — just push
                result.push(msg.clone());
            }
        }
    }

    // Any remaining attachments bubble all the way to the top.
    for att in &pending_attachments {
        result.push(att.clone());
    }

    result.reverse();
    *messages = result;
}

// ---------------------------------------------------------------------------
// Pass 0c — convertSystemLocalCommandToUser
// ---------------------------------------------------------------------------

/// Convert `system` messages with subtype `local_command` into `user` messages.
///
/// The TS reference converts these so the model can reference previous command
/// output in later turns.  In the Rust message model, system messages with
/// `local_command` subtype have `role == "system"` and `subtype == "local_command"`.
fn convert_system_local_command_to_user(messages: &mut Vec<Value>) {
    for msg in messages.iter_mut() {
        if msg["role"].as_str() == Some("system")
            && msg
                .get("subtype")
                .or_else(|| msg.get("sub_type"))
                .and_then(|v| v.as_str())
                == Some("local_command")
        {
            msg["role"] = json!("user");
            // Remove subtype since it's not a valid field for user messages
            msg.as_object_mut().map(|m| {
                m.remove("subtype");
                m.remove("sub_type");
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Pass 1c — stripErrorTriggeredAttachments
// ---------------------------------------------------------------------------

/// Placeholder text for the tool_reference turn boundary injection.
const TOOL_REFERENCE_TURN_BOUNDARY: &str = "Tool loaded.";

/// Strip document/image content blocks from meta user messages that
/// preceded PDF/image/request-too-large API errors.
///
/// When the API returns a "PDF too large" or "image too large" error, the
/// offending document or image block in the preceding meta user message is
/// stripped so it is not re-sent on subsequent API calls.
fn strip_error_triggered_attachments(messages: &mut Vec<Value>) {
    // 1. Build a map from error text patterns to block types that should be stripped.
    let error_patterns: &[(&[&str], &[&str])] = &[
        // "PDF too large" -> strip document blocks
        (&["PDF", "too large"], &["document"]),
        // "PDF is password protected" -> strip document blocks
        (&["PDF", "password protected"], &["document"]),
        // "PDF file was not valid" -> strip document blocks
        (&["PDF", "not valid"], &["document"]),
        // "Request too large" -> strip both document and image blocks
        // (must come before the general "too large" pattern)
        (&["Request too large"], &["document", "image"]),
        // "Image was too large" / "Image too large" -> strip image blocks
        (&["too large"], &["image"]),
    ];

    // 2. Walk messages to find error tool_results and build strip targets.
    //    Error tool_results are identified by is_error: true and matching text.
    //    We look for the nearest preceding meta user message (isMeta: true) and
    //    record which block types to strip.
    let mut strip_targets: Vec<(usize, Vec<String>)> = Vec::new();

    for i in 0..messages.len() {
        let msg = &messages[i];
        if msg["role"].as_str() != Some("user") {
            continue;
        }

        let Some(content) = msg["content"].as_array() else {
            continue;
        };

        // Find error tool_result blocks with matching error text
        for block in content {
            if block["type"].as_str() != Some("tool_result") {
                continue;
            }
            if block["is_error"].as_bool() != Some(true) {
                continue;
            }

            // Extract error text from the tool_result content
            let error_text = extract_text_from_tool_result(block);
            let Some(ref text) = error_text else { continue };

            // Match against known patterns
            for (patterns, block_types) in error_patterns {
                if patterns.iter().all(|p| text.contains(p)) {
                    // Walk backward to find nearest preceding meta user message
                    for j in (0..i).rev() {
                        let candidate = &messages[j];
                        if candidate["role"].as_str() == Some("user")
                            && candidate
                                .get("isMeta")
                                .or_else(|| candidate.get("is_meta"))
                                .and_then(|v| v.as_bool())
                                == Some(true)
                        {
                            let types: Vec<String> =
                                block_types.iter().map(|s| s.to_string()).collect();
                            strip_targets.push((j, types));
                            break;
                        }
                        // Skip over other error messages
                        if is_user_with_error_tool_result(candidate) {
                            continue;
                        }
                        // Stop if we hit a non-meta message
                        break;
                    }
                    break; // Only match the first matching pattern
                }
            }
        }
    }

    // 3. Apply strips — merge all types to strip per message index
    let mut merge_map: std::collections::HashMap<usize, HashSet<String>> =
        std::collections::HashMap::new();
    for (idx, types) in strip_targets {
        let entry = merge_map.entry(idx).or_default();
        for t in types {
            entry.insert(t);
        }
    }

    for (idx, types_to_strip) in merge_map {
        if let Some(content) = messages[idx]["content"].as_array_mut() {
            content.retain(|block| {
                block["type"]
                    .as_str()
                    .is_none_or(|t| !types_to_strip.contains(t))
            });
            // If all content was stripped, leave a placeholder text block
            if content.is_empty() {
                content.push(
                    json!({"type": "text", "text": "[Content removed due to API size limit]"}),
                );
            }
        }
    }
}

/// Extract the text content from a tool_result block.
fn extract_text_from_tool_result(block: &Value) -> Option<String> {
    let content = block.get("content")?;
    if let Some(s) = content.as_str() {
        return Some(s.to_owned());
    }
    if let Some(arr) = content.as_array() {
        let texts: Vec<&str> = arr
            .iter()
            .filter_map(|b| {
                if b["type"].as_str() == Some("text") {
                    b["text"].as_str()
                } else {
                    None
                }
            })
            .collect();
        if !texts.is_empty() {
            return Some(texts.join(" "));
        }
    }
    None
}

/// Check if a message is a user message containing at least one error tool_result.
fn is_user_with_error_tool_result(msg: &Value) -> bool {
    if msg["role"].as_str() != Some("user") {
        return false;
    }
    msg["content"].as_array().is_some_and(|content| {
        content.iter().any(|b| {
            b["type"].as_str() == Some("tool_result") && b["is_error"].as_bool() == Some(true)
        })
    })
}

// ---------------------------------------------------------------------------
// Pass 3c — stripAdvisorBlocks
// ---------------------------------------------------------------------------

/// Strip advisor blocks from assistant messages.
///
/// The API rejects `server_tool_use` blocks with name `"advisor"` and
/// `advisor_tool_result` blocks unless the advisor beta header is present.
/// Matches TS `stripAdvisorBlocks`.
fn strip_advisor_blocks(messages: &mut Vec<Value>) {
    let mut changed = false;
    for msg in messages.iter_mut() {
        let role = msg.get("role").and_then(Value::as_str).unwrap_or("");
        if role != "assistant" {
            continue;
        }
        let Some(content) = msg.get_mut("content").and_then(|c| c.as_array_mut()) else {
            continue;
        };
        let before_len = content.len();
        content.retain(|block| !is_advisor_block(block));
        if content.len() != before_len {
            changed = true;
            // If all blocks were removed (or only thinking/empty text remain),
            // insert a placeholder so the message is not empty.
            let all_trivial = content.iter().all(|b| {
                let t = b.get("type").and_then(Value::as_str).unwrap_or("");
                matches!(t, "thinking" | "redacted_thinking")
                    || (t == "text"
                        && b.get("text")
                            .and_then(Value::as_str)
                            .is_none_or(|s| s.trim().is_empty()))
            });
            if all_trivial {
                content.push(serde_json::json!({
                    "type": "text",
                    "text": "[Advisor response]",
                }));
            }
        }
    }
    if changed {
        // Remove entirely empty messages left after stripping
        messages.retain(|msg| {
            msg.get("content")
                .and_then(|c| c.as_array())
                .is_none_or(|arr| !arr.is_empty())
        });
    }
}

fn is_advisor_block(block: &Value) -> bool {
    let block_type = block.get("type").and_then(Value::as_str).unwrap_or("");
    if block_type == "advisor_tool_result" {
        return true;
    }
    block_type == "server_tool_use" && block.get("name").and_then(Value::as_str) == Some("advisor")
}

// ---------------------------------------------------------------------------
// Pass 3b — stripToolReferenceBlocks
// ---------------------------------------------------------------------------

/// Strip `tool_reference` blocks from `tool_result` content.
///
/// When `tool_search_enabled` is false, ALL tool_reference blocks are removed
/// from tool_result content (they are only valid with the tool search beta).
/// When `tool_search_enabled` is true, only tool_reference blocks referencing
/// tools NOT in the `available_tool_names` set are removed.
///
/// If all content inside a tool_result is removed, a placeholder text replaces it.
/// After stripping, if the message still contains tool_reference content and
/// does not already have a "Tool loaded." sibling text, one is injected.
fn strip_tool_reference_blocks(
    messages: &mut [Value],
    tool_search_enabled: bool,
    available_tool_names: Option<&HashSet<String>>,
) {
    for msg in messages.iter_mut() {
        if msg["role"].as_str() != Some("user") {
            continue;
        }
        let Some(content) = msg["content"].as_array_mut() else {
            continue;
        };

        let mut had_tool_reference = false;
        let mut still_has_tool_reference = false;

        for block in content.iter_mut() {
            if block["type"].as_str() != Some("tool_result") {
                continue;
            }
            let Some(inner) = block.get_mut("content").and_then(|c| c.as_array_mut()) else {
                continue;
            };

            let _before_count = inner.len();
            let before_has_ref = inner.iter().any(is_tool_reference_block);
            if !before_has_ref {
                continue;
            }
            had_tool_reference = true;

            if !tool_search_enabled {
                // Strip ALL tool_reference blocks
                inner.retain(|b| !is_tool_reference_block(b));
            } else if let Some(tool_names) = available_tool_names {
                // Strip only tool_reference blocks for unavailable tools
                inner.retain(|b| {
                    if !is_tool_reference_block(b) {
                        return true;
                    }
                    let Some(name) = b.get("tool_name").and_then(|n| n.as_str()) else {
                        return true; // keep if no tool_name field
                    };
                    tool_names.contains(&normalize_legacy_tool_name(name))
                });
            }

            // If all content was removed, replace with placeholder
            if inner.is_empty() {
                let placeholder = if tool_search_enabled {
                    "[Tool references removed - tools no longer available]"
                } else {
                    "[Tool references removed - tool search not enabled]"
                };
                inner.push(json!({"type": "text", "text": placeholder}));
            }

            // Check if tool_references still remain
            if inner.iter().any(is_tool_reference_block) {
                still_has_tool_reference = true;
            }
        }

        // Inject TOOL_REFERENCE_TURN_BOUNDARY sibling if tool_references remain
        // and no existing sibling starts with "Tool loaded."
        if had_tool_reference && still_has_tool_reference {
            let has_boundary = content.iter().any(|b| {
                b["type"].as_str() == Some("text")
                    && b["text"]
                        .as_str()
                        .is_some_and(|t| t.starts_with(TOOL_REFERENCE_TURN_BOUNDARY))
            });
            if !has_boundary {
                content.push(json!({
                    "type": "text",
                    "text": TOOL_REFERENCE_TURN_BOUNDARY
                }));
            }
        }
    }
}

/// Check if a content block is a tool_reference block.
fn is_tool_reference_block(block: &Value) -> bool {
    block["type"].as_str() == Some("tool_reference")
}

/// Normalize legacy tool names (e.g. add missing prefixes).
/// Mirrors the TS `normalizeLegacyToolName` function.
fn normalize_legacy_tool_name(name: &str) -> String {
    // The TS reference handles legacy tool name normalization.
    // For now, return the name as-is since the Rust codebase uses
    // the same naming convention.  This can be extended if needed.
    name.to_owned()
}

// ---------------------------------------------------------------------------
// Pass 6c — filterUnavailableToolUseBlocks
// ---------------------------------------------------------------------------

/// Filter out `tool_use` blocks in assistant messages where the tool name is
/// NOT in the available tool set.  This prevents sending tool calls for tools
/// that don't exist in the current session (e.g., MCP server was disconnected).
fn filter_unavailable_tool_use_blocks(
    messages: &mut [Value],
    available_tool_names: &HashSet<String>,
) {
    for msg in messages.iter_mut() {
        if msg["role"].as_str() != Some("assistant") {
            continue;
        }
        let Some(content) = msg["content"].as_array_mut() else {
            continue;
        };

        content.retain(|block| {
            if block["type"].as_str() != Some("tool_use") {
                return true;
            }
            let Some(name) = block["name"].as_str() else {
                return true; // keep if no name field
            };
            available_tool_names.contains(&normalize_legacy_tool_name(name))
        });
    }
}

// ---------------------------------------------------------------------------
// 12. Validate images for API (size check)
// ---------------------------------------------------------------------------

/// Maximum base64 string length for images accepted by the Anthropic API (5 MB).
/// Mirrors `API_IMAGE_MAX_BASE64_SIZE` from `imageValidation.ts`.
const API_IMAGE_MAX_BASE64_SIZE: usize = 5 * 1024 * 1024;

/// Check all image content blocks.  If the base64 `source.data` exceeds 5 MB
/// in base64 string length, replace the block with a text placeholder so the
/// API does not reject the request.
fn validate_images_for_api(messages: &mut [Value]) {
    for msg in messages.iter_mut() {
        if let Some(content) = msg.get_mut("content").and_then(|c| c.as_array_mut()) {
            for block in content.iter_mut() {
                if block["type"].as_str() == Some("image")
                    && let Some(data) = block
                        .get("source")
                        .and_then(|s| s.get("data"))
                        .and_then(|d| d.as_str())
                {
                    // Check base64 string length directly (matches TS behavior)
                    if data.len() > API_IMAGE_MAX_BASE64_SIZE {
                        *block = json!({
                            "type": "text",
                            "text": "[Image too large for API]"
                        });
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_tool_result_pairing_orphaned_tool_use() {
        let mut messages = vec![
            json!({"role": "assistant", "content": [
                {"type": "text", "text": "Let me check."},
                {"type": "tool_use", "id": "tool-1", "name": "Read", "input": {"path": "/foo"}}
            ]}),
            json!({"role": "user", "content": "next message"}),
        ];
        ensure_tool_result_pairing(&mut messages);
        // Should have injected synthetic tool_result
        assert_eq!(messages[1]["role"], "user");
        let content = messages[1]["content"].as_array().unwrap();
        assert!(
            content
                .iter()
                .any(|b| b["type"].as_str() == Some("tool_result")
                    && b["tool_use_id"].as_str() == Some("tool-1"))
        );
    }

    #[test]
    fn test_tool_result_pairing_already_paired() {
        let mut messages = vec![
            json!({"role": "assistant", "content": [
                {"type": "tool_use", "id": "tool-1", "name": "Read", "input": {}}
            ]}),
            json!({"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "tool-1", "content": "ok"}
            ]}),
        ];
        ensure_tool_result_pairing(&mut messages);
        // No insertion needed
        assert_eq!(messages.len(), 2);
    }

    #[test]
    fn test_merge_consecutive_user_messages() {
        let mut messages = vec![
            json!({"role": "user", "content": "hello"}),
            json!({"role": "user", "content": "world"}),
        ];
        merge_consecutive_same_role(&mut messages);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], "user");
    }

    #[test]
    fn test_merge_consecutive_assistant_messages() {
        let mut messages = vec![
            json!({"role": "assistant", "content": [
                {"type": "text", "text": "part1"}
            ]}),
            json!({"role": "assistant", "content": [
                {"type": "text", "text": "part2"}
            ]}),
        ];
        merge_consecutive_same_role(&mut messages);
        assert_eq!(messages.len(), 1);
        let content = messages[0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
    }

    #[test]
    fn test_no_merge_different_roles() {
        let mut messages = vec![
            json!({"role": "user", "content": "hello"}),
            json!({"role": "assistant", "content": "hi"}),
            json!({"role": "user", "content": "bye"}),
        ];
        merge_consecutive_same_role(&mut messages);
        assert_eq!(messages.len(), 3);
    }

    #[test]
    fn test_filter_orphaned_thinking_only() {
        let mut messages = vec![
            json!({"role": "user", "content": "hello"}),
            json!({"role": "assistant", "content": [
                {"type": "thinking", "thinking": "hmm", "signature": "abc"}
            ]}),
            json!({"role": "user", "content": "world"}),
        ];
        filter_orphaned_thinking_only(&mut messages);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[1]["role"], "user");
    }

    #[test]
    fn test_keep_assistant_with_mixed_content() {
        let mut messages = vec![json!({"role": "assistant", "content": [
            {"type": "thinking", "thinking": "hmm", "signature": "abc"},
            {"type": "text", "text": "result"}
        ]})];
        filter_orphaned_thinking_only(&mut messages);
        assert_eq!(messages.len(), 1);
    }

    #[test]
    fn test_filter_trailing_thinking() {
        let mut messages = vec![json!({
            "role": "assistant",
            "content": [
                {"type": "text", "text": "hello"},
                {"type": "thinking", "thinking": "hmm", "signature": "abc"},
                {"type": "redacted_thinking", "data": "xyz"}
            ]
        })];
        filter_trailing_thinking_from_last_assistant(&mut messages);
        let content = messages[0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "text");
    }

    #[test]
    fn test_filter_whitespace_only_assistant() {
        let mut messages = vec![
            json!({"role": "user", "content": "hello"}),
            json!({"role": "assistant", "content": "   "}),
            json!({"role": "user", "content": "world"}),
        ];
        filter_whitespace_only_assistant_messages(&mut messages);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[1]["role"], "user");
    }

    #[test]
    fn test_ensure_non_empty_assistant() {
        // Empty assistant in the middle of the array should be filled.
        // The last message is always skipped (API prefill support).
        let mut messages = vec![
            json!({"role": "assistant", "content": []}),
            json!({"role": "user", "content": "next"}),
        ];
        ensure_non_empty_assistant_content(&mut messages);
        let content = messages[0]["content"].as_array().unwrap();
        assert!(!content.is_empty());
        // Verify filler text matches TS
        assert_eq!(content[0]["text"].as_str(), Some("No content."));
    }

    #[test]
    fn test_ensure_non_empty_assistant_skips_last() {
        // Empty assistant as the very last message should be left alone (prefill).
        let mut messages = vec![json!({
            "role": "assistant",
            "content": []
        })];
        ensure_non_empty_assistant_content(&mut messages);
        let content = messages[0]["content"].as_array().unwrap();
        assert!(content.is_empty());
    }

    #[test]
    fn test_full_normalization_pipeline() {
        let mut messages = vec![
            json!({"role": "user", "content": "read file"}),
            json!({"role": "assistant", "content": [
                {"type": "tool_use", "id": "t1", "name": "Read", "input": {"path": "/x"}}
            ]}),
            json!({"role": "assistant", "content": [
                {"type": "thinking", "thinking": "hmm", "signature": "s1"}
            ]}),
            json!({"role": "user", "content": "thanks"}),
            json!({"role": "assistant", "content": "   "}),
            json!({"role": "user", "content": "bye"}),
        ];
        normalize_messages_for_api(&mut messages);

        // Should have: user, assistant(with tool_use), user(with synthetic tool_result + "thanks" merged), user("bye")
        // After orphan thinking filter removes the thinking-only assistant
        // After whitespace filter removes "   " assistant
        // After merge: consecutive users are merged

        // Verify role alternation
        for i in 1..messages.len() {
            assert_ne!(
                messages[i]["role"].as_str(),
                messages[i - 1]["role"].as_str(),
                "Consecutive messages at index {} have same role",
                i
            );
        }

        // Verify tool_use has a matching tool_result
        let tool_use_ids: Vec<&str> = messages
            .iter()
            .flat_map(|m| m["content"].as_array().into_iter().flatten())
            .filter(|b| b["type"].as_str() == Some("tool_use"))
            .filter_map(|b| b["id"].as_str())
            .collect();
        let tool_result_ids: Vec<&str> = messages
            .iter()
            .flat_map(|m| m["content"].as_array().into_iter().flatten())
            .filter(|b| b["type"].as_str() == Some("tool_result"))
            .filter_map(|b| b["tool_use_id"].as_str())
            .collect();
        for id in &tool_use_ids {
            assert!(
                tool_result_ids.contains(id),
                "Missing tool_result for tool_use id {}",
                id
            );
        }

        // Verify first message is user
        assert_eq!(messages[0]["role"], "user");
    }

    #[test]
    fn test_strip_empty_content_blocks() {
        let mut messages = vec![json!({
            "role": "user",
            "content": [
                {"type": "text", "text": ""},
                {"type": "text", "text": "keep me"},
                {"type": "tool_result", "tool_use_id": "t1", "content": "ok"}
            ]
        })];
        strip_empty_content_blocks(&mut messages);
        let content = messages[0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["text"].as_str(), Some("keep me"));
    }

    #[test]
    fn test_ensure_first_message_is_user_inserts_placeholder() {
        let mut messages = vec![json!({
            "role": "assistant",
            "content": [{"type": "text", "text": "hi"}]
        })];
        ensure_first_message_is_user(&mut messages);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "user");
        let content = messages[0]["content"].as_array().unwrap();
        assert!(
            content[0]["text"]
                .as_str()
                .unwrap()
                .contains("Conversation start")
        );
    }

    #[test]
    fn test_ensure_first_message_is_user_noop_when_already_user() {
        let mut messages = vec![json!({"role": "user", "content": "hello"})];
        ensure_first_message_is_user(&mut messages);
        assert_eq!(messages.len(), 1);
    }

    #[test]
    fn test_validate_content_block_types_replaces_unknown() {
        let mut messages = vec![json!({
            "role": "user",
            "content": [
                {"type": "text", "text": "hello"},
                {"type": "unknown_type", "data": "x"}
            ]
        })];
        validate_content_block_types(&mut messages);
        let content = messages[0]["content"].as_array().unwrap();
        assert_eq!(content[1]["type"], "text");
        assert!(
            content[1]["text"]
                .as_str()
                .unwrap()
                .contains("unknown_type")
        );
    }

    #[test]
    fn test_validate_content_block_types_keeps_valid() {
        let mut messages = vec![json!({
            "role": "assistant",
            "content": [
                {"type": "text", "text": "thinking..."},
                {"type": "tool_use", "id": "t1", "name": "Read", "input": {}},
                {"type": "thinking", "thinking": "hmm", "signature": "sig"}
            ]
        })];
        validate_content_block_types(&mut messages);
        let content = messages[0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 3);
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[1]["type"], "tool_use");
        assert_eq!(content[2]["type"], "thinking");
    }

    // -- New pass tests --------------------------------------------------------

    #[test]
    fn test_filter_virtual_messages_removes_is_virtual() {
        let mut messages = vec![
            json!({"role": "user", "content": "real", "is_virtual": true}),
            json!({"role": "user", "content": "keep"}),
            json!({"role": "assistant", "content": "also real", "isVirtual": true}),
        ];
        filter_virtual_messages(&mut messages);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["content"], "keep");
    }

    #[test]
    fn test_filter_virtual_messages_keeps_non_virtual() {
        let mut messages = vec![
            json!({"role": "user", "content": "a"}),
            json!({"role": "assistant", "content": "b"}),
        ];
        filter_virtual_messages(&mut messages);
        assert_eq!(messages.len(), 2);
    }

    #[test]
    fn test_sanitize_error_tool_result_removes_images() {
        let mut messages = vec![json!({
            "role": "user",
            "content": [
                {
                    "type": "tool_result",
                    "tool_use_id": "t1",
                    "is_error": true,
                    "content": [
                        {"type": "text", "text": "Error occurred"},
                        {"type": "image", "source": {"type": "base64", "data": "abc"}},
                        {"type": "document", "source": {"type": "base64", "data": "def"}}
                    ]
                }
            ]
        })];
        sanitize_error_tool_result_content(&mut messages);
        let content = messages[0]["content"].as_array().unwrap();
        let inner = content[0]["content"].as_array().unwrap();
        assert_eq!(inner[0]["type"], "text");
        assert_eq!(inner[0]["text"], "Error occurred");
        // Image replaced
        assert_eq!(inner[1]["type"], "text");
        assert!(
            inner[1]["text"]
                .as_str()
                .unwrap()
                .contains("image content removed from error result")
        );
        // Document replaced
        assert_eq!(inner[2]["type"], "text");
        assert!(
            inner[2]["text"]
                .as_str()
                .unwrap()
                .contains("document content removed from error result")
        );
    }

    #[test]
    fn test_sanitize_error_tool_result_keeps_non_error() {
        let mut messages = vec![json!({
            "role": "user",
            "content": [
                {
                    "type": "tool_result",
                    "tool_use_id": "t1",
                    "content": [
                        {"type": "image", "source": {"type": "base64", "data": "abc"}}
                    ]
                }
            ]
        })];
        sanitize_error_tool_result_content(&mut messages);
        let content = messages[0]["content"].as_array().unwrap();
        let inner = content[0]["content"].as_array().unwrap();
        // Image should remain (not an error result)
        assert_eq!(inner[0]["type"], "image");
    }

    #[test]
    fn test_validate_images_for_api_replaces_oversized() {
        // Create a base64 string that decodes to > 20MB
        // We need ~26.67M chars of base64 to represent 20MB decoded
        // For test, we'll use a smaller threshold by creating a large string
        let big_data = "A".repeat(30_000_000); // ~30MB base64 -> ~22.5MB decoded
        let mut messages = vec![json!({
            "role": "user",
            "content": [
                {"type": "image", "source": {"type": "base64", "data": big_data}},
                {"type": "text", "text": "label"}
            ]
        })];
        validate_images_for_api(&mut messages);
        let content = messages[0]["content"].as_array().unwrap();
        // First block should have been replaced with text placeholder
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "[Image too large for API]");
        // Second block untouched
        assert_eq!(content[1]["type"], "text");
        assert_eq!(content[1]["text"], "label");
    }

    #[test]
    fn test_validate_images_for_api_keeps_small_images() {
        let small_data = "smallbase64data";
        let mut messages = vec![json!({
            "role": "user",
            "content": [
                {"type": "image", "source": {"type": "base64", "data": small_data}}
            ]
        })];
        validate_images_for_api(&mut messages);
        let content = messages[0]["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "image");
    }

    #[test]
    fn test_normalize_tool_inputs_strips_extra_fields() {
        let mut messages = vec![json!({
            "role": "assistant",
            "content": [
                {
                    "type": "tool_use",
                    "id": "t1",
                    "name": "Read",
                    "input": {"path": "/foo"},
                    "extra_field": "should be removed",
                    "another_extra": 42
                }
            ]
        })];
        validate_content_block_types(&mut messages);
        let content = messages[0]["content"].as_array().unwrap();
        let tool_use = &content[0];
        assert_eq!(tool_use["type"], "tool_use");
        assert_eq!(tool_use["id"], "t1");
        assert_eq!(tool_use["name"], "Read");
        assert_eq!(tool_use["input"]["path"], "/foo");
        // Extra fields should be stripped
        assert!(tool_use.get("extra_field").is_none());
        assert!(tool_use.get("another_extra").is_none());
    }

    // =========================================================================
    // Pass 0b — reorderAttachmentsForAPI
    // =========================================================================

    #[test]
    fn test_reorder_attachments_bubbles_to_top() {
        // Attachments with no stopping points should bubble to the top
        let mut messages = vec![
            json!({"role": "attachment", "content": "att1"}),
            json!({"role": "user", "content": "hello"}),
            json!({"role": "attachment", "content": "att2"}),
        ];
        reorder_attachments_for_api(&mut messages);
        // Attachments bubble to the top, in original order
        assert_eq!(messages[0]["role"], "attachment");
        assert_eq!(messages[1]["role"], "attachment");
        assert_eq!(messages[2]["role"], "user");
    }

    #[test]
    fn test_reorder_attachments_stops_at_assistant() {
        let mut messages = vec![
            json!({"role": "user", "content": "hello"}),
            json!({"role": "assistant", "content": "response"}),
            json!({"role": "attachment", "content": "att1"}),
            json!({"role": "user", "content": "followup"}),
        ];
        reorder_attachments_for_api(&mut messages);
        // Attachment should appear after the assistant message
        let att_idx = messages
            .iter()
            .position(|m| m["role"].as_str() == Some("attachment"))
            .unwrap();
        let asst_idx = messages
            .iter()
            .position(|m| m["role"].as_str() == Some("assistant"))
            .unwrap();
        assert!(att_idx > asst_idx, "attachment should be after assistant");
        // Followup user should be after attachment
        let followup_idx = messages
            .iter()
            .position(|m| {
                m["role"].as_str() == Some("user") && m["content"].as_str() == Some("followup")
            })
            .unwrap();
        assert!(
            followup_idx > att_idx,
            "followup should be after attachment"
        );
    }

    #[test]
    fn test_reorder_attachments_stops_at_tool_result_user() {
        let mut messages = vec![
            json!({"role": "user", "content": "hello"}),
            json!({"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "t1", "content": "ok"}
            ]}),
            json!({"role": "attachment", "content": "att1"}),
        ];
        reorder_attachments_for_api(&mut messages);
        // Attachment should appear after the tool_result user message
        let att_idx = messages
            .iter()
            .position(|m| m["role"].as_str() == Some("attachment"))
            .unwrap();
        let tr_idx = messages
            .iter()
            .position(|m| {
                m["role"].as_str() == Some("user")
                    && m["content"].as_array().is_some_and(|c| {
                        c.first()
                            .is_some_and(|b| b["type"].as_str() == Some("tool_result"))
                    })
            })
            .unwrap();
        assert!(
            att_idx > tr_idx,
            "attachment should be after tool_result user"
        );
    }

    #[test]
    fn test_reorder_attachments_no_attachments() {
        let mut messages = vec![
            json!({"role": "user", "content": "hello"}),
            json!({"role": "assistant", "content": "hi"}),
            json!({"role": "user", "content": "bye"}),
        ];
        reorder_attachments_for_api(&mut messages);
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0]["content"], "hello");
        assert_eq!(messages[1]["content"], "hi");
        assert_eq!(messages[2]["content"], "bye");
    }

    #[test]
    fn test_reorder_attachments_preserves_order_at_stopping_point() {
        // Multiple attachments should maintain their original relative order
        let mut messages = vec![
            json!({"role": "user", "content": "hello"}),
            json!({"role": "attachment", "content": "att1"}),
            json!({"role": "attachment", "content": "att2"}),
            json!({"role": "assistant", "content": "response"}),
            json!({"role": "user", "content": "bye"}),
        ];
        reorder_attachments_for_api(&mut messages);
        // att1 and att2 should be after "response" (stopping point) in original order
        let atts: Vec<&str> = messages
            .iter()
            .filter(|m| m["role"].as_str() == Some("attachment"))
            .filter_map(|m| m["content"].as_str())
            .collect();
        assert_eq!(atts, vec!["att1", "att2"]);
    }

    // =========================================================================
    // Pass 0c — convertSystemLocalCommandToUser
    // =========================================================================

    #[test]
    fn test_convert_system_local_command_to_user() {
        let mut messages = vec![
            json!({"role": "system", "subtype": "local_command", "content": "ls -la"}),
            json!({"role": "user", "content": "hello"}),
        ];
        convert_system_local_command_to_user(&mut messages);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[0]["content"], "ls -la");
        // subtype should be removed
        assert!(messages[0].get("subtype").is_none());
    }

    #[test]
    fn test_convert_system_local_command_with_sub_type_variant() {
        let mut messages =
            vec![json!({"role": "system", "sub_type": "local_command", "content": "pwd"})];
        convert_system_local_command_to_user(&mut messages);
        assert_eq!(messages[0]["role"], "user");
        assert!(messages[0].get("sub_type").is_none());
    }

    #[test]
    fn test_convert_system_non_local_command_unchanged() {
        let mut messages = vec![json!({"role": "system", "subtype": "other", "content": "info"})];
        convert_system_local_command_to_user(&mut messages);
        assert_eq!(messages[0]["role"], "system");
    }

    #[test]
    fn test_convert_system_no_messages_unchanged() {
        let mut messages = vec![
            json!({"role": "user", "content": "hello"}),
            json!({"role": "assistant", "content": "hi"}),
        ];
        convert_system_local_command_to_user(&mut messages);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[1]["role"], "assistant");
    }

    // =========================================================================
    // Pass 1c — stripErrorTriggeredAttachments
    // =========================================================================

    #[test]
    fn test_strip_error_triggered_pdf_too_large() {
        let mut messages = vec![
            json!({"role": "user", "content": [
                {"type": "text", "text": "Please read this PDF"},
                {"type": "document", "source": {"type": "base64", "data": "pdfdata"}}
            ], "isMeta": true}),
            json!({"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "t1", "is_error": true, "content": [
                    {"type": "text", "text": "PDF too large (max 100 pages, 30MB). Try reading the file a different way."}
                ]}
            ]}),
        ];
        strip_error_triggered_attachments(&mut messages);
        let content = messages[0]["content"].as_array().unwrap();
        // Document block should be stripped, text block should remain
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "Please read this PDF");
    }

    #[test]
    fn test_strip_error_triggered_image_too_large() {
        let mut messages = vec![
            json!({"role": "user", "content": [
                {"type": "text", "text": "Analyze this image"},
                {"type": "image", "source": {"type": "base64", "data": "imgdata"}},
                {"type": "document", "source": {"type": "base64", "data": "docdata"}}
            ], "isMeta": true}),
            json!({"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "t1", "is_error": true, "content": [
                    {"type": "text", "text": "Image was too large. Double press esc to go back and try again."}
                ]}
            ]}),
        ];
        strip_error_triggered_attachments(&mut messages);
        let content = messages[0]["content"].as_array().unwrap();
        // Image block should be stripped, text and document should remain
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[1]["type"], "document");
    }

    #[test]
    fn test_strip_error_triggered_request_too_large() {
        let mut messages = vec![
            json!({"role": "user", "content": [
                {"type": "text", "text": "Check this file"},
                {"type": "document", "source": {"type": "base64", "data": "d1"}},
                {"type": "image", "source": {"type": "base64", "data": "i1"}}
            ], "isMeta": true}),
            json!({"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "t1", "is_error": true, "content": [
                    {"type": "text", "text": "Request too large (max 30MB). Try with a smaller file."}
                ]}
            ]}),
        ];
        strip_error_triggered_attachments(&mut messages);
        let content = messages[0]["content"].as_array().unwrap();
        // Both document and image should be stripped
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "text");
    }

    #[test]
    fn test_strip_error_triggered_all_content_removed() {
        let mut messages = vec![
            json!({"role": "user", "content": [
                {"type": "document", "source": {"type": "base64", "data": "d1"}}
            ], "isMeta": true}),
            json!({"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "t1", "is_error": true, "content": [
                    {"type": "text", "text": "PDF too large (max 100 pages). Try again."}
                ]}
            ]}),
        ];
        strip_error_triggered_attachments(&mut messages);
        let content = messages[0]["content"].as_array().unwrap();
        // All content stripped — placeholder should remain
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "text");
        assert!(
            content[0]["text"]
                .as_str()
                .unwrap()
                .contains("Content removed")
        );
    }

    #[test]
    fn test_strip_error_triggered_non_meta_user_not_stripped() {
        let mut messages = vec![
            json!({"role": "user", "content": [
                {"type": "text", "text": "Please read this"},
                {"type": "document", "source": {"type": "base64", "data": "pdfdata"}}
            ]}),
            json!({"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "t1", "is_error": true, "content": [
                    {"type": "text", "text": "PDF too large (max 100 pages). Try again."}
                ]}
            ]}),
        ];
        strip_error_triggered_attachments(&mut messages);
        let content = messages[0]["content"].as_array().unwrap();
        // Non-meta user message should NOT be stripped
        assert_eq!(content.len(), 2);
        assert_eq!(content[1]["type"], "document");
    }

    #[test]
    fn test_strip_error_triggered_no_error_no_strip() {
        let mut messages = vec![
            json!({"role": "user", "content": [
                {"type": "text", "text": "Please read this"},
                {"type": "document", "source": {"type": "base64", "data": "pdfdata"}}
            ], "isMeta": true}),
            json!({"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "t1", "content": [
                    {"type": "text", "text": "All good"}
                ]}
            ]}),
        ];
        strip_error_triggered_attachments(&mut messages);
        let content = messages[0]["content"].as_array().unwrap();
        // No error, nothing stripped
        assert_eq!(content.len(), 2);
    }

    // =========================================================================
    // Pass 3b — stripToolReferenceBlocks
    // =========================================================================

    #[test]
    fn test_strip_tool_reference_blocks_tool_search_disabled() {
        let mut messages = vec![json!({
            "role": "user",
            "content": [
                {
                    "type": "tool_result",
                    "tool_use_id": "t1",
                    "content": [
                        {"type": "tool_reference", "tool_name": "mcp__server__tool1"},
                        {"type": "text", "text": "result text"}
                    ]
                }
            ]
        })];
        strip_tool_reference_blocks(&mut messages, false, None);
        let content = messages[0]["content"].as_array().unwrap();
        let inner = content[0]["content"].as_array().unwrap();
        // tool_reference should be removed, text should remain
        assert_eq!(inner.len(), 1);
        assert_eq!(inner[0]["type"], "text");
        assert_eq!(inner[0]["text"], "result text");
    }

    #[test]
    fn test_strip_tool_reference_blocks_all_removed_placeholder() {
        let mut messages = vec![json!({
            "role": "user",
            "content": [
                {
                    "type": "tool_result",
                    "tool_use_id": "t1",
                    "content": [
                        {"type": "tool_reference", "tool_name": "mcp__server__tool1"}
                    ]
                }
            ]
        })];
        strip_tool_reference_blocks(&mut messages, false, None);
        let content = messages[0]["content"].as_array().unwrap();
        let inner = content[0]["content"].as_array().unwrap();
        // All content removed — placeholder should be there
        assert_eq!(inner.len(), 1);
        assert_eq!(inner[0]["type"], "text");
        assert!(
            inner[0]["text"]
                .as_str()
                .unwrap()
                .contains("tool search not enabled")
        );
    }

    #[test]
    fn test_strip_tool_reference_blocks_tool_search_enabled_keep_available() {
        let available: HashSet<String> = ["mcp__server__tool1".to_string()].into_iter().collect();
        let mut messages = vec![json!({
            "role": "user",
            "content": [
                {
                    "type": "tool_result",
                    "tool_use_id": "t1",
                    "content": [
                        {"type": "tool_reference", "tool_name": "mcp__server__tool1"},
                        {"type": "tool_reference", "tool_name": "mcp__server__tool2"},
                        {"type": "text", "text": "result text"}
                    ]
                }
            ]
        })];
        strip_tool_reference_blocks(&mut messages, true, Some(&available));
        let content = messages[0]["content"].as_array().unwrap();
        let inner = content[0]["content"].as_array().unwrap();
        // Only unavailable tool_reference should be stripped
        assert_eq!(inner.len(), 2);
        assert_eq!(inner[0]["type"], "tool_reference");
        assert_eq!(inner[0]["tool_name"], "mcp__server__tool1");
        assert_eq!(inner[1]["type"], "text");
    }

    #[test]
    fn test_strip_tool_reference_blocks_tool_search_enabled_all_unavailable() {
        let available: HashSet<String> = HashSet::new();
        let mut messages = vec![json!({
            "role": "user",
            "content": [
                {
                    "type": "tool_result",
                    "tool_use_id": "t1",
                    "content": [
                        {"type": "tool_reference", "tool_name": "mcp__server__tool1"}
                    ]
                }
            ]
        })];
        strip_tool_reference_blocks(&mut messages, true, Some(&available));
        let content = messages[0]["content"].as_array().unwrap();
        let inner = content[0]["content"].as_array().unwrap();
        // All unavailable — placeholder
        assert_eq!(inner.len(), 1);
        assert!(
            inner[0]["text"]
                .as_str()
                .unwrap()
                .contains("tools no longer available")
        );
    }

    #[test]
    fn test_strip_tool_reference_blocks_injects_turn_boundary() {
        let mut messages = vec![json!({
            "role": "user",
            "content": [
                {
                    "type": "tool_result",
                    "tool_use_id": "t1",
                    "content": [
                        {"type": "tool_reference", "tool_name": "mcp__server__tool1"},
                        {"type": "text", "text": "result"}
                    ]
                }
            ]
        })];
        let available: HashSet<String> = ["mcp__server__tool1".to_string()].into_iter().collect();
        strip_tool_reference_blocks(&mut messages, true, Some(&available));
        let content = messages[0]["content"].as_array().unwrap();
        // Should have tool_result + "Tool loaded." sibling
        assert!(content.iter().any(|b| {
            b["type"].as_str() == Some("text") && b["text"].as_str() == Some("Tool loaded.")
        }));
    }

    #[test]
    fn test_strip_tool_reference_blocks_no_double_boundary() {
        let mut messages = vec![json!({
            "role": "user",
            "content": [
                {
                    "type": "tool_result",
                    "tool_use_id": "t1",
                    "content": [
                        {"type": "tool_reference", "tool_name": "mcp__server__tool1"},
                        {"type": "text", "text": "result"}
                    ]
                },
                {"type": "text", "text": "Tool loaded."}
            ]
        })];
        let available: HashSet<String> = ["mcp__server__tool1".to_string()].into_iter().collect();
        strip_tool_reference_blocks(&mut messages, true, Some(&available));
        let content = messages[0]["content"].as_array().unwrap();
        // Should NOT add a duplicate boundary
        let boundary_count = content
            .iter()
            .filter(|b| {
                b["type"].as_str() == Some("text") && b["text"].as_str() == Some("Tool loaded.")
            })
            .count();
        assert_eq!(boundary_count, 1);
    }

    #[test]
    fn test_strip_tool_reference_blocks_non_user_message_unchanged() {
        let mut messages = vec![json!({
            "role": "assistant",
            "content": [
                {"type": "text", "text": "hello"}
            ]
        })];
        strip_tool_reference_blocks(&mut messages, false, None);
        assert_eq!(messages[0]["content"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn test_strip_tool_reference_no_tool_results_unchanged() {
        let mut messages = vec![json!({
            "role": "user",
            "content": [
                {"type": "text", "text": "hello"}
            ]
        })];
        strip_tool_reference_blocks(&mut messages, false, None);
        let content = messages[0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "text");
    }

    // =========================================================================
    // Pass 6c — filterUnavailableToolUseBlocks
    // =========================================================================

    #[test]
    fn test_filter_unavailable_tool_use_blocks_removes_unknown() {
        let available: HashSet<String> = ["Read".to_string(), "Write".to_string()]
            .into_iter()
            .collect();
        let mut messages = vec![json!({
            "role": "assistant",
            "content": [
                {"type": "text", "text": "Let me help"},
                {"type": "tool_use", "id": "t1", "name": "Read", "input": {}},
                {"type": "tool_use", "id": "t2", "name": "UnknownTool", "input": {}},
                {"type": "tool_use", "id": "t3", "name": "Write", "input": {}}
            ]
        })];
        filter_unavailable_tool_use_blocks(&mut messages, &available);
        let content = messages[0]["content"].as_array().unwrap();
        // UnknownTool should be removed
        assert_eq!(content.len(), 3);
        let tool_names: Vec<&str> = content
            .iter()
            .filter(|b| b["type"].as_str() == Some("tool_use"))
            .filter_map(|b| b["name"].as_str())
            .collect();
        assert_eq!(tool_names, vec!["Read", "Write"]);
    }

    #[test]
    fn test_filter_unavailable_tool_use_blocks_all_available() {
        let available: HashSet<String> = ["Read".to_string()].into_iter().collect();
        let mut messages = vec![json!({
            "role": "assistant",
            "content": [
                {"type": "tool_use", "id": "t1", "name": "Read", "input": {}}
            ]
        })];
        filter_unavailable_tool_use_blocks(&mut messages, &available);
        let content = messages[0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
    }

    #[test]
    fn test_filter_unavailable_tool_use_blocks_user_message_unchanged() {
        let available: HashSet<String> = HashSet::new();
        let mut messages = vec![json!({
            "role": "user",
            "content": [
                {"type": "tool_result", "tool_use_id": "t1", "content": "ok"}
            ]
        })];
        filter_unavailable_tool_use_blocks(&mut messages, &available);
        let content = messages[0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
    }

    // =========================================================================
    // Full pipeline with config
    // =========================================================================

    #[test]
    fn test_full_pipeline_with_tool_search_config() {
        let mut messages = vec![
            json!({"role": "user", "content": "read file"}),
            json!({"role": "assistant", "content": [
                {"type": "tool_use", "id": "t1", "name": "Read", "input": {"path": "/x"}},
                {"type": "tool_use", "id": "t2", "name": "UnknownTool", "input": {}}
            ]}),
            json!({"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "t1", "content": "file contents"},
                {"type": "tool_result", "tool_use_id": "t2", "content": "unknown result"}
            ]}),
            json!({"role": "user", "content": "thanks"}),
        ];

        let available: HashSet<String> = ["Read".to_string()].into_iter().collect();
        let config = NormalizeConfig {
            tool_search_enabled: true,
            available_tool_names: Some(&available),
        };
        normalize_messages_for_api_with_config(&mut messages, config);

        // Verify role alternation
        for i in 1..messages.len() {
            assert_ne!(
                messages[i]["role"].as_str(),
                messages[i - 1]["role"].as_str(),
                "Consecutive messages at index {} have same role",
                i
            );
        }

        // Verify first message is user
        assert_eq!(messages[0]["role"], "user");

        // Verify UnknownTool tool_use was filtered out
        let tool_use_names: Vec<&str> = messages
            .iter()
            .flat_map(|m| m["content"].as_array().into_iter().flatten())
            .filter(|b| b["type"].as_str() == Some("tool_use"))
            .filter_map(|b| b["name"].as_str())
            .collect();
        assert!(
            !tool_use_names.contains(&"UnknownTool"),
            "UnknownTool should have been filtered out"
        );
        assert!(tool_use_names.contains(&"Read"), "Read should be kept");
    }

    #[test]
    fn test_full_pipeline_with_tool_references() {
        let mut messages = vec![
            json!({"role": "user", "content": "use the tool"}),
            json!({"role": "assistant", "content": [
                {"type": "tool_use", "id": "t1", "name": "ToolSearch", "input": {}}
            ]}),
            json!({"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "t1", "content": [
                    {"type": "tool_reference", "tool_name": "mcp__server__tool1"},
                    {"type": "text", "text": "Found the tool"}
                ]}
            ]}),
        ];

        let config = NormalizeConfig {
            tool_search_enabled: false,
            available_tool_names: None,
        };
        normalize_messages_for_api_with_config(&mut messages, config);

        // tool_reference blocks should have been stripped
        let has_tool_ref = messages
            .iter()
            .flat_map(|m| m["content"].as_array().into_iter().flatten())
            .any(|b| b["type"].as_str() == Some("tool_reference"));
        assert!(
            !has_tool_ref,
            "tool_reference blocks should be stripped when tool search is disabled"
        );
    }

    #[test]
    fn test_full_pipeline_system_local_command() {
        let mut messages = vec![
            json!({"role": "system", "subtype": "local_command", "content": "ls -la"}),
            json!({"role": "assistant", "content": "response"}),
            json!({"role": "user", "content": "thanks"}),
        ];
        normalize_messages_for_api(&mut messages);

        // system/local_command should have been converted to user
        assert_eq!(messages[0]["role"], "user");
        // First message should be user (either the converted one or a placeholder)
        assert_eq!(messages[0]["role"], "user");
    }

    #[test]
    fn test_full_pipeline_error_attachment_stripping() {
        let mut messages = vec![
            json!({"role": "user", "content": [
                {"type": "text", "text": "Read this PDF"},
                {"type": "document", "source": {"type": "base64", "data": "pdfdata"}}
            ], "isMeta": true}),
            json!({"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "t1", "is_error": true, "content": [
                    {"type": "text", "text": "PDF too large (max 100 pages, 30MB). Try again."}
                ]}
            ]}),
            json!({"role": "assistant", "content": "I see the issue"}),
        ];
        normalize_messages_for_api(&mut messages);

        // Verify role alternation
        for i in 1..messages.len() {
            assert_ne!(
                messages[i]["role"].as_str(),
                messages[i - 1]["role"].as_str(),
                "Consecutive messages at index {} have same role",
                i
            );
        }

        // The document block should have been stripped from the meta user message
        let first_content = messages[0]["content"].as_array().unwrap();
        let has_document = first_content
            .iter()
            .any(|b| b["type"].as_str() == Some("document"));
        assert!(
            !has_document,
            "document block should have been stripped from meta user message"
        );
    }
}

// ---------------------------------------------------------------------------
// Tool input normalization (mirrors TS normalizeToolInputForAPI)
// ---------------------------------------------------------------------------

/// Normalize tool inputs before sending to the API.
///
/// Strips tool-specific synthetic fields that were injected by the runtime
/// but are not part of the tool's API schema.  Mirrors the TS reference's
/// `normalizeToolInputForAPI()` in `src/utils/api.ts`.
fn normalize_tool_input_for_api(tool_name: &str, input: Value) -> Value {
    let Some(obj) = input.as_object() else {
        return input;
    };

    match tool_name {
        "exit_plan_mode" => {
            if obj.contains_key("plan") || obj.contains_key("planFilePath") {
                let mut cleaned = obj.clone();
                cleaned.remove("plan");
                cleaned.remove("planFilePath");
                Value::Object(cleaned)
            } else {
                input
            }
        }
        "apply_diff" | "FileEdit" => {
            if obj.contains_key("edits") {
                let mut cleaned = obj.clone();
                cleaned.remove("old_string");
                cleaned.remove("new_string");
                cleaned.remove("replace_all");
                Value::Object(cleaned)
            } else {
                input
            }
        }
        _ => input,
    }
}

// ---------------------------------------------------------------------------
// Pass 5b — smooshSystemReminderSiblings
// ---------------------------------------------------------------------------

/// Merge `<system-reminder>` text blocks that are siblings of `tool_result`
/// blocks in the same user message into the last `tool_result`.
///
/// Mirrors the TS `smooshSystemReminderSiblings()`.  When a user message
/// contains both `tool_result` blocks and text blocks starting with
/// `<system-reminder>`, the system-reminder texts are appended to the last
/// tool_result's content.  This avoids the API interpreting system-reminder
/// text as a separate human turn sandwiched between tool results.
fn smoosh_system_reminder_siblings(messages: &mut [Value]) {
    for msg in messages.iter_mut() {
        if msg["role"].as_str() != Some("user") {
            continue;
        }
        let Some(content) = msg.get_mut("content").and_then(|c| c.as_array_mut()) else {
            continue;
        };

        let has_tool_result = content
            .iter()
            .any(|b| b["type"].as_str() == Some("tool_result"));
        if !has_tool_result {
            continue;
        }

        let has_system_reminder = content.iter().any(|b| {
            b["type"].as_str() == Some("text")
                && b.get("text")
                    .and_then(|t| t.as_str())
                    .is_some_and(|t| t.starts_with("<system-reminder>"))
        });
        if !has_system_reminder {
            continue;
        }

        let mut sr_texts: Vec<Value> = Vec::new();
        let mut kept: Vec<Value> = Vec::new();
        for block in content.drain(..) {
            if block["type"].as_str() == Some("text")
                && block
                    .get("text")
                    .and_then(|t| t.as_str())
                    .is_some_and(|t| t.starts_with("<system-reminder>"))
            {
                sr_texts.push(block);
            } else {
                kept.push(block);
            }
        }

        if sr_texts.is_empty() {
            *content = kept;
            continue;
        }

        let last_tr_idx = kept
            .iter()
            .rposition(|b| b["type"].as_str() == Some("tool_result"));
        let Some(tr_idx) = last_tr_idx else {
            kept.extend(sr_texts);
            *content = kept;
            continue;
        };

        // Guard: don't smoosh into tool_results that contain tool_reference blocks.
        // Mixing text with tool_reference inside a tool_result is a server ValueError.
        if let Some(tr_content) = kept[tr_idx].get("content").and_then(|c| c.as_array())
            && tr_content
                .iter()
                .any(|b| b["type"].as_str() == Some("tool_reference"))
        {
            kept.extend(sr_texts);
            *content = kept;
            continue;
        }

        if kept[tr_idx].get("is_error").and_then(|v| v.as_bool()) == Some(true) {
            kept.extend(sr_texts);
            *content = kept;
            continue;
        }

        let tr = &mut kept[tr_idx];
        match tr.get_mut("content").and_then(|c| c.as_array_mut()) {
            Some(inner) => {
                inner.extend(sr_texts);
            }
            None => {
                let text = tr
                    .get("content")
                    .and_then(|c| c.as_str())
                    .unwrap_or("")
                    .to_owned();
                let mut arr = vec![json!({"type": "text", "text": text})];
                arr.extend(sr_texts);
                tr["content"] = json!(arr);
            }
        }

        *content = kept;
    }
}
