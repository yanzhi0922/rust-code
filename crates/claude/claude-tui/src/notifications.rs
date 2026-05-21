//! Notification system for the TUI.
//!
//! Provides a notification management system inspired by
//! Claude Code's notification hooks. Supports:
//! - Multiple notification types (Info, Warning, Error, Success)
//! - Configurable display duration and position
//! - Max visible notifications with auto-dismissal
//! - Priority-based ordering

use std::fmt;
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Notification ID
// ---------------------------------------------------------------------------

/// Unique identifier for a notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NotificationId(u64);

impl NotificationId {
    /// Generate a new unique ID.
    fn next() -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        NotificationId(id)
    }
}

impl fmt::Display for NotificationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "notif-{}", self.0)
    }
}

// ---------------------------------------------------------------------------
// Notification Type
// ---------------------------------------------------------------------------

/// Type of notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NotificationType {
    /// Informational message.
    Info,
    /// Warning message.
    Warning,
    /// Error message.
    Error,
    /// Success message.
    Success,
}

impl NotificationType {
    /// Get the icon/emoji for this notification type.
    pub fn icon(self) -> &'static str {
        match self {
            Self::Info => "ℹ",
            Self::Warning => "⚠",
            Self::Error => "✖",
            Self::Success => "✔",
        }
    }

    /// Get the label for this notification type.
    pub fn label(self) -> &'static str {
        match self {
            Self::Info => "INFO",
            Self::Warning => "WARN",
            Self::Error => "ERROR",
            Self::Success => "OK",
        }
    }

    /// Get the default duration for this notification type.
    pub fn default_duration(self) -> Duration {
        match self {
            Self::Info => Duration::from_secs(5),
            Self::Warning => Duration::from_secs(8),
            Self::Error => Duration::from_secs(15),
            Self::Success => Duration::from_secs(3),
        }
    }

    /// Get the priority (higher = more important).
    pub fn priority(self) -> u8 {
        match self {
            Self::Info => 0,
            Self::Success => 1,
            Self::Warning => 2,
            Self::Error => 3,
        }
    }
}

// ---------------------------------------------------------------------------
// Notification Position
// ---------------------------------------------------------------------------

/// Position of the notification on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NotificationPosition {
    /// Top-right corner.
    TopRight,
    /// Top-left corner.
    TopLeft,
    /// Bottom-right corner.
    BottomRight,
    /// Bottom-left corner.
    BottomLeft,
    /// Top center.
    TopCenter,
    /// Bottom center.
    BottomCenter,
}

impl fmt::Display for NotificationPosition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TopRight => write!(f, "top-right"),
            Self::TopLeft => write!(f, "top-left"),
            Self::BottomRight => write!(f, "bottom-right"),
            Self::BottomLeft => write!(f, "bottom-left"),
            Self::TopCenter => write!(f, "top-center"),
            Self::BottomCenter => write!(f, "bottom-center"),
        }
    }
}

// ---------------------------------------------------------------------------
// Notification
// ---------------------------------------------------------------------------

/// A notification to display.
#[derive(Debug, Clone)]
pub struct Notification {
    /// Unique identifier.
    pub id: NotificationId,
    /// Notification type.
    pub notif_type: NotificationType,
    /// Title text.
    pub title: String,
    /// Body text.
    pub message: String,
    /// When the notification was created.
    pub created_at: Instant,
    /// Duration after which the notification auto-dismisses.
    pub duration: Duration,
    /// Whether the notification has been dismissed.
    pub dismissed: bool,
    /// Optional source/module.
    pub source: Option<String>,
    /// Whether the notification is dismissible by the user.
    pub dismissible: bool,
}

impl Notification {
    /// Create a new notification.
    pub fn new(
        notif_type: NotificationType,
        title: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        let duration = notif_type.default_duration();
        Notification {
            id: NotificationId::next(),
            notif_type,
            title: title.into(),
            message: message.into(),
            created_at: Instant::now(),
            duration,
            dismissed: false,
            source: None,
            dismissible: true,
        }
    }

    /// Create an info notification.
    pub fn info(title: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(NotificationType::Info, title, message)
    }

    /// Create a warning notification.
    pub fn warning(title: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(NotificationType::Warning, title, message)
    }

    /// Create an error notification.
    pub fn error(title: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(NotificationType::Error, title, message)
    }

    /// Create a success notification.
    pub fn success(title: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(NotificationType::Success, title, message)
    }

    /// Set the duration.
    pub fn with_duration(mut self, duration: Duration) -> Self {
        self.duration = duration;
        self
    }

