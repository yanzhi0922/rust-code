//! Server resource types for MCP resource management.

use serde::{Deserialize, Serialize};

/// A resource exposed by an MCP server via `resources/list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerResource {
    /// Resource URI (e.g. `file:///path/to/resource`).
    pub uri: String,
    /// Human-readable name.
    #[serde(default)]
    pub name: Option<String>,
    /// Resource description.
    #[serde(default)]
    pub description: Option<String>,
    /// MIME type of the resource content.
    #[serde(default)]
    pub mime_type: Option<String>,
    /// The server that owns this resource.
    pub server: String,
}

impl ServerResource {
    /// Create a new server resource with the given URI and server name.
    #[must_use]
    pub fn new(uri: impl Into<String>, server: impl Into<String>) -> Self {
        Self {
            uri: uri.into(),
            name: None,
            description: None,
            mime_type: None,
            server: server.into(),
        }
    }

    /// Attach a human-readable name.
    #[must_use]
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Attach a description.
    #[must_use]
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Attach a MIME type.
    #[must_use]
    pub fn with_mime_type(mut self, mime_type: impl Into<String>) -> Self {
        self.mime_type = Some(mime_type.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_resource_has_uri_and_server() {
        let res = ServerResource::new("file:///data.csv", "my-server");
        assert_eq!(res.uri, "file:///data.csv");
        assert_eq!(res.server, "my-server");
        assert!(res.name.is_none());
        assert!(res.description.is_none());
        assert!(res.mime_type.is_none());
    }

    #[test]
    fn builder_pattern_sets_optional_fields() {
        let res = ServerResource::new("file:///data.csv", "my-server")
            .with_name("Data")
            .with_description("CSV data")
            .with_mime_type("text/csv");
        assert_eq!(res.name.as_deref(), Some("Data"));
        assert_eq!(res.description.as_deref(), Some("CSV data"));
        assert_eq!(res.mime_type.as_deref(), Some("text/csv"));
    }

    #[test]
    fn resource_serde_roundtrip() {
        let res = ServerResource::new("file:///data.csv", "my-server")
            .with_name("Data")
            .with_mime_type("text/csv");
        let json = serde_json::to_string(&res).expect("serialize");
        let back: ServerResource = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(res, back);
    }

    #[test]
    fn resource_equality() {
        let a = ServerResource::new("file:///a", "srv");
        let b = ServerResource::new("file:///a", "srv");
        assert_eq!(a, b);
    }

    #[test]
    fn resource_inequality_different_uri() {
        let a = ServerResource::new("file:///a", "srv");
        let b = ServerResource::new("file:///b", "srv");
        assert_ne!(a, b);
    }
}
