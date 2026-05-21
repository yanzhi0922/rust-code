use anyhow::Result;
use claude_core::ConversationRole;
use claude_session::{SessionStore, SessionSummary};
use serde::Serialize;
use uuid::Uuid;

use crate::cli::{ExportArgs, ExportFormat, SessionsCommand, SessionsStatsArgs};
use crate::conversation::truncate_preview;

#[derive(Debug, Clone, Serialize)]
struct SessionStatsRow {
    session_id: Uuid,
    title: String,
    provider_name: String,
    model: Option<String>,
    updated_at: String,
    archived: bool,
    total_events: usize,
    conversation_entries: usize,
    tool_call_count: usize,
    error_count: usize,
    input_tokens: u64,
    output_tokens: u64,
    last_stop_reason: Option<String>,
}

pub(crate) fn run_sessions(store: &SessionStore, command: Option<SessionsCommand>) -> Result<()> {
    match command.unwrap_or(SessionsCommand::List) {
        SessionsCommand::List => {
            let sessions = store.list_sessions()?;
            if sessions.is_empty() {
                println!("No sessions found.");
                return Ok(());
            }
            for session in sessions {
                println!(
                    "{}  {}  {}  {}",
                    session.session_id, session.updated_at, session.provider_name, session.title
                );
            }
            Ok(())
        }
        SessionsCommand::Show(args) => {
            let bundle = store.load_session_bundle(args.session_id)?;
            if args.json {
                println!("{}", serde_json::to_string_pretty(&bundle)?);
            } else {
                print_session_summary(&bundle.summary);
                println!("- transcript: {}", bundle.summary.transcript_path.display());
                println!("- events: {}", bundle.stats.total_events);
                println!("- messages: {}", bundle.stats.conversation_entries);
                println!(
                    "- usage: {} input / {} output",
                    bundle.stats.usage.input_tokens, bundle.stats.usage.output_tokens
                );
                if let Some(stop_reason) = &bundle.stats.last_stop_reason {
                    println!("- last stop reason: {stop_reason}");
                }
                if !bundle.conversation.is_empty() {
                    println!("\nRecent conversation:");
                    for entry in bundle.conversation.iter().rev().take(5).rev() {
                        println!(
                            "  {}: {}",
                            entry_role_label(&entry.role),
                            truncate_preview(&entry.history_text(), 120)
                        );
                    }
                }
            }
            Ok(())
        }
        SessionsCommand::Stats(args) => run_session_stats(store, args),
    }
}

pub(crate) fn run_export(store: &SessionStore, args: ExportArgs) -> Result<()> {
    let path = match args.format {
        ExportFormat::Ndjson => store.export_session(args.session_id, args.output)?,
        ExportFormat::Json => store.export_session_bundle_json(args.session_id, args.output)?,
    };
    println!("{}", path.display());
    Ok(())
}

pub(crate) fn print_session_summary(summary: &SessionSummary) {
    println!("Session {}", summary.session_id);
    println!("- title: {}", summary.title);
    println!("- cwd: {}", summary.cwd.display());
    println!("- provider: {}", summary.provider_name);
    println!(
        "- model: {}",
        summary.model.as_deref().unwrap_or("(missing)")
    );
    println!("- created: {}", summary.created_at);
    println!("- updated: {}", summary.updated_at);
}

fn entry_role_label(role: &ConversationRole) -> &'static str {
    match role {
        ConversationRole::System => "system",
        ConversationRole::User => "user",
        ConversationRole::Assistant => "assistant",
        ConversationRole::Tool => "tool",
    }
}

