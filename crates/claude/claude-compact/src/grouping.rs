//! Message grouping for compaction.
//!
//! Groups conversation messages into logical turns (user turn, assistant turn,
//! tool use, system) so that compaction strategies can operate on coherent
//! units rather than individual messages.
//!
//! Also provides [`group_messages_by_api_round`] which partitions messages at
//! boundaries where a new assistant message (identified by UUID) begins — used
//! by the PTL retry truncation logic.

use claude_core::Message;

// ---------------------------------------------------------------------------
// GroupType
// ---------------------------------------------------------------------------

/// Classification of a message group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GroupType {
    /// A user-initiated turn (user message + any follow-up).
    UserTurn,
    /// An assistant response turn.
    AssistantTurn,
    /// Tool-use messages (tool calls and results).
    ToolUse,
    /// System-level messages.
    System,
}

impl std::fmt::Display for GroupType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UserTurn => write!(f, "user_turn"),
            Self::AssistantTurn => write!(f, "assistant_turn"),
            Self::ToolUse => write!(f, "tool_use"),
            Self::System => write!(f, "system"),
        }
    }
}

// ---------------------------------------------------------------------------
// MessageGroup
// ---------------------------------------------------------------------------

/// A group of related messages forming a logical unit.
#[derive(Debug, Clone)]
pub struct MessageGroup {
    /// Messages in this group.
    pub messages: Vec<Message>,
    /// The type of this group.
    pub group_type: GroupType,
    /// Approximate token count for all messages in this group.
    pub token_count: u64,
}

impl MessageGroup {
    /// Create a new message group.
    #[must_use]
    pub fn new(group_type: GroupType) -> Self {
        Self {
            messages: Vec::new(),
            group_type,
            token_count: 0,
        }
    }

    /// Create a group with the given messages and token count.
    #[must_use]
    pub fn with_messages(messages: Vec<Message>, group_type: GroupType, token_count: u64) -> Self {
        Self {
            messages,
            group_type,
            token_count,
        }
    }

    /// Number of messages in the group.
    #[must_use]
    pub fn len(&self) -> usize {
        self.messages.len()
    }

    /// Whether the group is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    /// Add a message to the group, updating the token count.
    pub fn push(&mut self, message: Message, tokens: u64) {
        self.token_count += tokens;
        self.messages.push(message);
    }
}

// ---------------------------------------------------------------------------
// group_messages
// ---------------------------------------------------------------------------

/// Classify a message into a [`GroupType`] based on its variant.
fn classify_message(message: &Message) -> GroupType {
    match message {
        Message::User(_) => GroupType::UserTurn,
        Message::Assistant(_) => GroupType::AssistantTurn,
        Message::System(_) => GroupType::System,
        Message::Progress(_) => GroupType::AssistantTurn,
        Message::Attachment(_) => GroupType::UserTurn,
        Message::HookResult(_) => GroupType::System,
        Message::CollapsedReadSearch(_) => GroupType::ToolUse,
        Message::ToolUseSummary(_) => GroupType::ToolUse,
        Message::GroupedToolUse(_) => GroupType::ToolUse,
        Message::Tombstone(_) => GroupType::System,
    }
}

/// Rough token count estimate for a message (characters / 4).
fn rough_tokens_for_message(message: &Message) -> u64 {
    let text = format!("{message:?}");
    let len = text.len() as u64;
    if len == 0 { 0 } else { len.div_ceil(4) }
}

/// Group messages into logical turns.
///
/// Consecutive messages of the same type are merged into a single group.
/// Returns groups in the same order as the input messages.
pub fn group_messages(messages: &[Message]) -> Vec<MessageGroup> {
    if messages.is_empty() {
        return Vec::new();
    }

    let mut groups: Vec<MessageGroup> = Vec::new();
    let mut current_group = MessageGroup::new(classify_message(&messages[0]));

    for message in messages {
        let msg_type = classify_message(message);
        let tokens = rough_tokens_for_message(message);

        if msg_type == current_group.group_type {
            current_group.push(message.clone(), tokens);
        } else {
            if !current_group.is_empty() {
                groups.push(current_group);
            }
            current_group = MessageGroup::new(msg_type);
            current_group.push(message.clone(), tokens);
        }
    }

    if !current_group.is_empty() {
        groups.push(current_group);
    }

    groups
}

