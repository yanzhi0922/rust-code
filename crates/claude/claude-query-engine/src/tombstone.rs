//! Tombstone handling for orphaned messages during streaming fallback.
//!
//! When a streaming response is interrupted or falls back, some messages
//! may become orphaned. The tombstone manager tracks these IDs so they
//! can be cleaned up or ignored during subsequent processing.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

/// Manages tombstone markers for orphaned or invalidated messages.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TombstoneManager {
    /// Set of message IDs that have been tombstoned.
    orphaned_ids: HashSet<String>,
}

impl TombstoneManager {
    /// Create a new empty tombstone manager.
    #[must_use]
    pub fn new() -> Self {
        Self {
            orphaned_ids: HashSet::new(),
        }
    }

    /// Mark a message ID as orphaned (tombstoned).
    pub fn mark_orphaned(&mut self, message_id: impl Into<String>) {
        self.orphaned_ids.insert(message_id.into());
    }

    /// Mark multiple message IDs as orphaned.
    pub fn mark_many_orphaned(&mut self, ids: impl IntoIterator<Item = String>) {
        self.orphaned_ids.extend(ids);
    }

    /// Check if a message ID is tombstoned.
    #[must_use]
    pub fn is_orphaned(&self, message_id: &str) -> bool {
        self.orphaned_ids.contains(message_id)
    }

    /// Remove a message ID from the tombstone set (e.g., after recovery).
    pub fn unmark(&mut self, message_id: &str) -> bool {
        self.orphaned_ids.remove(message_id)
    }

    /// Returns the number of tombstoned message IDs.
    #[must_use]
    pub fn count(&self) -> usize {
        self.orphaned_ids.len()
    }

    /// Returns true if there are no tombstoned IDs.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.orphaned_ids.is_empty()
    }

    /// Clear all tombstoned IDs.
    pub fn clear(&mut self) {
        self.orphaned_ids.clear();
    }

    /// Filter a list of message IDs, returning only non-orphaned ones.
    #[must_use]
    pub fn filter_orphaned<'a>(&self, ids: &'a [String]) -> Vec<&'a String> {
        ids.iter()
            .filter(|id| !self.orphaned_ids.contains(id.as_str()))
            .collect()
    }

    /// Returns all tombstoned IDs.
    #[must_use]
    pub fn orphaned_ids(&self) -> &HashSet<String> {
        &self.orphaned_ids
    }
}

#[cfg(test)]
mod tests {
    use super::TombstoneManager;

    #[test]
    fn tombstone_manager_marks_and_checks() {
        let mut mgr = TombstoneManager::new();
        assert!(!mgr.is_orphaned("msg-1"));
        mgr.mark_orphaned("msg-1");
        assert!(mgr.is_orphaned("msg-1"));
        assert!(!mgr.is_orphaned("msg-2"));
    }

    #[test]
    fn tombstone_manager_unmark() {
        let mut mgr = TombstoneManager::new();
        mgr.mark_orphaned("msg-1");
        assert!(mgr.unmark("msg-1"));
        assert!(!mgr.is_orphaned("msg-1"));
        assert!(!mgr.unmark("nonexistent"));
    }

    #[test]
    fn tombstone_manager_mark_many() {
        let mut mgr = TombstoneManager::new();
        mgr.mark_many_orphaned(vec!["a".to_string(), "b".to_string(), "c".to_string()]);
        assert_eq!(mgr.count(), 3);
    }

    #[test]
    fn tombstone_manager_filter_orphaned() {
        let mut mgr = TombstoneManager::new();
        mgr.mark_orphaned("b");
        let ids = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let filtered = mgr.filter_orphaned(&ids);
        assert_eq!(filtered.len(), 2);
    }

    #[test]
    fn tombstone_manager_clear() {
        let mut mgr = TombstoneManager::new();
        mgr.mark_orphaned("msg-1");
        mgr.mark_orphaned("msg-2");
        assert_eq!(mgr.count(), 2);
        mgr.clear();
        assert!(mgr.is_empty());
    }

    #[test]
    fn tombstone_manager_default_is_empty() {
        let mgr = TombstoneManager::default();
        assert!(mgr.is_empty());
        assert_eq!(mgr.count(), 0);
    }
}