    /// Set the source.
    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }

    /// Set whether dismissible.
    pub fn with_dismissible(mut self, dismissible: bool) -> Self {
        self.dismissible = dismissible;
        self
    }

    /// Check if the notification has expired.
    pub fn is_expired(&self) -> bool {
        self.dismissed || self.created_at.elapsed() >= self.duration
    }

    /// Dismiss the notification.
    pub fn dismiss(&mut self) {
        self.dismissed = true;
    }

    /// Time remaining before auto-dismiss.
    pub fn time_remaining(&self) -> Duration {
        self.duration.saturating_sub(self.created_at.elapsed())
    }

    /// Format for display.
    pub fn display_line(&self) -> String {
        format!(
            "{} [{}] {} — {}",
            self.notif_type.icon(),
            self.notif_type.label(),
            self.title,
            self.message
        )
    }
}

// ---------------------------------------------------------------------------
// Notification Config
// ---------------------------------------------------------------------------

/// Configuration for the notification system.
#[derive(Debug, Clone, PartialEq)]
pub struct NotificationConfig {
    /// Default display duration.
    pub default_duration: Duration,
    /// Maximum number of visible notifications.
    pub max_visible: usize,
    /// Position on screen.
    pub position: NotificationPosition,
    /// Whether notifications are enabled.
    pub enabled: bool,
}

impl Default for NotificationConfig {
    fn default() -> Self {
        NotificationConfig {
            default_duration: Duration::from_secs(5),
            max_visible: 5,
            position: NotificationPosition::TopRight,
            enabled: true,
        }
    }
}

impl NotificationConfig {
    /// Create a new config with custom max_visible.
    pub fn with_max_visible(mut self, max: usize) -> Self {
        self.max_visible = max.max(1);
        self
    }

    /// Create a new config with custom position.
    pub fn with_position(mut self, position: NotificationPosition) -> Self {
        self.position = position;
        self
    }

    /// Create a new config with custom duration.
    pub fn with_duration(mut self, duration: Duration) -> Self {
        self.default_duration = duration;
        self
    }
}

// ---------------------------------------------------------------------------
// Notification Manager
// ---------------------------------------------------------------------------

/// Manages notification lifecycle.
#[derive(Debug, Clone)]
pub struct NotificationManager {
    /// Configuration.
    config: NotificationConfig,
    /// Active notifications.
    notifications: Vec<Notification>,
    /// History of dismissed notifications.
    history: Vec<Notification>,
    /// Maximum history size.
    max_history: usize,
}

impl NotificationManager {
    /// Create a new notification manager with the given config.
    pub fn new(config: NotificationConfig) -> Self {
        NotificationManager {
            config,
            notifications: Vec::new(),
            history: Vec::new(),
            max_history: 100,
        }
    }

    /// Create with default config.
    pub fn with_defaults() -> Self {
        Self::new(NotificationConfig::default())
    }

    /// Get the configuration.
    pub fn config(&self) -> &NotificationConfig {
        &self.config
    }

    /// Add a notification.
    pub fn add(&mut self, notification: Notification) -> NotificationId {
        if !self.config.enabled {
            return notification.id;
        }

        let id = notification.id;
        self.notifications.push(notification);

        // Enforce max visible.
        while self.notifications.len() > self.config.max_visible {
            if let Some(oldest) = self.notifications.first_mut() {
                oldest.dismiss();
            }
            let dismissed = self.notifications.remove(0);
            self.push_history(dismissed);
        }

        id
    }

    /// Add an info notification.
    pub fn info(&mut self, title: impl Into<String>, message: impl Into<String>) -> NotificationId {
        self.add(Notification::info(title, message))
    }

    /// Add a warning notification.
    pub fn warning(
        &mut self,
        title: impl Into<String>,
        message: impl Into<String>,
    ) -> NotificationId {
        self.add(Notification::warning(title, message))
    }

    /// Add an error notification.
    pub fn error(
        &mut self,
        title: impl Into<String>,
        message: impl Into<String>,
    ) -> NotificationId {
        self.add(Notification::error(title, message))
    }

    /// Add a success notification.
    pub fn success(
        &mut self,
        title: impl Into<String>,
        message: impl Into<String>,
    ) -> NotificationId {
        self.add(Notification::success(title, message))
    }

