//! System prompt section trait and registry.
//!
//! Each section of the system prompt implements [`SystemPromptSection`] and can
//! be either static (cacheable across sessions) or dynamic (recomputed per-turn).

pub mod actions;
pub mod agent;
pub mod ant_model_override;
pub mod brief;
pub mod doing_tasks;
pub mod env_info;
pub mod hooks;
pub mod intro;
pub mod language;
pub mod mcp_instructions;
pub mod memory;
pub mod numeric_length_anchors;
pub mod output_efficiency;
pub mod output_style;
pub mod proactive;
pub mod scratchpad;
pub mod session_guidance;
pub mod system;
pub mod system_reminders;
pub mod token_budget;
pub mod tone_style;
pub mod tool_result;
pub mod using_tools;

use anyhow::Result;

use crate::PromptContext;

/// A single section of the system prompt.
///
/// Implementations produce a string block (or `None` if the section should be
/// omitted) based on the current [`PromptContext`].
pub trait SystemPromptSection: Send + Sync {
    /// Human-readable name used for caching and debugging.
    fn name(&self) -> &str;

    /// Whether this section's output can be cached across turns.
    /// Cache-breaking sections recompute every turn.
    fn is_cacheable(&self) -> bool {
        true
    }

    /// Compute the section content given the current context.
    /// Return `None` to omit this section from the prompt.
    fn compute(&self, ctx: &PromptContext) -> Result<Option<String>>;
}

/// Helper: prepend bullet markers to a flat or nested list of items.
///
/// Matches Claude Code's `prependBullets`:
/// - Top-level items get `" - "`
/// - Sub-items (inner arrays) get `"  - "`
pub fn prepend_bullets(items: &[BulletItem]) -> Vec<String> {
    let mut result = Vec::with_capacity(items.len());
    for item in items {
        match item {
            BulletItem::Single(s) => {
                result.push(format!(" - {s}"));
            }
            BulletItem::Nested(subs) => {
                for sub in subs {
                    result.push(format!("  - {sub}"));
                }
            }
        }
    }
    result
}

/// A single bullet or a group of sub-bullets for [`prepend_bullets`].
#[derive(Debug, Clone)]
pub enum BulletItem {
    /// A single top-level bullet point.
    Single(String),
    /// A group of indented sub-bullet points.
    Nested(Vec<String>),
}

/// Build a section header + bullet list, matching Claude Code's pattern of
/// `["# Title", ...prependBullets(items)].join("\n")`.
pub fn section_with_bullets(title: &str, items: &[BulletItem]) -> String {
    let mut lines = vec![format!("# {title}")];
    lines.extend(prepend_bullets(items));
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepend_bullets_single_items() {
        let items = vec![
            BulletItem::Single("first".to_string()),
            BulletItem::Single("second".to_string()),
        ];
        let result = prepend_bullets(&items);
        assert_eq!(result, vec![" - first", " - second"]);
    }

    #[test]
    fn prepend_bullets_nested_items() {
        let items = vec![
            BulletItem::Single("top".to_string()),
            BulletItem::Nested(vec!["sub1".to_string(), "sub2".to_string()]),
        ];
        let result = prepend_bullets(&items);
        assert_eq!(result, vec![" - top", "  - sub1", "  - sub2"]);
    }

    #[test]
    fn prepend_bullets_mixed() {
        let items = vec![
            BulletItem::Single("a".to_string()),
            BulletItem::Nested(vec!["b1".to_string()]),
            BulletItem::Single("c".to_string()),
        ];
        let result = prepend_bullets(&items);
        assert_eq!(result, vec![" - a", "  - b1", " - c"]);
    }

    #[test]
    fn section_with_bullets_format() {
        let items = vec![
            BulletItem::Single("item1".to_string()),
            BulletItem::Nested(vec!["sub1".to_string()]),
        ];
        let result = section_with_bullets("My Section", &items);
        assert_eq!(result, "# My Section\n - item1\n  - sub1");
    }

    #[test]
    fn prepend_bullets_empty() {
        let items: Vec<BulletItem> = vec![];
        let result = prepend_bullets(&items);
        assert!(result.is_empty());
    }
}
