//! Fuzzy-search picker component for the TUI.
//!
//! Provides a fuzzy-matching algorithm and a picker widget that filters and
//! displays a list of items based on a user's query string.
//!
//! # Components
//!
//! | Type | Description |
//! |------|-------------|
//! | [`FuzzyPicker`] | Stateful picker with query, items, and selection |
//! | [`fuzzy_match`] | Fuzzy matching algorithm |
//! | [`render_fuzzy_picker`] | Render the picker into ratatui lines |

#![allow(dead_code)]

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::style::StyleConfig;

// ---------------------------------------------------------------------------
// Fuzzy matching
// ---------------------------------------------------------------------------

/// Result of a fuzzy match containing the score and matched character indices.
#[derive(Debug, Clone)]
pub struct FuzzyResult {
    /// Match quality score (higher is better).
    pub score: i64,
    /// Indices of matched characters in the original string.
    pub matched_indices: Vec<usize>,
}

/// Perform a fuzzy match of `query` against `candidate`.
///
/// Returns `Some(FuzzyResult)` if the query matches the candidate, `None`
/// otherwise. The scoring rewards:
/// - Consecutive character matches
/// - Matches at word boundaries (after `_`, `-`, spaces, or at start)
/// - Matches at the beginning of the candidate
///
/// # Examples
///
/// ```
/// use claude_tui::components::fuzzy_picker::fuzzy_match;
///
/// let result = fuzzy_match("fs", "filesystem");
/// assert!(result.is_some());
/// assert!(result.unwrap().score > 0);
/// ```
pub fn fuzzy_match(query: &str, candidate: &str) -> Option<FuzzyResult> {
    if query.is_empty() {
        return Some(FuzzyResult {
            score: 0,
            matched_indices: Vec::new(),
        });
    }

    let query_lower: String = query.to_lowercase();
    let candidate_lower: String = candidate.to_lowercase();

    let query_chars: Vec<char> = query_lower.chars().collect();
    let candidate_chars: Vec<char> = candidate_lower.chars().collect();

    if query_chars.is_empty() {
        return Some(FuzzyResult {
            score: 0,
            matched_indices: Vec::new(),
        });
    }

    // Try to find each query char in order in the candidate
    let mut matched_indices = Vec::new();
    let mut candidate_idx = 0;

    for query_char in &query_chars {
        let mut found = false;
        while candidate_idx < candidate_chars.len() {
            if candidate_chars[candidate_idx] == *query_char {
                matched_indices.push(candidate_idx);
                candidate_idx += 1;
                found = true;
                break;
            }
            candidate_idx += 1;
        }
        if !found {
            return None;
        }
    }

    // Score the match
    let mut score: i64 = 100; // base score for matching all chars

    // Bonus for short candidate (closer match)
    score -= candidate.len() as i64;

    // Bonus for consecutive matches
    for window in matched_indices.windows(2) {
        if window[1] == window[0] + 1 {
            score += 10; // consecutive bonus
        }
    }

    // Bonus for matches at word boundaries
    let candidate_str: Vec<char> = candidate.chars().collect();
    for &idx in &matched_indices {
        if idx == 0 {
            score += 5; // start of string bonus
        } else if idx > 0 {
            let prev = candidate_str[idx - 1];
            if prev == '_' || prev == '-' || prev == ' ' || prev == '/' {
                score += 5; // word boundary bonus
            }
        }
    }

    // Bonus for early matches
    if let Some(&first_idx) = matched_indices.first() {
        score += (100 - first_idx as i64).max(0);
    }

    Some(FuzzyResult {
        score,
        matched_indices,
    })
}

// ---------------------------------------------------------------------------
// FuzzyPicker
// ---------------------------------------------------------------------------

/// A fuzzy-search picker that maintains query state and a filtered item list.
#[derive(Debug, Clone)]
pub struct FuzzyPicker {
    /// The current search query.
    pub query: String,
    /// All available items.
    pub items: Vec<String>,
    /// Index of the currently selected item in the filtered list.
    pub selected: usize,
    /// Maximum number of visible items.
    pub max_visible: usize,
}

impl FuzzyPicker {
    /// Create a new picker with the given items.
    pub fn new(items: Vec<String>) -> Self {
        Self {
            query: String::new(),
            items,
            selected: 0,
            max_visible: 10,
        }
    }

    /// Set the maximum number of visible items.
    pub fn with_max_visible(mut self, n: usize) -> Self {
        self.max_visible = n.max(1);
        self
    }

    /// Update the query string and reset selection.
    pub fn set_query(&mut self, query: String) {
        self.query = query;
        self.selected = 0;
    }

    /// Get the filtered and scored list of items.
    pub fn filtered(&self) -> Vec<(String, FuzzyResult)> {
        let mut results: Vec<(String, FuzzyResult)> = self
            .items
            .iter()
            .filter_map(|item| fuzzy_match(&self.query, item).map(|fr| (item.clone(), fr)))
            .collect();

        // Sort by score descending
        results.sort_by(|a, b| b.1.score.cmp(&a.1.score));
        results
    }

