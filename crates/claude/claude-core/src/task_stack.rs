//! Task stack for managing nested subtask delegation.
//!
//! [`TaskStack`] provides a LIFO stack of [`TaskFrame`] entries that track
//! parent tasks paused while child sub-agents execute. This enables
//! Roo-Code-style subtask delegation where a parent agent can spawn a child,
//! wait for it to complete, and then resume with the child's result.

use anyhow::{Result, anyhow};

use crate::{ConversationEntry, ToolCall};

/// Default maximum nesting depth for subtask delegation.
pub const DEFAULT_MAX_DEPTH: u32 = 3;

/// Current state of a task frame on the stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskFrameState {
    /// Task is actively running.
    Running,
    /// Task is paused waiting for a child to complete.
    PausedForChild,
    /// Task completed successfully.
    Completed,
    /// Task failed with an error.
    Failed,
}

/// A frame on the task stack representing a task (parent or child).
#[derive(Debug, Clone)]
pub struct TaskFrame {
    /// Unique identifier for this task.
    pub task_id: String,
    /// Conversation history snapshot at pause time.
    pub conversation_snapshot: Vec<ConversationEntry>,
    /// Tool calls that were pending when paused.
    pub pending_tool_calls: Vec<ToolCall>,
    /// Current depth in the delegation hierarchy (0 = root).
    pub depth: u32,
    /// Parent task ID (`None` for the root task).
    pub parent_task_id: Option<String>,
    /// Current state of this frame.
    pub state: TaskFrameState,
}

/// LIFO stack of task frames for nested delegation.
///
/// # Concurrency
///
/// `TaskStack` is **not** `Sync`. It is designed to be used within a single
/// conversation loop (single-threaded or wrapped in `Mutex` if shared).
///
/// # Example
///
/// ```
/// use claude_core::task_stack::{TaskStack, TaskFrame, TaskFrameState, DEFAULT_MAX_DEPTH};
///
/// let mut stack = TaskStack::new(DEFAULT_MAX_DEPTH);
/// assert!(stack.can_delegate());
/// assert_eq!(stack.depth(), 0);
/// ```
pub struct TaskStack {
    frames: Vec<TaskFrame>,
    max_depth: u32,
    next_id: u64,
}

impl TaskStack {
    /// Create a new task stack with the given maximum delegation depth.
    pub fn new(max_depth: u32) -> Self {
        Self {
            frames: Vec::new(),
            max_depth: if max_depth == 0 {
                DEFAULT_MAX_DEPTH
            } else {
                max_depth
            },
            next_id: 1,
        }
    }

    /// Generate a unique task ID.
    fn gen_id(&mut self) -> String {
        let id = self.next_id;
        self.next_id += 1;
        format!("task_{id}")
    }

    /// Push a new root task frame onto the stack.
    ///
    /// Returns the assigned task ID.
    pub fn push_root(&mut self, conversation: Vec<ConversationEntry>) -> String {
        let id = self.gen_id();
        self.frames.push(TaskFrame {
            task_id: id.clone(),
            conversation_snapshot: conversation,
            pending_tool_calls: Vec::new(),
            depth: 0,
            parent_task_id: None,
            state: TaskFrameState::Running,
        });
        id
    }

    /// Push a child task frame onto the stack.
    ///
    /// Returns an error if the maximum delegation depth would be exceeded.
    pub fn push_child(
        &mut self,
        conversation: Vec<ConversationEntry>,
        _allowed_tools: Vec<String>,
    ) -> Result<String> {
        let parent_depth = self.current().map(|f| f.depth).unwrap_or(0);
        let child_depth = parent_depth + 1;
        if child_depth >= self.max_depth {
            return Err(anyhow!(
                "Maximum delegation depth ({}) exceeded. Current depth: {}",
                self.max_depth,
                parent_depth
            ));
        }

        // Pause the current (parent) frame.
        if let Some(parent) = self.frames.last_mut() {
            parent.state = TaskFrameState::PausedForChild;
        }

        let parent_id = self.frames.last().map(|f| f.task_id.clone());
        let id = self.gen_id();
        self.frames.push(TaskFrame {
            task_id: id.clone(),
            conversation_snapshot: conversation,
            pending_tool_calls: Vec::new(),
            depth: child_depth,
            parent_task_id: parent_id,
            state: TaskFrameState::Running,
        });
        Ok(id)
    }

    /// Pop the top frame (child completed).
    ///
    /// Returns the completed child frame, or `None` if the stack is empty.
    pub fn pop(&mut self) -> Option<TaskFrame> {
        let frame = self.frames.pop();
        // Resume the new top frame.
        if let Some(parent) = self.frames.last_mut()
            && parent.state == TaskFrameState::PausedForChild
        {
            parent.state = TaskFrameState::Running;
        }
        frame
    }

    /// Peek at the current top frame.
    pub fn current(&self) -> Option<&TaskFrame> {
        self.frames.last()
    }

    /// Peek at the current top frame mutably.
    pub fn current_mut(&mut self) -> Option<&mut TaskFrame> {
        self.frames.last_mut()
    }

    /// Get the current delegation depth (number of frames on the stack).
    pub fn depth(&self) -> u32 {
        u32::try_from(self.frames.len()).unwrap_or(u32::MAX)
    }

    /// Check if further delegation is allowed at the current depth.
    pub fn can_delegate(&self) -> bool {
        self.depth() < self.max_depth
    }

    /// Get the maximum allowed delegation depth.
    pub fn max_depth(&self) -> u32 {
        self.max_depth
    }

