//! Cache control boundary management for system prompt sections.
//!
//! The system prompt is split into two regions:
//! - **Static** (before the boundary): content that is the same across sessions
//!   and can use global prompt caching.
//! - **Dynamic** (after the boundary): content that varies per-session and
//!   must not be cached globally.

use std::collections::HashMap;

/// Boundary marker separating static (cross-org cacheable) content from dynamic content.
///
/// Everything BEFORE this marker in the system prompt array can use scope: 'global'.
/// Everything AFTER contains user/session-specific content and should not be cached.
pub const SYSTEM_PROMPT_DYNAMIC_BOUNDARY: &str = "__SYSTEM_PROMPT_DYNAMIC_BOUNDARY__";

/// In-memory cache for computed system prompt sections.
///
/// Sections are computed once and cached until explicitly cleared (e.g. on `/clear`
/// or `/compact`). Cache-breaking sections recompute every turn.
#[derive(Debug, Default)]
pub struct SectionCache {
    entries: HashMap<String, Option<String>>,
}

impl SectionCache {
    /// Create a new empty section cache.
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    /// Get a cached section value by name.
    pub fn get(&self, name: &str) -> Option<&Option<String>> {
        self.entries.get(name)
    }

    /// Insert a computed section value into the cache.
    pub fn set(&mut self, name: &str, value: Option<String>) {
        self.entries.insert(name.to_string(), value);
    }

    /// Clear all cached entries.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Check if a section is cached.
    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.entries.contains_key(name)
    }

    /// Return the number of cached entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Return true if the cache is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boundary_marker_has_expected_value() {
        assert_eq!(
            SYSTEM_PROMPT_DYNAMIC_BOUNDARY,
            "__SYSTEM_PROMPT_DYNAMIC_BOUNDARY__"
        );
    }

    #[test]
    fn cache_set_and_get() {
        let mut cache = SectionCache::new();
        assert!(!cache.contains("test_section"));
        cache.set("test_section", Some("hello".to_string()));
        assert!(cache.contains("test_section"));
        assert_eq!(cache.get("test_section"), Some(&Some("hello".to_string())));
    }

    #[test]
    fn cache_clear_removes_all() {
        let mut cache = SectionCache::new();
        cache.set("a", Some("1".to_string()));
        cache.set("b", None);
        assert_eq!(cache.len(), 2);
        cache.clear();
        assert!(cache.is_empty());
    }

    #[test]
    fn cache_stores_none_values() {
        let mut cache = SectionCache::new();
        cache.set("empty", None);
        assert!(cache.contains("empty"));
        assert_eq!(cache.get("empty"), Some(&None));
    }

    #[test]
    fn cache_overwrites_existing() {
        let mut cache = SectionCache::new();
        cache.set("key", Some("old".to_string()));
        cache.set("key", Some("new".to_string()));
        assert_eq!(cache.get("key"), Some(&Some("new".to_string())));
    }
}