    /// Dismiss a notification by ID.
    pub fn dismiss(&mut self, id: NotificationId) -> bool {
        if let Some(notif) = self.notifications.iter_mut().find(|n| n.id == id)
            && notif.dismissible
        {
            notif.dismiss();
            let idx = self.notifications.iter().position(|n| n.id == id);
            if let Some(idx) = idx {
                let dismissed = self.notifications.remove(idx);
                self.push_history(dismissed);
            }
            return true;
        }
        false
    }

    /// Dismiss all notifications.
    pub fn dismiss_all(&mut self) {
        for notif in &mut self.notifications {
            notif.dismiss();
        }
        while let Some(dismissed) = self.notifications.pop() {
            self.push_history(dismissed);
        }
    }

    /// Remove expired notifications.
    pub fn remove_expired(&mut self) -> usize {
        let expired: Vec<NotificationId> = self
            .notifications
            .iter()
            .filter(|n| n.is_expired())
            .map(|n| n.id)
            .collect();

        let count = expired.len();
        for id in expired {
            let idx = self.notifications.iter().position(|n| n.id == id);
            if let Some(idx) = idx {
                let dismissed = self.notifications.remove(idx);
                self.push_history(dismissed);
            }
        }
        count
    }

    /// Get the active (non-expired, non-dismissed) notifications.
    pub fn active(&self) -> Vec<&Notification> {
        self.notifications
            .iter()
            .filter(|n| !n.is_expired())
            .collect()
    }

    /// Get all notifications (including expired but not yet cleaned).
    pub fn all(&self) -> &[Notification] {
        &self.notifications
    }

    /// Get the notification history.
    pub fn history(&self) -> &[Notification] {
        &self.history
    }

    /// Get the number of active notifications.
    pub fn active_count(&self) -> usize {
        self.notifications
            .iter()
            .filter(|n| !n.is_expired())
            .count()
    }

    /// Get the total number of notifications (active + history).
    pub fn total_count(&self) -> usize {
        self.notifications.len() + self.history.len()
    }

    /// Check if there are any active notifications.
    pub fn has_active(&self) -> bool {
        self.active_count() > 0
    }

    /// Find a notification by ID.
    pub fn find(&self, id: NotificationId) -> Option<&Notification> {
        self.notifications.iter().find(|n| n.id == id)
    }

    /// Clear all notifications and history.
    pub fn clear(&mut self) {
        self.notifications.clear();
        self.history.clear();
    }

    /// Push a notification into history, respecting max size.
    fn push_history(&mut self, notification: Notification) {
        self.history.push(notification);
        if self.history.len() > self.max_history {
            self.history.remove(0);
        }
    }
}