fn run_session_stats(store: &SessionStore, args: SessionsStatsArgs) -> Result<()> {
    let rows = collect_session_stats(store, args.session_id, args.limit.max(1))?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    if rows.is_empty() {
        println!("No session stats available.");
        return Ok(());
    }

    let total_input = rows.iter().map(|row| row.input_tokens).sum::<u64>();
    let total_output = rows.iter().map(|row| row.output_tokens).sum::<u64>();
    let total_tools = rows.iter().map(|row| row.tool_call_count).sum::<usize>();
    let total_errors = rows.iter().map(|row| row.error_count).sum::<usize>();
    println!(
        "Sessions: {}  input={}  output={}  tools={}  errors={}",
        rows.len(),
        total_input,
        total_output,
        total_tools,
        total_errors
    );
    for row in rows {
        println!(
            "{}  {}  {}  {}  in={} out={} tools={} err={} stop={}",
            row.session_id,
            row.updated_at,
            row.provider_name,
            row.title,
            row.input_tokens,
            row.output_tokens,
            row.tool_call_count,
            row.error_count,
            row.last_stop_reason.as_deref().unwrap_or("(none)")
        );
    }
    Ok(())
}

fn collect_session_stats(
    store: &SessionStore,
    session_id: Option<Uuid>,
    limit: usize,
) -> Result<Vec<SessionStatsRow>> {
    if let Some(session_id) = session_id {
        return Ok(vec![session_stats_row(store, session_id)?]);
    }

    store
        .list_sessions()?
        .into_iter()
        .take(limit)
        .map(|summary| session_stats_row(store, summary.session_id))
        .collect()
}

fn session_stats_row(store: &SessionStore, session_id: Uuid) -> Result<SessionStatsRow> {
    let bundle = store.load_session_bundle(session_id)?;
    Ok(SessionStatsRow {
        session_id: bundle.summary.session_id,
        title: bundle.summary.title,
        provider_name: bundle.summary.provider_name,
        model: bundle.summary.model,
        updated_at: bundle.summary.updated_at.to_rfc3339(),
        archived: bundle.summary.archived,
        total_events: bundle.stats.total_events,
        conversation_entries: bundle.stats.conversation_entries,
        tool_call_count: bundle.stats.tool_call_count,
        error_count: bundle.stats.error_count,
        input_tokens: bundle.stats.usage.input_tokens,
        output_tokens: bundle.stats.usage.output_tokens,
        last_stop_reason: bundle.stats.last_stop_reason,
    })
}

#[cfg(test)]
mod tests {
    use super::collect_session_stats;
    use claude_config::{ProviderOverrides, RuntimeOverrides, load_runtime_config};
    use claude_core::{ConversationEntry, InputFormat, OutputFormat, PermissionMode};
    use claude_session::SessionStore;
    use tempfile::tempdir;

    #[test]
    fn collect_session_stats_reports_usage_and_stop_reason() {
        let temp = tempdir().expect("tempdir should work");
        let config = load_runtime_config(
            Some(temp.path().to_path_buf()),
            Some(temp.path().join(".remote-code-rust")),
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
                provider: Some("glm".to_owned()),
                base_url: Some("https://open.bigmodel.cn/api/paas/v4".to_owned()),
                api_key: Some("secret".to_owned()),
                model: Some("glm-5.1".to_owned()),
                protocol: Some(claude_core::ProviderProtocol::OpenAi),
            },
            RuntimeOverrides::default(),
        )
        .expect("config should load");
        let store = SessionStore::open(config.paths.clone()).expect("store should open");
        let entry = ConversationEntry::assistant("done");
        store
            .ensure_session(
                config.session_id,
                &config.cwd,
                &config.provider.name,
                config.provider.model.as_deref(),
                Some("stats-test"),
            )
            .expect("session should exist");
        store
            .append_conversation_entry(config.session_id, &entry)
            .expect("conversation append should work");
        store
            .append_named_event(
                config.session_id,
                "result",
                serde_json::json!({
                    "stop_reason": "end_turn",
                    "usage": {"input_tokens": 10, "output_tokens": 5}
                }),
            )
            .expect("usage append should work");

        let rows = collect_session_stats(&store, Some(config.session_id), 10)
            .expect("stats collection should work");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].input_tokens, 10);
        assert_eq!(rows[0].output_tokens, 5);
        assert_eq!(rows[0].last_stop_reason.as_deref(), Some("end_turn"));
    }
}
