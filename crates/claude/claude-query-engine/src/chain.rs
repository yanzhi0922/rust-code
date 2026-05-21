//! Query chain tracking for nested/recursive query execution.
//!
//! Each query submission creates a chain context. Sub-queries (e.g. from
//! agent delegation) create child chains with incremented depth.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::config::QuerySource;

/// Tracks the chain context for a query execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryChain {
    /// Unique identifier for this chain.
    pub chain_id: String,
    /// Nesting depth (0 = top-level query, 1+ = sub-query).
    pub depth: u32,
    /// Parent chain ID if this is a sub-query.
    pub parent_chain_id: Option<String>,
    /// What initiated this query.
    pub query_source: QuerySource,
}

impl QueryChain {
    /// Create a new top-level chain (depth 0).
    #[must_use]
    pub fn new_root(query_source: QuerySource) -> Self {
        Self {
            chain_id: Uuid::new_v4().to_string(),
            depth: 0,
            parent_chain_id: None,
            query_source,
        }
    }

    /// Create a child chain from this parent, incrementing depth.
    #[must_use]
    pub fn create_child(&self, query_source: QuerySource) -> Self {
        Self {
            chain_id: Uuid::new_v4().to_string(),
            depth: self.depth.saturating_add(1),
            parent_chain_id: Some(self.chain_id.clone()),
            query_source,
        }
    }

    /// Returns true if this is a top-level chain (depth 0, no parent).
    #[must_use]
    pub fn is_root(&self) -> bool {
        self.depth == 0 && self.parent_chain_id.is_none()
    }

    /// Returns true if this is a nested sub-query.
    #[must_use]
    pub fn is_nested(&self) -> bool {
        self.depth > 0
    }

    /// Returns the chain ID.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.chain_id
    }

    /// Returns the depth of nesting.
    #[must_use]
    pub fn depth(&self) -> u32 {
        self.depth
    }
}

/// Manages active query chains and enforces depth limits.
#[derive(Debug, Clone)]
pub struct ChainManager {
    /// Maximum allowed nesting depth.
    max_depth: u32,
    /// Currently active chains.
    active_chains: Vec<QueryChain>,
}

impl ChainManager {
    /// Create a new chain manager with the given maximum nesting depth.
    #[must_use]
    pub fn new(max_depth: u32) -> Self {
        Self {
            max_depth,
            active_chains: Vec::new(),
        }
    }

    /// Returns the maximum allowed nesting depth.
    #[must_use]
    pub fn max_depth(&self) -> u32 {
        self.max_depth
    }

    /// Start a new root chain. Returns the created chain.
    pub fn start_root(&mut self, query_source: QuerySource) -> QueryChain {
        let chain = QueryChain::new_root(query_source);
        self.active_chains.push(chain.clone());
        chain
    }

    /// Start a child chain from the given parent. Returns an error if
    /// the maximum depth would be exceeded.
    pub fn start_child(
        &mut self,
        parent: &QueryChain,
        query_source: QuerySource,
    ) -> Result<QueryChain, ChainError> {
        let child = parent.create_child(query_source);
        if child.depth > self.max_depth {
            return Err(ChainError::DepthExceeded {
                depth: child.depth,
                max_depth: self.max_depth,
            });
        }
        self.active_chains.push(child.clone());
        Ok(child)
    }

    /// End a chain by its ID. Returns true if the chain was found and removed.
    pub fn end_chain(&mut self, chain_id: &str) -> bool {
        let before = self.active_chains.len();
        self.active_chains.retain(|c| c.chain_id != chain_id);
        self.active_chains.len() < before
    }

    /// Returns the number of currently active chains.
    #[must_use]
    pub fn active_count(&self) -> usize {
        self.active_chains.len()
    }

    /// Find a chain by its ID.
    #[must_use]
    pub fn find_chain(&self, chain_id: &str) -> Option<&QueryChain> {
        self.active_chains.iter().find(|c| c.chain_id == chain_id)
    }

    /// Returns all active chains.
    #[must_use]
    pub fn active_chains(&self) -> &[QueryChain] {
        &self.active_chains
    }

    /// Clear all active chains.
    pub fn clear(&mut self) {
        self.active_chains.clear();
    }
}

impl Default for ChainManager {
    fn default() -> Self {
        Self::new(4)
    }
}

/// Errors from chain operations.
#[derive(Debug, Clone, thiserror::Error)]
pub enum ChainError {
    #[error("chain depth {depth} exceeds maximum {max_depth}")]
    DepthExceeded { depth: u32, max_depth: u32 },
}

#[cfg(test)]
mod tests {
    use super::{ChainError, ChainManager, QueryChain};
    use crate::config::QuerySource;

    #[test]
    fn root_chain_has_depth_zero() {
        let chain = QueryChain::new_root(QuerySource::User);
        assert_eq!(chain.depth(), 0);
        assert!(chain.is_root());
        assert!(!chain.is_nested());
        assert!(chain.parent_chain_id.is_none());
    }

    #[test]
    fn child_chain_increments_depth() {
        let root = QueryChain::new_root(QuerySource::User);
        let child = root.create_child(QuerySource::Agent);
        assert_eq!(child.depth(), 1);
        assert!(!child.is_root());
        assert!(child.is_nested());
        assert_eq!(child.parent_chain_id.as_deref(), Some(root.id()));
    }

    #[test]
    fn chain_manager_enforces_depth_limit() {
        let mut mgr = ChainManager::new(2);
        let root = mgr.start_root(QuerySource::User);
        let child = mgr
            .start_child(&root, QuerySource::Agent)
            .expect("depth 1 ok");
        let grandchild = mgr
            .start_child(&child, QuerySource::Agent)
            .expect("depth 2 ok");
        let result = mgr.start_child(&grandchild, QuerySource::Agent);
        assert!(matches!(
            result,
            Err(ChainError::DepthExceeded {
                depth: 3,
                max_depth: 2
            })
        ));
    }

    #[test]
    fn chain_manager_tracks_active_chains() {
        let mut mgr = ChainManager::new(4);
        let root = mgr.start_root(QuerySource::User);
        assert_eq!(mgr.active_count(), 1);
        let child = mgr.start_child(&root, QuerySource::Agent).expect("child");
        assert_eq!(mgr.active_count(), 2);
        assert!(mgr.end_chain(child.id()));
        assert_eq!(mgr.active_count(), 1);
    }

    #[test]
    fn chain_manager_find_chain() {
        let mut mgr = ChainManager::new(4);
        let root = mgr.start_root(QuerySource::Compact);
        assert!(mgr.find_chain(root.id()).is_some());
        assert!(mgr.find_chain("nonexistent").is_none());
    }

    #[test]
    fn chain_manager_clear_removes_all() {
        let mut mgr = ChainManager::new(4);
        mgr.start_root(QuerySource::User);
        mgr.start_root(QuerySource::Agent);
        assert_eq!(mgr.active_count(), 2);
        mgr.clear();
        assert_eq!(mgr.active_count(), 0);
    }

    #[test]
    fn chain_manager_end_nonexistent_returns_false() {
        let mut mgr = ChainManager::new(4);
        assert!(!mgr.end_chain("nonexistent"));
    }

    #[test]
    fn deeply_nested_chain_tracking() {
        let root = QueryChain::new_root(QuerySource::User);
        let mut current = root.clone();
        for expected_depth in 1..=5 {
            current = current.create_child(QuerySource::Agent);
            assert_eq!(current.depth(), expected_depth);
            assert!(current.parent_chain_id.is_some());
        }
    }
}
