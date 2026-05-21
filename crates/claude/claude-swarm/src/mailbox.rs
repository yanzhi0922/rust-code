//! File-system-based agent mailbox.
//!
//! Agents communicate by writing messages to each other's mailbox
//! directories on the file system.
//!
//! Directory structure:
//! ```text
//! ~/.remote-code/teams/<team>/mailbox/<agent>/
//!   <message_id>.msg.json
//! ```

use std::path::PathBuf;

use tokio::fs;

use crate::constants::{MAILBOX_DIR_NAME, MAILBOX_MESSAGE_EXT};
use crate::error::{SwarmError, SwarmResult};
use crate::team_helpers::team_dir;
use crate::types::{MailboxMessage, MailboxMessageType};

/// Get the mailbox directory for a team.
fn mailbox_dir(team_name: &str) -> PathBuf {
    team_dir(team_name).join(MAILBOX_DIR_NAME)
}

/// Get the mailbox directory for a specific agent.
fn agent_mailbox_dir(team_name: &str, agent_name: &str) -> PathBuf {
    mailbox_dir(team_name).join(agent_name)
}

/// Get the file path for a specific message.
fn message_file_path(team_name: &str, agent_name: &str, message_id: &str) -> PathBuf {
    agent_mailbox_dir(team_name, agent_name).join(format!("{}{}", message_id, MAILBOX_MESSAGE_EXT))
}

/// Send a message to an agent's mailbox.
pub async fn send_message(team_name: &str, message: &MailboxMessage) -> SwarmResult<()> {
    let dir = agent_mailbox_dir(team_name, &message.to_agent);
    fs::create_dir_all(&dir).await?;
    let path = message_file_path(team_name, &message.to_agent, &message.id);
    let json = serde_json::to_string_pretty(message)?;
    fs::write(&path, json).await?;
    Ok(())
}

/// Read all messages from an agent's mailbox.
pub async fn read_messages(team_name: &str, agent_name: &str) -> SwarmResult<Vec<MailboxMessage>> {
    let dir = agent_mailbox_dir(team_name, agent_name);
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut entries = fs::read_dir(&dir).await?;
    let mut messages = Vec::new();

    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if let Some(name) = path.file_name().map(|n| n.to_string_lossy().to_string())
            && name.ends_with(MAILBOX_MESSAGE_EXT)
        {
            let content = fs::read_to_string(&path).await?;
            let msg: MailboxMessage = serde_json::from_str(&content)?;
            messages.push(msg);
        }
    }

    // Sort by timestamp.
    messages.sort_by_key(|m| m.timestamp);
    Ok(messages)
}

/// Read only unread messages from an agent's mailbox.
pub async fn read_unread_messages(
    team_name: &str,
    agent_name: &str,
) -> SwarmResult<Vec<MailboxMessage>> {
    let messages = read_messages(team_name, agent_name).await?;
    Ok(messages.into_iter().filter(|m| !m.read).collect())
}

/// Mark a message as read.
pub async fn mark_message_read(
    team_name: &str,
    agent_name: &str,
    message_id: &str,
) -> SwarmResult<()> {
    let path = message_file_path(team_name, agent_name, message_id);
    if !path.exists() {
        return Err(SwarmError::MailboxError {
            agent_name: agent_name.to_owned(),
            reason: format!("message {message_id} not found"),
        });
    }
    let content = fs::read_to_string(&path).await?;
    let mut msg: MailboxMessage = serde_json::from_str(&content)?;
    msg.mark_read();
    let json = serde_json::to_string_pretty(&msg)?;
    fs::write(&path, json).await?;
    Ok(())
}

/// Delete a message from an agent's mailbox.
pub async fn delete_message(
    team_name: &str,
    agent_name: &str,
    message_id: &str,
) -> SwarmResult<()> {
    let path = message_file_path(team_name, agent_name, message_id);
    if !path.exists() {
        return Err(SwarmError::MailboxError {
            agent_name: agent_name.to_owned(),
            reason: format!("message {message_id} not found"),
        });
    }
    fs::remove_file(&path).await?;
    Ok(())
}

