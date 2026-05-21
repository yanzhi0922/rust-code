//! Agent and task commands: `/fork`, `/peers`.

use claude_config::RuntimeConfig;
use claude_session::SessionStore;
use claude_tools::runtime_plan_mode::copy_plan_mode_state_for_fork;
use uuid::Uuid;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ForkCommandOutcome {
    pub outputs: Vec<String>,
    pub next_session_id: Option<Uuid>,
}

/// Dispatch `/fork` — branch the current conversation into a new session.
pub fn dispatch_fork(
    input: &str,
    config: &RuntimeConfig,
    store: &SessionStore,
) -> ForkCommandOutcome {
    let requested_title = input
        .trim()
        .strip_prefix("/fork")
        .unwrap_or_default()
        .trim();

    let source_title = store
        .get_session_summary(config.session_id)
        .map(|summary| summary.title)
        .unwrap_or_else(|_| {
            config
                .session_name
                .clone()
                .unwrap_or_else(|| "Branched conversation".to_owned())
        });
    let target_title = derive_branch_title(&source_title, requested_title);
    let target_session_id = Uuid::new_v4();

    match store.fork_session_from_source(config.session_id, target_session_id, Some(&target_title))
    {
        Ok(summary) => {
            let mut outputs = vec![format!(
                "Branched conversation into session {} ({})",
                summary.session_id, summary.title
            )];
            match copy_plan_mode_state_for_fork(
                store,
                &config.paths,
                config.session_id,
                target_session_id,
            ) {
                Ok(Some(state)) => {
                    if let Some(plan_file_path) = state.plan_file_path {
                        outputs.push(format!(
                            "Copied plan mode state into {}",
                            plan_file_path.display()
                        ));
                    }
                }
                Ok(None) => {}
                Err(error) => outputs.push(format!(
                    "Warning: branched conversation, but failed to copy plan state: {error:#}"
                )),
            }

            ForkCommandOutcome {
                outputs,
                next_session_id: Some(target_session_id),
            }
        }
        Err(error) => ForkCommandOutcome {
            outputs: vec![format!("Failed to branch conversation: {error:#}")],
            next_session_id: None,
        },
    }
}

fn derive_branch_title(source_title: &str, requested_title: &str) -> String {
    let base = if requested_title.trim().is_empty() {
        source_title.trim()
    } else {
        requested_title.trim()
    };

    if base.contains("(Branch") {
        base.to_owned()
    } else {
        format!("{base} (Branch)")
    }
}