    /// Get the currently selected item, if any.
    pub fn selected_item(&self) -> Option<String> {
        let filtered = self.filtered();
        filtered.get(self.selected).map(|(item, _)| item.clone())
    }

    /// Move selection up.
    pub fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    /// Move selection down.
    pub fn move_down(&mut self) {
        let count = self.filtered().len();
        if self.selected + 1 < count {
            self.selected += 1;
        }
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Render the fuzzy picker into ratatui lines.
///
/// Shows a search prompt, the filtered list with the selected item
/// highlighted, and match indicators on matched characters.
pub fn render_fuzzy_picker(picker: &FuzzyPicker, style: &StyleConfig) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    // Search prompt
    lines.push(Line::from(vec![
        Span::styled(
            " > ".to_owned(),
            Style::default()
                .fg(style.accent_color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            if picker.query.is_empty() {
                "Type to search…".to_owned()
            } else {
                picker.query.clone()
            },
            Style::default().fg(style.status_fg),
        ),
    ]));

    // Filtered items
    let filtered = picker.filtered();
    let visible_count = filtered.len().min(picker.max_visible);

    // Calculate scroll offset to keep selected item visible
    let scroll_offset = if picker.selected >= picker.max_visible {
        picker.selected - picker.max_visible + 1
    } else {
        0
    };

    for i in 0..visible_count {
        let idx = i + scroll_offset;
        if idx >= filtered.len() {
            break;
        }

        let (item, result) = &filtered[idx];
        let is_selected = idx == picker.selected;

        let mut spans = Vec::new();

        // Selection indicator
        if is_selected {
            spans.push(Span::styled(
                " ❯ ".to_owned(),
                Style::default()
                    .fg(style.accent_color)
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            spans.push(Span::styled("   ", Style::default()));
        }

        // Item text with highlighted matched characters
        let item_chars: Vec<char> = item.chars().collect();
        for (ci, ch) in item_chars.iter().enumerate() {
            let is_matched = result.matched_indices.contains(&ci);
            if is_matched {
                if is_selected {
                    spans.push(Span::styled(
                        ch.to_string(),
                        Style::default()
                            .fg(Color::Black)
                            .bg(style.accent_color)
                            .add_modifier(Modifier::BOLD),
                    ));
                } else {
                    spans.push(Span::styled(
                        ch.to_string(),
                        Style::default()
                            .fg(style.accent_color)
                            .add_modifier(Modifier::BOLD),
                    ));
                }
            } else if is_selected {
                spans.push(Span::styled(
                    ch.to_string(),
                    Style::default().fg(style.status_fg),
                ));
            } else {
                spans.push(Span::styled(
                    ch.to_string(),
                    Style::default().fg(style.info_color),
                ));
            }
        }

        lines.push(Line::from(spans));
    }

    // Footer hint
    if !filtered.is_empty() {
        let total = filtered.len();
        let showing = visible_count.min(total);
        lines.push(Line::from(vec![Span::styled(
            format!("   {showing}/{total} items │ ↑↓ navigate │ Enter select │ Esc cancel"),
            Style::default()
                .fg(style.info_color)
                .add_modifier(Modifier::DIM),
        )]));
    } else {
        lines.push(Line::from(vec![Span::styled(
            "   No matches found".to_owned(),
            Style::default()
                .fg(style.error_color)
                .add_modifier(Modifier::DIM),
        )]));
    }

    lines
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // --- Fuzzy match tests ---

    #[test]
    fn fuzzy_match_exact() {
        let result = fuzzy_match("hello", "hello");
        assert!(result.is_some());
        assert!(result.expect("exact match should succeed").score > 0);
    }

    #[test]
    fn fuzzy_match_prefix() {
        let result = fuzzy_match("fs", "filesystem");
        assert!(result.is_some());
        let r = result.expect("prefix match should succeed");
        assert!(r.score > 0);
        assert_eq!(r.matched_indices.len(), 2);
    }

    #[test]
    fn fuzzy_match_subsequence() {
        let result = fuzzy_match("fle", "filesystem");
        assert!(result.is_some());
        assert_eq!(
            result
                .expect("subsequence match should succeed")
                .matched_indices
                .len(),
            3
        );
    }

    #[test]
    fn fuzzy_match_case_insensitive() {
        let result = fuzzy_match("FS", "filesystem");
        assert!(result.is_some());
    }

    #[test]
    fn fuzzy_match_no_match() {
        let result = fuzzy_match("xyz", "filesystem");
        assert!(result.is_none());
    }

    #[test]
    fn fuzzy_match_empty_query() {
        let result = fuzzy_match("", "anything");
        assert!(result.is_some());
        assert_eq!(result.expect("empty query match should succeed").score, 0);
    }

    #[test]
    fn fuzzy_match_empty_candidate() {
        let result = fuzzy_match("a", "");
        assert!(result.is_none());
    }

    #[test]
    fn fuzzy_match_empty_both() {
        let result = fuzzy_match("", "");
        assert!(result.is_some());
    }

    #[test]
    fn fuzzy_match_word_boundary_bonus() {
        let r1 = fuzzy_match("sf", "some_function").expect("word boundary match");
        let r2 = fuzzy_match("sf", "sxxxxfxxxx").expect("non-word boundary match");
        // Word boundary match should score higher
        assert!(r1.score > r2.score);
    }

    #[test]
    fn fuzzy_match_consecutive_bonus() {
        let r1 = fuzzy_match("ab", "abxxxx").expect("consecutive match");
        let r2 = fuzzy_match("ab", "axbxxx").expect("non-consecutive match");
        // Consecutive match should score higher
        assert!(r1.score > r2.score);
    }

    // --- FuzzyPicker tests ---

    #[test]
    fn picker_new() {
        let picker = FuzzyPicker::new(vec!["file1.rs".to_owned(), "file2.ts".to_owned()]);
        assert_eq!(picker.items.len(), 2);
        assert!(picker.query.is_empty());
        assert_eq!(picker.selected, 0);
    }

    #[test]
    fn picker_set_query() {
        let mut picker = FuzzyPicker::new(vec!["file1.rs".to_owned(), "file2.ts".to_owned()]);
        picker.set_query("rs".to_owned());
        assert_eq!(picker.query, "rs");
        assert_eq!(picker.selected, 0);
    }

    #[test]
    fn picker_filtered_no_query() {
        let picker = FuzzyPicker::new(vec!["file1.rs".to_owned(), "file2.ts".to_owned()]);
        let filtered = picker.filtered();
        assert_eq!(filtered.len(), 2);
    }

    #[test]
    fn picker_filtered_with_query() {
        let mut picker = FuzzyPicker::new(vec![
            "file1.rs".to_owned(),
            "file2.ts".to_owned(),
            "mod.rs".to_owned(),
        ]);
        picker.set_query("rs".to_owned());
        let filtered = picker.filtered();
        assert_eq!(filtered.len(), 2); // file1.rs and mod.rs
    }

    #[test]
    fn picker_move_up_down() {
        let mut picker = FuzzyPicker::new(vec!["a".to_owned(), "b".to_owned(), "c".to_owned()]);
        assert_eq!(picker.selected, 0);
        picker.move_down();
        assert_eq!(picker.selected, 1);
        picker.move_down();
        assert_eq!(picker.selected, 2);
        picker.move_down(); // at end, should stay
        assert_eq!(picker.selected, 2);
        picker.move_up();
        assert_eq!(picker.selected, 1);
    }

    #[test]
    fn picker_move_up_at_zero_stays() {
        let mut picker = FuzzyPicker::new(vec!["a".to_owned()]);
        picker.move_up();
        assert_eq!(picker.selected, 0);
    }

    #[test]
    fn picker_selected_item() {
        let mut picker = FuzzyPicker::new(vec!["alpha".to_owned(), "beta".to_owned()]);
        assert_eq!(picker.selected_item(), Some("alpha".to_owned()));
        picker.move_down();
        assert_eq!(picker.selected_item(), Some("beta".to_owned()));
    }

    // --- Render tests ---

    #[test]
    fn render_picker_basic() {
        let picker = FuzzyPicker::new(vec!["file1.rs".to_owned(), "file2.ts".to_owned()]);
        let lines = render_fuzzy_picker(&picker, &StyleConfig::dark());
        assert!(lines.len() >= 3); // prompt + 2 items + footer
        let first = lines[0].to_string();
        assert!(first.contains("Type to search"));
    }

    #[test]
    fn render_picker_with_query() {
        let mut picker = FuzzyPicker::new(vec!["file1.rs".to_owned(), "file2.ts".to_owned()]);
        picker.set_query("rs".to_owned());
        let lines = render_fuzzy_picker(&picker, &StyleConfig::dark());
        let first = lines[0].to_string();
        assert!(first.contains("rs"));
    }

    #[test]
    fn render_picker_no_matches() {
        let mut picker = FuzzyPicker::new(vec!["file1.rs".to_owned()]);
        picker.set_query("xyz".to_owned());
        let lines = render_fuzzy_picker(&picker, &StyleConfig::dark());
        let combined = lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(combined.contains("No matches"));
    }

    #[test]
    fn render_picker_max_visible() {
        let items: Vec<String> = (0..20).map(|i| format!("item{i}")).collect();
        let picker = FuzzyPicker::new(items).with_max_visible(5);
        let lines = render_fuzzy_picker(&picker, &StyleConfig::dark());
        // prompt + 5 visible items + footer = 7
        assert_eq!(lines.len(), 7);
    }
}