impl Default for NotificationManager {
    fn default() -> Self {
        Self::with_defaults()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_notification_type_icon() {
        assert_eq!(NotificationType::Info.icon(), "ℹ");
        assert_eq!(NotificationType::Warning.icon(), "⚠");
        assert_eq!(NotificationType::Error.icon(), "✖");
        assert_eq!(NotificationType::Success.icon(), "✔");
    }

    #[test]
    fn test_notification_type_label() {
        assert_eq!(NotificationType::Info.label(), "INFO");
        assert_eq!(NotificationType::Warning.label(), "WARN");
        assert_eq!(NotificationType::Error.label(), "ERROR");
        assert_eq!(NotificationType::Success.label(), "OK");
    }

    #[test]
    fn test_notification_type_priority() {
        assert!(NotificationType::Error.priority() > NotificationType::Warning.priority());
        assert!(NotificationType::Warning.priority() > NotificationType::Success.priority());
        assert!(NotificationType::Success.priority() > NotificationType::Info.priority());
    }

    #[test]
    fn test_notification_type_default_duration() {
        assert!(
            NotificationType::Error.default_duration() > NotificationType::Info.default_duration()
        );
    }

    #[test]
    fn test_notification_new() {
        let n = Notification::info("Title", "Message");
        assert_eq!(n.title, "Title");
        assert_eq!(n.message, "Message");
        assert_eq!(n.notif_type, NotificationType::Info);
        assert!(!n.dismissed);
        assert!(n.dismissible);
    }

    #[test]
    fn test_notification_with_duration() {
        let n = Notification::info("T", "M").with_duration(Duration::from_secs(10));
        assert_eq!(n.duration, Duration::from_secs(10));
    }

    #[test]
    fn test_notification_with_source() {
        let n = Notification::info("T", "M").with_source("test-module");
        assert_eq!(n.source.as_deref(), Some("test-module"));
    }

    #[test]
    fn test_notification_dismissible() {
        let n = Notification::info("T", "M").with_dismissible(false);
        assert!(!n.dismissible);
    }

    #[test]
    fn test_notification_display_line() {
        let n = Notification::info("Test", "Hello world");
        let line = n.display_line();
        assert!(line.contains("INFO"));
        assert!(line.contains("Test"));
        assert!(line.contains("Hello world"));
    }

    #[test]
    fn test_notification_dismiss() {
        let mut n = Notification::info("T", "M");
        assert!(!n.dismissed);
        n.dismiss();
        assert!(n.dismissed);
    }

    #[test]
    fn test_notification_position_display() {
        assert_eq!(NotificationPosition::TopRight.to_string(), "top-right");
        assert_eq!(
            NotificationPosition::BottomCenter.to_string(),
            "bottom-center"
        );
    }

    #[test]
    fn test_notification_config_default() {
        let config = NotificationConfig::default();
        assert_eq!(config.max_visible, 5);
        assert!(config.enabled);
        assert_eq!(config.position, NotificationPosition::TopRight);
    }

    #[test]
    fn test_notification_config_builder() {
        let config = NotificationConfig::default()
            .with_max_visible(3)
            .with_position(NotificationPosition::BottomLeft)
            .with_duration(Duration::from_secs(10));
        assert_eq!(config.max_visible, 3);
        assert_eq!(config.position, NotificationPosition::BottomLeft);
        assert_eq!(config.default_duration, Duration::from_secs(10));
    }

    #[test]
    fn test_manager_add() {
        let mut mgr = NotificationManager::with_defaults();
        let id = mgr.info("Test", "Message");
        assert_eq!(mgr.active_count(), 1);
        assert!(mgr.find(id).is_some());
    }

    #[test]
    fn test_manager_add_multiple_types() {
        let mut mgr = NotificationManager::with_defaults();
        mgr.info("Info", "info msg");
        mgr.warning("Warn", "warn msg");
        mgr.error("Error", "error msg");
        mgr.success("Success", "success msg");
        assert_eq!(mgr.active_count(), 4);
    }

    #[test]
    fn test_manager_max_visible() {
        let config = NotificationConfig::default().with_max_visible(2);
        let mut mgr = NotificationManager::new(config);
        mgr.info("1", "one");
        mgr.info("2", "two");
        mgr.info("3", "three");
        // Should have at most 2 active.
        assert!(mgr.all().len() <= 2);
        // Oldest should be in history.
        assert!(!mgr.history().is_empty());
    }

    #[test]
    fn test_manager_dismiss() {
        let mut mgr = NotificationManager::with_defaults();
        let id = mgr.info("Test", "Message");
        let dismissed = mgr.dismiss(id);
        assert!(dismissed);
        assert_eq!(mgr.active_count(), 0);
    }

    #[test]
    fn test_manager_dismiss_non_dismissible() {
        let mut mgr = NotificationManager::with_defaults();
        let n = Notification::info("T", "M").with_dismissible(false);
        let id = n.id;
        mgr.add(n);
        let dismissed = mgr.dismiss(id);
        assert!(!dismissed);
        assert_eq!(mgr.active_count(), 1);
    }

    #[test]
    fn test_manager_dismiss_all() {
        let mut mgr = NotificationManager::with_defaults();
        mgr.info("1", "one");
        mgr.info("2", "two");
        mgr.info("3", "three");
        mgr.dismiss_all();
        assert_eq!(mgr.active_count(), 0);
        assert_eq!(mgr.history().len(), 3);
    }

    #[test]
    fn test_manager_clear() {
        let mut mgr = NotificationManager::with_defaults();
        mgr.info("1", "one");
        mgr.dismiss_all();
        mgr.clear();
        assert!(mgr.all().is_empty());
        assert!(mgr.history().is_empty());
    }

    #[test]
    fn test_manager_has_active() {
        let mut mgr = NotificationManager::with_defaults();
        assert!(!mgr.has_active());
        mgr.info("Test", "Message");
        assert!(mgr.has_active());
    }

    #[test]
    fn test_manager_total_count() {
        let mut mgr = NotificationManager::with_defaults();
        mgr.info("1", "one");
        mgr.info("2", "two");
        assert_eq!(mgr.total_count(), 2);
    }

    #[test]
    fn test_notification_id_unique() {
        let a = Notification::info("A", "a");
        let b = Notification::info("B", "b");
        assert_ne!(a.id, b.id);
    }

    #[test]
    fn test_notification_id_display() {
        let n = Notification::info("T", "M");
        let display = n.id.to_string();
        assert!(display.starts_with("notif-"));
    }
}
