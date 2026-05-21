use serde::{Deserialize, Serialize};

use crate::PermissionRequest;
use crate::shell_rules::rule_matches_request;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleAction {
    Allow,
    Ask,
    Deny,
}

impl RuleAction {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Ask => "ask",
            Self::Deny => "deny",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleSource {
    Cli,
    Session,
    Project,
    User,
}

impl RuleSource {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cli => "cli",
            Self::Session => "session",
            Self::Project => "project",
            Self::User => "user",
        }
    }

    #[must_use]
    pub const fn all() -> [RuleSource; 4] {
        [Self::Cli, Self::Session, Self::Project, Self::User]
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceAwarePermissionRule {
    pub tool_pattern: String,
    pub action: RuleAction,
    pub source: RuleSource,
}

#[derive(Debug, Clone)]
pub struct RuleMatch {
    pub rule: SourceAwarePermissionRule,
}

#[derive(Debug, Clone, Default)]
pub struct LayeredRuleEngine {
    rules: Vec<SourceAwarePermissionRule>,
}

impl LayeredRuleEngine {
    #[must_use]
    pub fn new(mut rules: Vec<SourceAwarePermissionRule>) -> Self {
        rules.sort_by_key(|rule| source_priority(rule.source));
        Self { rules }
    }

    #[must_use]
    pub fn rules(&self) -> &[SourceAwarePermissionRule] {
        &self.rules
    }

    #[must_use]
    pub fn check(&self, request: &PermissionRequest) -> Option<RuleMatch> {
        self.rules.iter().find_map(|rule| {
            rule_matches_request(&rule.tool_pattern, request)
                .then(|| RuleMatch { rule: rule.clone() })
        })
    }
}

const fn source_priority(source: RuleSource) -> usize {
    match source {
        RuleSource::Cli => 0,
        RuleSource::Session => 1,
        RuleSource::Project => 2,
        RuleSource::User => 3,
    }
}

#[must_use]
pub fn summarize_rule_sources(rules: &[SourceAwarePermissionRule]) -> Vec<(RuleSource, usize)> {
    RuleSource::all()
        .into_iter()
        .filter_map(|source| {
            let count = rules.iter().filter(|rule| rule.source == source).count();
            (count > 0).then_some((source, count))
        })
        .collect()
}
