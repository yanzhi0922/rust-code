use claude_config::RuntimeConfig;
use claude_session::SessionStore;

pub fn render(config: &RuntimeConfig, store: &SessionStore) {
    println!("Session surface:");
    match store.get_session_summary(config.session_id) {
        Ok(summary) => {
            println!("  id:         {}", summary.session_id);
            println!("  title:      {}", summary.title);
            println!("  cwd:        {}", summary.cwd.display());
            println!("  transcript: {}", summary.transcript_path.display());
            println!("  updated:    {}", summary.updated_at);
            println!("  archived:   {}", summary.archived);
        }
        Err(error) => {
            println!("  summary:    unavailable ({error})");
        }
    }

    match store.load_resume_state(config.session_id) {
        Ok(Some(state)) => {
            if state.pending_tool_calls.is_empty() {
                println!("  resume:     none");
            } else {
                println!(
                    "  resume:     {} pending tool call(s)",
                    state.pending_tool_calls.len()
                );
            }
            if let Some(call) = state.pending_tool_calls.first() {
                println!("  next tool:  {}", call.name);
            }
        }
        Ok(None) => println!("  resume:     none"),
        Err(error) => println!("  resume:     unavailable ({error})"),
    }
}