// ---------------------------------------------------------------------------
// merge_small_groups
// ---------------------------------------------------------------------------

/// Merge groups that are smaller than `min_tokens` into their neighbours.
///
/// Groups below the token threshold are absorbed into the adjacent group
/// (preferring the previous group, falling back to the next).
pub fn merge_small_groups(groups: &[MessageGroup], min_tokens: u64) -> Vec<MessageGroup> {
    if groups.is_empty() {
        return Vec::new();
    }

    // First pass: identify which groups are "small"
    let mut result: Vec<Option<MessageGroup>> = groups.iter().map(|g| Some(g.clone())).collect();

    for i in 0..result.len() {
        let is_small = result[i]
            .as_ref()
            .is_some_and(|g| g.token_count < min_tokens);
        if !is_small {
            continue;
        }

        // Try to merge into previous group
        if i > 0 && result[i - 1].is_some() {
            let small = result[i].take().expect("just checked");
            let prev = result[i - 1].as_mut().expect("just checked");
            prev.messages.extend(small.messages);
            prev.token_count += small.token_count;
        }
        // Otherwise try next group
        else if i + 1 < result.len() && result[i + 1].is_some() {
            let small = result[i].take().expect("just checked");
            let next = result[i + 1].as_mut().expect("just checked");
            next.messages.extend(small.messages);
            next.token_count += small.token_count;
        }
        // If neither neighbour exists, keep as-is
        else {
            result[i] = Some(result[i].take().expect("just checked"));
        }
    }

    result.into_iter().flatten().collect()
}

// ---------------------------------------------------------------------------
// API-round grouping (for PTL retry)
// ---------------------------------------------------------------------------

