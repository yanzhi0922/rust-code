use serde::{Deserialize, Serialize};

/// Marker written after compaction so loaders can recover the surviving suffix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactBoundary {
    /// Whether compaction was initiated manually or automatically.
    pub trigger: CompactTrigger,
    /// Token count observed before compaction happened.
    pub pre_tokens: u64,
    /// Optional free-form context captured alongside the boundary.
    #[serde(default)]
    pub user_context: Option<String>,
    /// Optional count of messages summarized by the compaction step.
    #[serde(default)]
    pub messages_summarized: Option<usize>,
    /// Tool names discovered before compaction that must survive suffix-only resume.
    #[serde(default)]
    pub pre_compact_discovered_tools: Vec<String>,
    /// Optional relink information for suffix/prefix-preserving compactions.
    #[serde(default)]
    pub preserved_segment: Option<PreservedSegment>,
}

impl CompactBoundary {
    /// Construct a minimal boundary marker.
    #[must_use]
    pub fn new(trigger: CompactTrigger, pre_tokens: u64) -> Self {
        Self {
            trigger,
            pre_tokens,
            user_context: None,
            messages_summarized: None,
            pre_compact_discovered_tools: Vec::new(),
            preserved_segment: None,
        }
    }
}

/// Compaction trigger source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactTrigger {
    Manual,
    Auto,
}

/// Message-chain relink info retained across compaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreservedSegment {
    pub head_uuid: String,
    pub anchor_uuid: String,
    pub tail_uuid: String,
}

#[cfg(test)]
mod tests {
    use super::{CompactBoundary, CompactTrigger, PreservedSegment};

    #[test]
    fn boundary_round_trips_with_optional_fields() {
        let boundary = CompactBoundary {
            trigger: CompactTrigger::Auto,
            pre_tokens: 4096,
            user_context: Some("session summary".to_owned()),
            messages_summarized: Some(12),
            pre_compact_discovered_tools: vec![
                "mcp__context7__query_docs".to_owned(),
                "web_fetch".to_owned(),
            ],
            preserved_segment: Some(PreservedSegment {
                head_uuid: "head".to_owned(),
                anchor_uuid: "anchor".to_owned(),
                tail_uuid: "tail".to_owned(),
            }),
        };

        let json = serde_json::to_value(&boundary).expect("boundary should serialize");
        let decoded: CompactBoundary =
            serde_json::from_value(json).expect("boundary should deserialize");
        assert_eq!(decoded, boundary);
    }
}
