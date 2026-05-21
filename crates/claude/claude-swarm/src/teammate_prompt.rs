//! Teammate prompt construction.
//!
//! Builds the system prompt additions for a teammate agent,
//! including team context, role information, and coordination hints.

use crate::constants::TEAM_LEAD_NAME;
use crate::types::TeammateIdentity;

/// Prompt context for a teammate.
#[derive(Debug, Clone)]
pub struct TeammatePromptContext {
    /// The teammate's identity.
    pub identity: TeammateIdentity,
    /// The team's objective.
    pub objective: Option<String>,
    /// Other teammates' names.
    pub teammate_names: Vec<String>,
    /// Working directory.
    pub cwd: String,
}

impl TeammatePromptContext {
    /// Create a new prompt context.
    #[must_use]
    pub fn new(identity: TeammateIdentity, cwd: impl Into<String>) -> Self {
        Self {
            identity,
            objective: None,
            teammate_names: Vec::new(),
            cwd: cwd.into(),
        }
    }

    /// Set the team objective.
    #[must_use]
    pub fn with_objective(mut self, objective: impl Into<String>) -> Self {
        self.objective = Some(objective.into());
        self
    }

    /// Set the teammate names.
    #[must_use]
    pub fn with_teammates(mut self, names: Vec<String>) -> Self {
        self.teammate_names = names;
        self
    }
}

/// Build the system prompt addition for a teammate.
#[must_use]
pub fn build_teammate_prompt(ctx: &TeammatePromptContext) -> String {
    let mut prompt = String::new();

    // Role section.
    prompt.push_str("# Swarm Teammate\n\n");
    prompt.push_str(&format!("You are **{}** ", ctx.identity.name));
    if ctx.identity.is_lead {
        prompt.push_str("(team lead) ");
    }
    prompt.push_str(&format!("in team **{}**.\n\n", ctx.identity.team_name));

    // Lead info.
    if !ctx.identity.is_lead {
        prompt.push_str(&format!(
            "Your team lead is **{}** (ID: `{}`).\n",
            TEAM_LEAD_NAME, ctx.identity.lead_agent_id
        ));
        prompt.push_str("You should coordinate with the lead for major decisions.\n\n");
    }

    // Objective.
    if let Some(ref obj) = ctx.objective {
        prompt.push_str(&format!("## Team Objective\n{obj}\n\n"));
    }

    // Teammates.
    if !ctx.teammate_names.is_empty() {
        prompt.push_str("## Teammates\n");
        for name in &ctx.teammate_names {
            prompt.push_str(&format!("- {name}\n"));
        }
        prompt.push('\n');
    }

    // Backend info.
    prompt.push_str(&format!(
        "## Environment\n- Backend: {}\n- Working directory: `{}`\n",
        ctx.identity.backend_type, ctx.cwd
    ));

    prompt
}

/// Build a short role description for a teammate.
#[must_use]
pub fn build_role_description(identity: &TeammateIdentity) -> String {
    if identity.is_lead {
        format!(
            "Team lead of '{}' (ID: {})",
            identity.team_name, identity.agent_id
        )
    } else {
        format!(
            "Worker '{}' in team '{}' (lead: {})",
            identity.name, identity.team_name, identity.lead_agent_id
        )
    }
}

/// Build a coordination hint for the teammate.
#[must_use]
pub fn build_coordination_hint(identity: &TeammateIdentity) -> String {
    if identity.is_lead {
        "You are the team lead. Coordinate tasks among your teammates and handle permission requests.".to_owned()
    } else {
        format!(
            "You are a worker agent. Report progress to the lead ('{}') and request permissions when needed.",
            TEAM_LEAD_NAME
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::BackendType;

    fn test_identity(is_lead: bool) -> TeammateIdentity {
        TeammateIdentity {
            agent_id: "a1".to_owned(),
            name: if is_lead {
                "lead".to_owned()
            } else {
                "worker-1".to_owned()
            },
            team_name: "test-team".to_owned(),
            is_lead,
            lead_agent_id: "lead-123".to_owned(),
            backend_type: BackendType::InProcess,
        }
    }

    #[test]
    fn build_teammate_prompt_worker() {
        let ctx = TeammatePromptContext::new(test_identity(false), "/tmp/project")
            .with_objective("Fix all bugs")
            .with_teammates(vec!["worker-2".to_owned()]);

        let prompt = build_teammate_prompt(&ctx);
        assert!(prompt.contains("worker-1"));
        assert!(prompt.contains("test-team"));
        assert!(prompt.contains("Fix all bugs"));
        assert!(prompt.contains("worker-2"));
        assert!(prompt.contains("in_process"));
    }

    #[test]
    fn build_teammate_prompt_lead() {
        let ctx = TeammatePromptContext::new(test_identity(true), "/tmp/project");

        let prompt = build_teammate_prompt(&ctx);
        assert!(prompt.contains("team lead"));
        assert!(prompt.contains("test-team"));
    }

    #[test]
    fn build_role_description_lead() {
        let desc = build_role_description(&test_identity(true));
        assert!(desc.contains("Team lead"));
        assert!(desc.contains("test-team"));
    }

    #[test]
    fn build_role_description_worker() {
        let desc = build_role_description(&test_identity(false));
        assert!(desc.contains("Worker"));
        assert!(desc.contains("worker-1"));
    }

    #[test]
    fn build_coordination_hint_lead() {
        let hint = build_coordination_hint(&test_identity(true));
        assert!(hint.contains("team lead"));
        assert!(hint.contains("permission requests"));
    }

    #[test]
    fn build_coordination_hint_worker() {
        let hint = build_coordination_hint(&test_identity(false));
        assert!(hint.contains("worker"));
        assert!(hint.contains("permissions"));
    }

    #[test]
    fn prompt_context_builder() {
        let ctx = TeammatePromptContext::new(test_identity(false), "/tmp")
            .with_objective("Build feature X")
            .with_teammates(vec!["a".to_owned(), "b".to_owned()]);

        assert_eq!(ctx.objective.as_deref(), Some("Build feature X"));
        assert_eq!(ctx.teammate_names.len(), 2);
        assert_eq!(ctx.cwd, "/tmp");
    }

    #[test]
    fn prompt_without_optional_fields() {
        let ctx = TeammatePromptContext::new(test_identity(false), "/tmp");
        let prompt = build_teammate_prompt(&ctx);
        assert!(!prompt.contains("## Team Objective"));
        assert!(!prompt.contains("## Teammates"));
    }
}