/// Group messages by API round, splitting at boundaries where a new assistant
/// message (identified by a different UUID) begins.
///
/// Mirrors `groupMessagesByApiRound()` from the TypeScript reference
/// (`services/compact/grouping.ts`).
///
/// Each group represents one API request/response cycle: the user messages,
/// assistant response, and any tool-use/result pairs that belong to that round.
pub fn group_messages_by_api_round(messages: &[Message]) -> Vec<Vec<Message>> {
    if messages.is_empty() {
        return Vec::new();
    }

    let mut groups: Vec<Vec<Message>> = Vec::new();
    let mut current: Vec<Message> = Vec::new();
    let mut last_assistant_uuid: Option<uuid::Uuid> = None;

    for msg in messages {
        let is_new_assistant_round = matches!(msg, Message::Assistant(_))
            && Some(msg.uuid()) != last_assistant_uuid
            && !current.is_empty();

        if is_new_assistant_round {
            groups.push(current);
            current = vec![msg.clone()];
        } else {
            current.push(msg.clone());
        }

        if matches!(msg, Message::Assistant(_)) {
            last_assistant_uuid = Some(msg.uuid());
        }
    }

    if !current.is_empty() {
        groups.push(current);
    }

    groups
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use claude_core::{MessageBase, UserMessage};

    fn make_user_message(text: &str) -> Message {
        Message::User(UserMessage {
            base: MessageBase::default(),
            text: text.to_string(),
            attachments: Vec::new(),
            provider_content_blocks: Vec::new(),
            summarize_metadata: None,
        })
    }

    fn make_system_message(text: &str) -> Message {
        Message::System(claude_core::SystemMessage {
            base: MessageBase::default(),
            subtype: claude_core::SystemMessageSubtype::Informational,
            text: text.to_string(),
            error: None,
        })
    }

    // -- GroupType ------------------------------------------------------------

    #[test]
    fn group_type_display() {
        assert_eq!(GroupType::UserTurn.to_string(), "user_turn");
        assert_eq!(GroupType::AssistantTurn.to_string(), "assistant_turn");
        assert_eq!(GroupType::ToolUse.to_string(), "tool_use");
        assert_eq!(GroupType::System.to_string(), "system");
    }

    // -- MessageGroup ---------------------------------------------------------

    #[test]
    fn message_group_new() {
        let g = MessageGroup::new(GroupType::UserTurn);
        assert!(g.is_empty());
        assert_eq!(g.group_type, GroupType::UserTurn);
        assert_eq!(g.token_count, 0);
    }

    #[test]
    fn message_group_push() {
        let mut g = MessageGroup::new(GroupType::UserTurn);
        let msg = make_user_message("hello");
        g.push(msg, 5);
        assert_eq!(g.len(), 1);
        assert_eq!(g.token_count, 5);
    }

    #[test]
    fn message_group_with_messages() {
        let msgs = vec![make_user_message("a"), make_user_message("b")];
        let g = MessageGroup::with_messages(msgs, GroupType::UserTurn, 10);
        assert_eq!(g.len(), 2);
        assert_eq!(g.token_count, 10);
    }

    // -- group_messages -------------------------------------------------------

    #[test]
    fn group_messages_empty() {
        let groups = group_messages(&[]);
        assert!(groups.is_empty());
    }

    #[test]
    fn group_messages_single() {
        let msgs = vec![make_user_message("hello")];
        let groups = group_messages(&msgs);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].group_type, GroupType::UserTurn);
        assert_eq!(groups[0].len(), 1);
    }

    #[test]
    fn group_messages_same_type_merged() {
        let msgs = vec![make_user_message("hello"), make_user_message("world")];
        let groups = group_messages(&msgs);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].len(), 2);
    }

    #[test]
    fn group_messages_different_types_separate() {
        let msgs = vec![
            make_user_message("hello"),
            make_system_message("system msg"),
        ];
        let groups = group_messages(&msgs);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].group_type, GroupType::UserTurn);
        assert_eq!(groups[1].group_type, GroupType::System);
    }

    #[test]
    fn group_messages_preserves_order() {
        let msgs = vec![
            make_user_message("u1"),
            make_system_message("s1"),
            make_user_message("u2"),
        ];
        let groups = group_messages(&msgs);
        assert_eq!(groups.len(), 3);
        assert_eq!(groups[0].group_type, GroupType::UserTurn);
        assert_eq!(groups[1].group_type, GroupType::System);
        assert_eq!(groups[2].group_type, GroupType::UserTurn);
    }

    #[test]
    fn group_messages_token_count_nonzero() {
        let msgs = vec![make_user_message("hello world this is a test")];
        let groups = group_messages(&msgs);
        assert!(groups[0].token_count > 0);
    }

    // -- merge_small_groups ---------------------------------------------------

    #[test]
    fn merge_small_groups_empty() {
        let result = merge_small_groups(&[], 100);
        assert!(result.is_empty());
    }

    #[test]
    fn merge_small_groups_no_small() {
        let groups = vec![MessageGroup::with_messages(
            vec![make_user_message(&"a".repeat(500))],
            GroupType::UserTurn,
            200,
        )];
        let result = merge_small_groups(&groups, 100);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn merge_small_groups_merges_into_previous() {
        let groups = vec![
            MessageGroup::with_messages(
                vec![make_user_message("big message")],
                GroupType::UserTurn,
                200,
            ),
            MessageGroup::with_messages(vec![make_user_message("tiny")], GroupType::System, 5),
        ];
        let result = merge_small_groups(&groups, 100);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].len(), 2);
    }

    #[test]
    fn merge_small_groups_merges_into_next_when_no_prev() {
        let groups = vec![
            MessageGroup::with_messages(vec![make_user_message("tiny")], GroupType::System, 5),
            MessageGroup::with_messages(
                vec![make_user_message("big message")],
                GroupType::UserTurn,
                200,
            ),
        ];
        let result = merge_small_groups(&groups, 100);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn merge_small_groups_keeps_isolated_small() {
        let groups = vec![MessageGroup::with_messages(
            vec![make_user_message("tiny")],
            GroupType::UserTurn,
            5,
        )];
        let result = merge_small_groups(&groups, 100);
        assert_eq!(result.len(), 1);
    }

    // -- classify_message -----------------------------------------------------

    #[test]
    fn classify_user_message() {
        let msg = make_user_message("hello");
        assert_eq!(classify_message(&msg), GroupType::UserTurn);
    }

    #[test]
    fn classify_system_message() {
        let msg = make_system_message("system");
        assert_eq!(classify_message(&msg), GroupType::System);
    }

    // -- rough_tokens_for_message ---------------------------------------------

    #[test]
    fn rough_tokens_nonzero() {
        let msg = make_user_message("hello world");
        let tokens = rough_tokens_for_message(&msg);
        assert!(tokens > 0);
    }

    // -- group_messages_by_api_round --------------------------------------------

    #[test]
    fn api_round_groups_empty() {
        let groups = group_messages_by_api_round(&[]);
        assert!(groups.is_empty());
    }

    #[test]
    fn api_round_groups_single_user() {
        let msgs = vec![make_user_message("hello")];
        let groups = group_messages_by_api_round(&msgs);
        assert_eq!(groups.len(), 1);
    }

    #[test]
    fn api_round_groups_splits_on_new_assistant_uuid() {
        // Each assistant message gets a unique UUID by default
        let a1 = Message::Assistant(claude_core::AssistantMessage {
            base: MessageBase::default(),
            text: "first response".into(),
            blocks: Vec::new(),
            tool_calls: Vec::new(),
            provider_content_blocks: Vec::new(),
        });
        let a2 = Message::Assistant(claude_core::AssistantMessage {
            base: MessageBase::default(),
            text: "second response".into(),
            blocks: Vec::new(),
            tool_calls: Vec::new(),
            provider_content_blocks: Vec::new(),
        });
        let msgs = vec![
            make_user_message("q1"),
            a1,
            make_user_message("tool_result_1"),
            make_user_message("q2"),
            a2,
            make_user_message("tool_result_2"),
        ];
        let groups = group_messages_by_api_round(&msgs);
        // Group 0: [user("q1")]
        // Group 1: [assistant(a1), user("tool_result_1"), user("q2")]
        // Group 2: [assistant(a2), user("tool_result_2")]
        assert_eq!(groups.len(), 3);
        assert_eq!(groups[0].len(), 1);
        assert_eq!(groups[1].len(), 3);
        assert_eq!(groups[2].len(), 2);
    }

    #[test]
    fn api_round_groups_same_assistant_id_not_split() {
        let base = MessageBase::default();
        let shared_uuid = base.uuid;
        let a1 = Message::Assistant(claude_core::AssistantMessage {
            base: base.clone(),
            text: "chunk1".into(),
            blocks: Vec::new(),
            tool_calls: Vec::new(),
            provider_content_blocks: Vec::new(),
        });
        let base2 = MessageBase {
            uuid: shared_uuid,
            ..Default::default()
        };
        let a2 = Message::Assistant(claude_core::AssistantMessage {
            base: base2,
            text: "chunk2".into(),
            blocks: Vec::new(),
            tool_calls: Vec::new(),
            provider_content_blocks: Vec::new(),
        });
        let msgs = vec![make_user_message("q"), a1, make_user_message("result"), a2];
        let groups = group_messages_by_api_round(&msgs);
        // Same UUID → no split, all in one group after the initial group
        assert_eq!(groups.len(), 2); // [user("q")], [a1, user("result"), a2]
    }
}