/// Clear all messages from an agent's mailbox.
pub async fn clear_mailbox(team_name: &str, agent_name: &str) -> SwarmResult<usize> {
    let dir = agent_mailbox_dir(team_name, agent_name);
    if !dir.exists() {
        return Ok(0);
    }

    let mut entries = fs::read_dir(&dir).await?;
    let mut removed = 0;

    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path
            .file_name()
            .map(|n| n.to_string_lossy().ends_with(MAILBOX_MESSAGE_EXT))
            .expect("should have filename")
        {
            fs::remove_file(&path).await?;
            removed += 1;
        }
    }

    Ok(removed)
}

/// Count unread messages for an agent.
pub async fn count_unread(team_name: &str, agent_name: &str) -> SwarmResult<usize> {
    let unread = read_unread_messages(team_name, agent_name).await?;
    Ok(unread.len())
}

/// Send a broadcast message to all team members.
pub async fn broadcast_message(
    team_name: &str,
    from_agent: &str,
    recipients: &[String],
    message_type: MailboxMessageType,
    content: &str,
) -> SwarmResult<Vec<MailboxMessage>> {
    let mut messages = Vec::new();
    for recipient in recipients {
        let msg = MailboxMessage::new(from_agent, recipient, message_type, content);
        send_message(team_name, &msg).await?;
        messages.push(msg);
    }
    Ok(messages)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::team_helpers::set_base_dir_override;

    struct TestDir {
        _temp: tempfile::TempDir,
    }

    impl TestDir {
        fn new() -> Self {
            let temp = tempfile::tempdir().expect("tempdir");
            let path = temp.path().to_path_buf();
            set_base_dir_override(Some(path));
            Self { _temp: temp }
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            set_base_dir_override(None);
        }
    }

    #[tokio::test]
    async fn test_send_and_read_message() {
        let _td = TestDir::new();
        let msg = MailboxMessage::new("lead", "worker-1", MailboxMessageType::Text, "hello");
        send_message("test-team", &msg).await.expect("should send");

        let messages = read_messages("test-team", "worker-1")
            .await
            .expect("should read");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content, "hello");
        assert_eq!(messages[0].from_agent, "lead");
    }

    #[tokio::test]
    async fn test_read_messages_empty() {
        let _td = TestDir::new();
        let messages = read_messages("test-team", "worker-1")
            .await
            .expect("should read");
        assert!(messages.is_empty());
    }

    #[tokio::test]
    async fn test_read_unread_messages() {
        let _td = TestDir::new();
        let msg1 = MailboxMessage::new("lead", "worker-1", MailboxMessageType::Text, "hello");
        let msg2 = MailboxMessage::new(
            "lead",
            "worker-1",
            MailboxMessageType::TaskAssignment,
            "task",
        );
        send_message("test-team", &msg1).await.expect("ok");
        send_message("test-team", &msg2).await.expect("ok");

        let unread = read_unread_messages("test-team", "worker-1")
            .await
            .expect("should read");
        assert_eq!(unread.len(), 2);
    }

    #[tokio::test]
    async fn test_mark_message_read() {
        let _td = TestDir::new();
        let msg = MailboxMessage::new("lead", "worker-1", MailboxMessageType::Text, "hello");
        send_message("test-team", &msg).await.expect("ok");
        mark_message_read("test-team", "worker-1", &msg.id)
            .await
            .expect("should mark");

        let unread = read_unread_messages("test-team", "worker-1")
            .await
            .expect("ok");
        assert!(unread.is_empty());
    }

    #[tokio::test]
    async fn test_mark_nonexistent_message() {
        let _td = TestDir::new();
        let result = mark_message_read("test-team", "worker-1", "nonexistent").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_delete_message() {
        let _td = TestDir::new();
        let msg = MailboxMessage::new("lead", "worker-1", MailboxMessageType::Text, "hello");
        send_message("test-team", &msg).await.expect("ok");
        delete_message("test-team", "worker-1", &msg.id)
            .await
            .expect("should delete");

        let messages = read_messages("test-team", "worker-1").await.expect("ok");
        assert!(messages.is_empty());
    }

    #[tokio::test]
    async fn test_delete_nonexistent_message() {
        let _td = TestDir::new();
        let result = delete_message("test-team", "worker-1", "nonexistent").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_clear_mailbox() {
        let _td = TestDir::new();
        let msg1 = MailboxMessage::new("lead", "worker-1", MailboxMessageType::Text, "hello");
        let msg2 = MailboxMessage::new("lead", "worker-1", MailboxMessageType::Text, "world");
        send_message("test-team", &msg1).await.expect("ok");
        send_message("test-team", &msg2).await.expect("ok");

        let removed = clear_mailbox("test-team", "worker-1")
            .await
            .expect("should clear");
        assert_eq!(removed, 2);

        let messages = read_messages("test-team", "worker-1").await.expect("ok");
        assert!(messages.is_empty());
    }

    #[tokio::test]
    async fn test_clear_empty_mailbox() {
        let _td = TestDir::new();
        let removed = clear_mailbox("test-team", "worker-1").await.expect("ok");
        assert_eq!(removed, 0);
    }

    #[tokio::test]
    async fn test_count_unread_messages() {
        let _td = TestDir::new();
        let msg1 = MailboxMessage::new("lead", "worker-1", MailboxMessageType::Text, "hello");
        let msg2 = MailboxMessage::new("lead", "worker-1", MailboxMessageType::Text, "world");
        send_message("test-team", &msg1).await.expect("ok");
        send_message("test-team", &msg2).await.expect("ok");

        let count = count_unread("test-team", "worker-1")
            .await
            .expect("should count");
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn test_broadcast_message() {
        let _td = TestDir::new();
        let recipients = vec![
            "worker-1".to_owned(),
            "worker-2".to_owned(),
            "worker-3".to_owned(),
        ];
        let messages = broadcast_message(
            "test-team",
            "lead",
            &recipients,
            MailboxMessageType::Coordination,
            "sync up",
        )
        .await
        .expect("should broadcast");

        assert_eq!(messages.len(), 3);

        for recipient in &recipients {
            let msgs = read_messages("test-team", recipient)
                .await
                .expect("should read");
            assert_eq!(msgs.len(), 1);
            assert_eq!(msgs[0].content, "sync up");
        }
    }

    #[tokio::test]
    async fn test_messages_sorted_by_timestamp() {
        let _td = TestDir::new();
        let msg1 = {
            let mut m = MailboxMessage::new("lead", "worker-1", MailboxMessageType::Text, "first");
            m.timestamp = 100;
            m
        };
        let msg2 = {
            let mut m = MailboxMessage::new("lead", "worker-1", MailboxMessageType::Text, "second");
            m.timestamp = 200;
            m
        };
        // Write in reverse order.
        send_message("test-team", &msg2).await.expect("ok");
        send_message("test-team", &msg1).await.expect("ok");

        let messages = read_messages("test-team", "worker-1").await.expect("ok");
        assert_eq!(messages[0].content, "first");
        assert_eq!(messages[1].content, "second");
    }

    #[test]
    fn mailbox_dir_contains_name() {
        let _td = TestDir::new();
        let dir = mailbox_dir("my-team");
        assert!(dir.to_string_lossy().contains("mailbox"));
    }

    #[test]
    fn message_file_path_format() {
        let _td = TestDir::new();
        let path = message_file_path("my-team", "worker-1", "msg-123");
        assert!(path.to_string_lossy().ends_with("msg-123.msg.json"));
    }
}