    /// Check if the stack is empty.
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// Get the number of frames on the stack.
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    /// Resume the parent task after a child completes.
    ///
    /// Pops the child frame and returns it. The parent frame (now on top)
    /// transitions back to `Running`.
    pub fn resume_parent(&mut self) -> Result<TaskFrame> {
        let child = self
            .pop()
            .ok_or_else(|| anyhow!("No child task to resume from"))?;
        Ok(child)
    }

    /// Update the conversation snapshot of the current top frame.
    pub fn update_conversation(&mut self, conversation: Vec<ConversationEntry>) {
        if let Some(frame) = self.frames.last_mut() {
            frame.conversation_snapshot = conversation;
        }
    }

    /// Mark the current task as completed.
    pub fn mark_completed(&mut self) {
        if let Some(frame) = self.frames.last_mut() {
            frame.state = TaskFrameState::Completed;
        }
    }

    /// Mark the current task as failed.
    pub fn mark_failed(&mut self) {
        if let Some(frame) = self.frames.last_mut() {
            frame.state = TaskFrameState::Failed;
        }
    }
}

impl Default for TaskStack {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_DEPTH)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn sample_conversation() -> Vec<ConversationEntry> {
        vec![
            ConversationEntry::system("test system"),
            ConversationEntry::user("hello"),
        ]
    }

    #[test]
    fn task_stack_default_is_empty() {
        let stack = TaskStack::default();
        assert!(stack.is_empty());
        assert_eq!(stack.depth(), 0);
        assert!(stack.can_delegate());
    }

    #[test]
    fn task_stack_push_root() {
        let mut stack = TaskStack::new(3);
        let id = stack.push_root(sample_conversation());
        assert!(!stack.is_empty());
        assert_eq!(stack.depth(), 1);
        assert_eq!(stack.current().unwrap().task_id, id);
        assert_eq!(stack.current().unwrap().depth, 0);
        assert_eq!(stack.current().unwrap().parent_task_id, None);
    }

    #[test]
    fn task_stack_push_child_increments_depth() {
        let mut stack = TaskStack::new(3);
        stack.push_root(sample_conversation());
        let child_id = stack.push_child(sample_conversation(), vec![]).unwrap();

        // Stack has 2 frames: root (depth=0) + child (depth=1).
        assert_eq!(stack.depth(), 2);
        assert_eq!(stack.current().unwrap().task_id, child_id);
        assert_eq!(stack.current().unwrap().depth, 1);
        assert!(stack.current().unwrap().parent_task_id.is_some());
    }

    #[test]
    fn task_stack_max_depth_enforced() {
        let mut stack = TaskStack::new(2);
        stack.push_root(sample_conversation()); // depth 1
        stack.push_child(sample_conversation(), vec![]).unwrap(); // depth 2

        // Third level should fail.
        let result = stack.push_child(sample_conversation(), vec![]);
        assert!(result.is_err());
    }

    #[test]
    fn task_stack_pop_resumes_parent() {
        let mut stack = TaskStack::new(3);
        let root_id = stack.push_root(sample_conversation());
        let _child_id = stack.push_child(sample_conversation(), vec![]).unwrap();

        // Parent should be paused.
        assert_eq!(stack.frames[0].state, TaskFrameState::PausedForChild);

        let child = stack.pop().unwrap();
        assert_eq!(child.depth, 1);

        // After pop, only root remains.
        assert_eq!(stack.depth(), 1);
        assert_eq!(stack.current().unwrap().task_id, root_id);
        assert_eq!(stack.current().unwrap().state, TaskFrameState::Running);
    }

    #[test]
    fn task_stack_resume_parent() {
        let mut stack = TaskStack::new(3);
        stack.push_root(sample_conversation());
        stack.push_child(sample_conversation(), vec![]).unwrap();

        let child = stack.resume_parent().unwrap();
        assert_eq!(child.depth, 1);
        // After resume, only root remains.
        assert_eq!(stack.depth(), 1);
        assert_eq!(stack.current().unwrap().state, TaskFrameState::Running);
    }

    #[test]
    fn task_stack_resume_parent_empty_fails() {
        let mut stack = TaskStack::new(3);
        assert!(stack.resume_parent().is_err());
    }

    #[test]
    fn task_stack_nested_three_levels() {
        let mut stack = TaskStack::new(3);
        stack.push_root(sample_conversation()); // depth 1
        stack.push_child(sample_conversation(), vec![]).unwrap(); // depth 2
        stack.push_child(sample_conversation(), vec![]).unwrap(); // depth 3

        assert_eq!(stack.depth(), 3);
        assert!(!stack.can_delegate());

        // Pop back to root.
        stack.pop();
        stack.pop();
        assert_eq!(stack.depth(), 1);
        assert!(stack.can_delegate());
    }

    #[test]
    fn task_stack_mark_completed_and_failed() {
        let mut stack = TaskStack::new(3);
        stack.push_root(sample_conversation());

        stack.mark_completed();
        assert_eq!(stack.current().unwrap().state, TaskFrameState::Completed);

        stack.mark_failed();
        assert_eq!(stack.current().unwrap().state, TaskFrameState::Failed);
    }

    #[test]
    fn task_stack_update_conversation() {
        let mut stack = TaskStack::new(3);
        stack.push_root(sample_conversation());

        let new_conv = vec![ConversationEntry::user("updated")];
        stack.update_conversation(new_conv.clone());

        assert_eq!(stack.current().unwrap().conversation_snapshot.len(), 1);
        assert_eq!(
            stack.current().unwrap().conversation_snapshot[0].text,
            "updated"
        );
    }

    #[test]
    fn task_stack_zero_depth_uses_default() {
        let stack = TaskStack::new(0);
        assert_eq!(stack.max_depth(), DEFAULT_MAX_DEPTH);
    }
}
