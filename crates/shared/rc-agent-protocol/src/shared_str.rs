//! A cheaply-cloneable string type backed by `Arc<str>`.
//!
//! [`SharedStr`] is designed for protocol event fields that are cloned
//! frequently (e.g., `session_id` on every streaming token delta). It
//! provides the same API surface as `String` for serialization and display
//! while making `Clone` a simple reference-count bump.

use std::fmt;
use std::ops::Deref;
use std::sync::Arc;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A reference-counted, immutable string.
///
/// Cloning is O(1) (atomic refcount increment). Useful for fields that are
/// set once and then shared across many event clones (e.g., session IDs,
/// tool names).
///
/// Serializes/deserializes as a plain JSON string.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SharedStr(Arc<str>);

impl SharedStr {
    /// Create a new `SharedStr` from any string-like value.
    #[inline]
    pub fn new(s: impl Into<Arc<str>>) -> Self {
        Self(s.into())
    }

    /// View the underlying string slice.
    #[inline]
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

// ── Conversions ─────────────────────────────────────────────────────────────

impl From<String> for SharedStr {
    #[inline]
    fn from(s: String) -> Self {
        Self(Arc::from(s))
    }
}

impl From<&str> for SharedStr {
    #[inline]
    fn from(s: &str) -> Self {
        Self(Arc::from(s))
    }
}

impl From<Arc<str>> for SharedStr {
    #[inline]
    fn from(s: Arc<str>) -> Self {
        Self(s)
    }
}

impl From<SharedStr> for String {
    #[inline]
    fn from(s: SharedStr) -> Self {
        s.0.to_string()
    }
}

// ── Deref / Display / Debug ─────────────────────────────────────────────────

impl Deref for SharedStr {
    type Target = str;

    #[inline]
    fn deref(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for SharedStr {
    #[inline]
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SharedStr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for SharedStr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", &*self.0)
    }
}

// ── Default ─────────────────────────────────────────────────────────────────

impl Default for SharedStr {
    #[inline]
    fn default() -> Self {
        Self(Arc::from(""))
    }
}

// ── Serde ───────────────────────────────────────────────────────────────────

impl Serialize for SharedStr {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for SharedStr {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Ok(Self::from(s))
    }
}

// ── PartialEq with str / String ─────────────────────────────────────────────

impl PartialEq<str> for SharedStr {
    fn eq(&self, other: &str) -> bool {
        &*self.0 == other
    }
}

impl PartialEq<String> for SharedStr {
    fn eq(&self, other: &String) -> bool {
        &*self.0 == other.as_str()
    }
}

impl PartialEq<&str> for SharedStr {
    fn eq(&self, other: &&str) -> bool {
        &*self.0 == *other
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clone_is_cheap() {
        let a = SharedStr::from("hello");
        let b = a.clone();
        assert!(Arc::ptr_eq(&a.0, &b.0));
    }

    #[test]
    fn serde_roundtrip() {
        let original = SharedStr::from("test-session-id");
        let json = serde_json::to_string(&original).expect("SharedStr should serialize");
        assert_eq!(json, "\"test-session-id\"");
        let back: SharedStr = serde_json::from_str(&json).expect("SharedStr should deserialize");
        assert_eq!(original, back);
    }

    #[test]
    fn display_and_deref() {
        let s = SharedStr::from("hello");
        assert_eq!(format!("{s}"), "hello");
        assert_eq!(s.len(), 5);
        assert_eq!(s.as_str(), "hello");
    }

    #[test]
    fn equality_with_str() {
        let s = SharedStr::from("abc");
        assert_eq!(s, "abc");
        assert_eq!(s, String::from("abc"));
    }

    #[test]
    fn default_is_empty() {
        let s = SharedStr::default();
        assert!(s.is_empty());
    }
}