/// Dispatch `/peers` — list peer agents in the current swarm.
pub fn render_peers(config: &RuntimeConfig) {
    println!("Peer agents:");
    println!("  session: {}", config.session_id);
    println!("  self:    leader (single-agent mode)");
    println!("  peers:   (none — single-agent mode)");
    println!("  Tip: use swarm mode to enable multi-agent collaboration.");
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use claude_config::{ProviderOverrides, RuntimeOverrides, load_runtime_config};
    use claude_core::{ConversationEntry, InputFormat, OutputFormat, PermissionMode};
    use claude_tools::plan_mode::PlanModeRuntime;
    use claude_tools::runtime_plan_mode::RuntimePlanModeController;
    use tempfile::tempdir;

    fn build_test_config() -> (RuntimeConfig, SessionStore) {
        let temp = tempdir().expect("tempdir should work");
        let root = temp.keep();
        let config = load_runtime_config(
            Some(root.clone()),
            Some(root.join(".remote-code-rust")),
            None,
            PermissionMode::Default,
            InputFormat::Text,
            OutputFormat::Text,
            false,
            false,
            false,
            false,
            8,
            ProviderOverrides {
                provider: Some("glm-coding".to_owned()),
                base_url: Some("https://open.bigmodel.cn/api/anthropic".to_owned()),
                api_key: Some("secret".to_owned()),
                model: Some("glm-5.1".to_owned()),
                protocol: Some(claude_core::ProviderProtocol::Anthropic),
            },
            RuntimeOverrides::default(),
        )
        .expect("config should load");
        let store = SessionStore::open(config.paths.clone()).expect("store should open");
        (config, store)
    }

    fn seed_session(store: &SessionStore, config: &RuntimeConfig, title: &str) {
        store
            .ensure_session(
                config.session_id,
                &config.cwd,
                &config.provider.name,
                config.provider.model.as_deref(),
                Some(title),
            )
            .expect("session should exist");
        store
            .append_conversation_entry(config.session_id, &ConversationEntry::system("system"))
            .expect("system prompt should append");
        store
            .append_conversation_entry(config.session_id, &ConversationEntry::user("hello"))
            .expect("user message should append");
    }

    #[test]
    fn fork_without_title_branches_current_conversation() {
        let (config, store) = build_test_config();
        seed_session(&store, &config, "source");

        let outcome = dispatch_fork("/fork", &config, &store);

        let next_session_id = outcome
            .next_session_id
            .expect("fork should return next session id");
        let summary = store
            .get_session_summary(next_session_id)
            .expect("forked summary should load");
        assert_eq!(summary.parent_session_id, Some(config.session_id));
        assert_eq!(summary.title, "source (Branch)");
        assert!(outcome.outputs[0].contains("Branched conversation into session"));
    }

    #[test]
    fn fork_with_title_copies_transcript_into_new_session() {
        let (config, store) = build_test_config();
        seed_session(&store, &config, "source");

        let outcome = dispatch_fork("/fork implement auth module", &config, &store);

        let next_session_id = outcome
            .next_session_id
            .expect("fork should return next session id");
        let summary = store
            .get_session_summary(next_session_id)
            .expect("forked summary should load");
        assert_eq!(summary.parent_session_id, Some(config.session_id));
        assert_eq!(summary.title, "implement auth module (Branch)");
        let source_conversation = store
            .load_conversation(config.session_id)
            .expect("source conversation should load");
        let target_conversation = store
            .load_conversation(next_session_id)
            .expect("forked conversation should load");
        assert_eq!(target_conversation.len(), source_conversation.len());
        for (target, source) in target_conversation.iter().zip(source_conversation.iter()) {
            assert_eq!(target.role, source.role);
            assert_eq!(target.text, source.text);
            assert_eq!(target.tool_call_id, source.tool_call_id);
            assert_eq!(target.name, source.name);
            assert_eq!(target.is_error, source.is_error);
        }
    }

    #[test]
    fn fork_copies_plan_mode_state_with_new_slug() {
        let (config, store) = build_test_config();
        seed_session(&store, &config, "source");
        let controller = RuntimePlanModeController::load(&config, &store).expect("controller");
        controller
            .enter_plan_mode("audit runtime")
            .expect("enter plan mode");
        let source_state = controller.snapshot_state();
        let source_plan_path = source_state
            .plan_file_path
            .clone()
            .expect("source plan path");
        fs::write(&source_plan_path, "# Plan\n- audit runtime\n").expect("plan should write");

        let outcome = dispatch_fork("/fork runtime audit", &config, &store);

        let next_session_id = outcome
            .next_session_id
            .expect("fork should return next session id");
        let target_state = store
            .load_plan_mode_state(next_session_id)
            .expect("target state should load")
            .expect("target state should exist");
        assert_eq!(target_state.parent_session_id, Some(config.session_id));
        assert_ne!(target_state.plan_slug, source_state.plan_slug);
        let target_plan_path = target_state
            .plan_file_path
            .expect("target plan path should exist");
        assert_eq!(
            fs::read_to_string(target_plan_path).expect("target plan should read"),
            "# Plan\n- audit runtime\n"
        );
    }

    #[test]
    fn peers_shows_single_agent_mode() {
        let (config, _store) = build_test_config();
        render_peers(&config);
    }

    #[test]
    fn peers_displays_session_id() {
        let (config, _store) = build_test_config();
        render_peers(&config);
    }
}
