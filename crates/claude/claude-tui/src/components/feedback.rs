//! Feedback component for the TUI.
//!
//! Provides rendering for user feedback dialogs, surveys, and
//! feedback submission UI elements.
//!
//! # Components
//!
//! | Type | Description |
//! |------|-------------|
//! | [`FeedbackRating`] | User satisfaction rating |
//! | [`FeedbackEntry`] | A feedback submission |
//! | [`render_feedback_dialog`] | Render feedback collection dialog |
//! | [`render_feedback_thanks`] | Render thank-you message |

#![allow(dead_code)]

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::style::StyleConfig;

// ---------------------------------------------------------------------------
// FeedbackRating
// ---------------------------------------------------------------------------

/// User satisfaction rating.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedbackRating {
    /// Very dissatisfied.
    VeryDissatisfied,
    /// Dissatisfied.
    Dissatisfied,
    /// Neutral.
    Neutral,
    /// Satisfied.
    Satisfied,
    /// Very satisfied.
    VerySatisfied,
}

impl FeedbackRating {
    /// Returns the emoji for this rating.
    pub fn emoji(&self) -> &'static str {
        match self {
            FeedbackRating::VeryDissatisfied => "😡",
            FeedbackRating::Dissatisfied => "😕",
            FeedbackRating::Neutral => "😐",
            FeedbackRating::Satisfied => "😊",
            FeedbackRating::VerySatisfied => "🤩",
        }
    }

    /// Returns the label for this rating.
    pub fn label(&self) -> &'static str {
        match self {
            FeedbackRating::VeryDissatisfied => "Very Bad",
            FeedbackRating::Dissatisfied => "Bad",
            FeedbackRating::Neutral => "OK",
            FeedbackRating::Satisfied => "Good",
            FeedbackRating::VerySatisfied => "Great",
        }
    }

    /// All rating variants in order.
    pub fn all() -> &'static [FeedbackRating] {
        &[
            FeedbackRating::VeryDissatisfied,
            FeedbackRating::Dissatisfied,
            FeedbackRating::Neutral,
            FeedbackRating::Satisfied,
            FeedbackRating::VerySatisfied,
        ]
    }
}

// ---------------------------------------------------------------------------
// FeedbackEntry
// ---------------------------------------------------------------------------

/// A feedback submission.
#[derive(Debug, Clone)]
pub struct FeedbackEntry {
    /// The selected rating.
    pub rating: FeedbackRating,
    /// Optional text comment.
    pub comment: String,
    /// Category of feedback.
    pub category: String,
}

// ---------------------------------------------------------------------------
// Render functions
// ---------------------------------------------------------------------------

fn dim_span(text: &str) -> Span<'static> {
    Span::styled(
        text.to_owned(),
        Style::default().add_modifier(Modifier::DIM),
    )
}

fn header_span(text: &str, style: &StyleConfig) -> Span<'static> {
    Span::styled(
        text.to_owned(),
        Style::default()
            .fg(style.accent_color)
            .add_modifier(Modifier::BOLD),
    )
}

/// Render a feedback collection dialog.
pub fn render_feedback_dialog(
    selected: Option<FeedbackRating>,
    style: &StyleConfig,
) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(header_span(" 📝 How was your experience?", style)),
        Line::from(dim_span(" ─────────────────────────────────────────")),
        Line::from(""),
        Line::from("  Rate your experience:"),
        Line::from(""),
    ];

    for rating in FeedbackRating::all() {
        let is_selected = selected == Some(*rating);
        let mut spans = vec![Span::styled("   ".to_owned(), Style::default())];

        if is_selected {
            spans.push(Span::styled(
                "❯ ".to_owned(),
                Style::default()
                    .fg(style.accent_color)
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            spans.push(Span::styled("  ".to_owned(), Style::default()));
        }

        spans.push(Span::styled(rating.emoji().to_owned(), Style::default()));
        spans.push(Span::styled(
            format!(" {}", rating.label()),
            if is_selected {
                Style::default()
                    .fg(style.accent_color)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(style.status_fg)
            },
        ));

        lines.push(Line::from(spans));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(dim_span(
        "   ↑↓ select │ Enter submit │ Esc skip",
    )));

    lines
}

/// Render a thank-you message after feedback submission.
pub fn render_feedback_thanks(entry: &FeedbackEntry, style: &StyleConfig) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    lines.push(Line::from(header_span(
        " ✅ Thank you for your feedback!",
        style,
    )));
    lines.push(Line::from(dim_span(
        " ─────────────────────────────────────────",
    )));
    lines.push(Line::from(""));

    lines.push(Line::from(vec![
        Span::styled(
            "  Rating: ".to_owned(),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{} {}", entry.rating.emoji(), entry.rating.label()),
            Style::default().fg(Color::Cyan),
        ),
    ]));

    if !entry.comment.is_empty() {
        lines.push(Line::from(vec![
            Span::styled(
                "  Comment: ".to_owned(),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(entry.comment.clone()),
        ]));
    }

    lines.push(Line::from(vec![
        Span::styled(
            "  Category: ".to_owned(),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw(entry.category.clone()),
    ]));

    lines
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn test_style() -> StyleConfig {
        StyleConfig::dark()
    }

    #[test]
    fn rating_emoji() {
        assert_eq!(FeedbackRating::VeryDissatisfied.emoji(), "😡");
        assert_eq!(FeedbackRating::VerySatisfied.emoji(), "🤩");
    }

    #[test]
    fn rating_label() {
        assert_eq!(FeedbackRating::Neutral.label(), "OK");
        assert_eq!(FeedbackRating::Satisfied.label(), "Good");
    }

    #[test]
    fn rating_all_has_5() {
        assert_eq!(FeedbackRating::all().len(), 5);
    }

    #[test]
    fn render_dialog_shows_all_ratings() {
        let lines = render_feedback_dialog(None, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("Very Bad"));
        assert!(combined.contains("Bad"));
        assert!(combined.contains("OK"));
        assert!(combined.contains("Good"));
        assert!(combined.contains("Great"));
    }

    #[test]
    fn render_dialog_selected_highlighted() {
        let lines = render_feedback_dialog(Some(FeedbackRating::Satisfied), &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("❯"));
        assert!(combined.contains("Good"));
    }

    #[test]
    fn render_thanks_shows_rating() {
        let entry = FeedbackEntry {
            rating: FeedbackRating::VerySatisfied,
            comment: "Great tool!".to_owned(),
            category: "general".to_owned(),
        };
        let lines = render_feedback_thanks(&entry, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("Thank you"));
        assert!(combined.contains("Great"));
        assert!(combined.contains("Great tool!"));
        assert!(combined.contains("general"));
    }

    #[test]
    fn render_thanks_no_comment() {
        let entry = FeedbackEntry {
            rating: FeedbackRating::Neutral,
            comment: String::new(),
            category: "ux".to_owned(),
        };
        let lines = render_feedback_thanks(&entry, &test_style());
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(!combined.contains("Comment"));
    }
}
