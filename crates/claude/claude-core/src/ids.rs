use std::fmt;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Strongly-typed session identifier used by the v2 engine surface.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionId(String);

impl SessionId {
    /// Create a new random session identifier.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4().to_string())
    }

    /// Create a session identifier from a UUID.
    #[must_use]
    pub fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid.to_string())
    }

    /// Borrow the raw identifier string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consume the wrapper and return the raw string.
    #[must_use]
    pub fn into_inner(self) -> String {
        self.0
    }

    /// Parse the identifier back into a UUID when it carries UUID formatting.
    pub fn try_as_uuid(&self) -> Result<Uuid, uuid::Error> {
        Uuid::parse_str(&self.0)
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl From<Uuid> for SessionId {
    fn from(value: Uuid) -> Self {
        Self(value.to_string())
    }
}

impl From<String> for SessionId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for SessionId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl From<SessionId> for String {
    fn from(value: SessionId) -> Self {
        value.0
    }
}

/// Strongly-typed agent identifier used by the v2 engine surface.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AgentId(String);

impl AgentId {
    /// Create a new agent identifier using the Claude Code-inspired prefix format.
    #[must_use]
    pub fn new(label: Option<&str>) -> Self {
        let raw = Uuid::new_v4().simple().to_string();
        let hex = &raw[..16];
        match label {
            Some(label) if !label.trim().is_empty() => Self(format!("a{}-{hex}", label.trim())),
            _ => Self(format!("a{hex}")),
        }
    }

    /// Borrow the raw identifier string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consume the wrapper and return the raw string.
    #[must_use]
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl Default for AgentId {
    fn default() -> Self {
        Self::new(None)
    }
}

impl fmt::Display for AgentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl From<String> for AgentId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for AgentId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl From<AgentId> for String {
    fn from(value: AgentId) -> Self {
        value.0
    }
}

#[cfg(test)]
mod tests {
    use super::{AgentId, SessionId};

    #[test]
    fn session_id_defaults_to_uuid_string() {
        let session_id = SessionId::new();
        assert_eq!(session_id.as_str().len(), 36);
        assert!(session_id.as_str().contains('-'));
    }

    #[test]
    fn agent_id_uses_optional_label_prefix() {
        let labeled = AgentId::new(Some("explorer"));
        assert!(labeled.as_str().starts_with("aexplorer-"));

        let unlabeled = AgentId::new(None);
        assert!(unlabeled.as_str().starts_with('a'));
        assert!(!unlabeled.as_str().contains("--"));
    }
}
