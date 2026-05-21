use super::*;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Install a panic hook that logs panics before delegating to the default handler.
    // This ensures panics are captured in the log even if they don't propagate to the UI.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!("Unhandled panic in GUI: {info}");
        default_hook(info);
    }));

    let runtime_state = build_runtime_state().unwrap_or_else(|error| {
        panic!("failed to initialize remote-code-gui runtime: {error:#}");
    });
    let pending_permissions = Arc::new(Mutex::new(HashMap::new()));
    let pending_codex_permissions = Arc::new(Mutex::new(HashMap::new()));
    let pending_roo_permissions = Arc::new(Mutex::new(HashMap::new()));
    let pending_claude_permissions = Arc::new(Mutex::new(HashMap::new()));
    let running_prompts = Arc::new(Mutex::new(HashMap::new()));
    let active_codex_adapters = Arc::new(Mutex::new(HashMap::new()));
    let active_roo_adapters = Arc::new(Mutex::new(HashMap::new()));
    let active_claude_adapters = Arc::new(Mutex::new(HashMap::new()));

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .setup(|app| {
            crate::mobile::register_mobile_plugins(app.handle());
            crate::remote_runner::start_remote_service(app.handle().clone());
            Ok(())
        })
        .manage(AppState {
            runtime: Mutex::new(runtime_state),
            pending_permissions,
            pending_codex_permissions,
            pending_roo_permissions,
            pending_claude_permissions,
            running_prompts,
            active_codex_adapters,
            active_roo_adapters,
            active_claude_adapters,
        })
        .manage(crate::quic_bridge::QuicBridgeState::new())
        .invoke_handler(tauri::generate_handler![
            session_commands::init_app,
            session_commands::list_sessions,
            session_commands::get_session_conversation,
            session_commands::get_session_tasks,
            session_commands::send_prompt,
            session_commands::cancel_prompt,
            provider_commands::get_provider_info,
            provider_commands::get_runtime_status,
            provider_commands::run_doctor_report,
            session_commands::export_session_bundle,
            mcp_commands::list_mcp_servers,
            mcp_commands::list_runtime_mcp_inventory,
            mcp_commands::save_mcp_server,
            mcp_commands::toggle_mcp_server,
            mcp_commands::remove_mcp_server,
            mcp_commands::reset_mcp_servers,
            session_commands::create_session,
            provider_commands::get_settings,
            codex_commands::codex_list_threads,
            codex_commands::codex_read_thread,
            codex_commands::codex_thread_start,
            codex_commands::codex_resume_thread,
            codex_commands::codex_resume_thread_native,
            codex_commands::codex_fork_thread,
            codex_commands::codex_fork_thread_native,
            codex_commands::codex_archive_thread,
            codex_commands::codex_unarchive_thread,
            codex_commands::codex_thread_unsubscribe,
            codex_commands::codex_thread_elicitation_increment,
            codex_commands::codex_thread_elicitation_decrement,
            codex_commands::codex_thread_set_name,
            codex_commands::codex_thread_metadata_update,
            codex_commands::codex_current_thread_id,
            codex_commands::codex_thread_goal_set,
            codex_commands::codex_thread_goal_get,
            codex_commands::codex_thread_goal_clear,
            codex_commands::codex_thread_compact_start,
            codex_commands::codex_thread_shell_command,
            codex_commands::codex_thread_background_terminals_clean,
            codex_commands::codex_thread_guardian_denied_action_approve,
            codex_commands::codex_thread_rollback,
            codex_commands::codex_thread_turns_list,
            codex_commands::codex_thread_loaded_list,
            codex_commands::codex_thread_inject_items,
            codex_commands::codex_turn_start,
            codex_commands::codex_turn_steer,
            codex_commands::codex_turn_interrupt,
            codex_commands::codex_model_list,
            codex_commands::codex_collaboration_mode_list,
            codex_commands::codex_experimental_feature_list,
            codex_commands::codex_experimental_feature_set,
            codex_commands::codex_account_read,
            codex_commands::codex_account_login,
            codex_commands::codex_account_login_cancel,
            codex_commands::codex_account_logout,
            codex_commands::codex_account_rate_limits_read,
            codex_commands::codex_account_add_credits_nudge,
            codex_commands::codex_apps_list,
            codex_commands::codex_exec,
            codex_commands::codex_app_server_request,
            codex_commands::codex_exec_write,
            codex_commands::codex_exec_terminate,
            codex_commands::codex_exec_resize,
            codex_commands::codex_windows_sandbox_setup_start,
            codex_commands::codex_mcp_refresh,
            codex_commands::codex_mcp_status,
            codex_commands::codex_mcp_read_resource,
            codex_commands::codex_mcp_call_tool,
            codex_commands::codex_mcp_oauth_login,
            codex_commands::codex_skills_list,
            codex_commands::codex_skills_config_write,
            codex_commands::codex_plugin_list,
            codex_commands::codex_plugin_read,
            codex_commands::codex_plugin_install,
            codex_commands::codex_plugin_uninstall,
            codex_commands::codex_marketplace_add,
            codex_commands::codex_marketplace_remove,
            codex_commands::codex_marketplace_upgrade,
            codex_commands::codex_review_start,
            codex_commands::codex_read_config,
            codex_commands::codex_config_requirements_read,
            codex_commands::codex_external_agent_config_detect,
            codex_commands::codex_external_agent_config_import,
            codex_commands::codex_write_config_value,
            codex_commands::codex_write_config_batch,
            codex_commands::codex_upload_feedback,
            codex_commands::codex_set_thread_memory_mode,
            codex_commands::codex_reset_memories,
            codex_commands::codex_realtime_start,
            codex_commands::codex_realtime_append_audio,
            codex_commands::codex_realtime_append_text,
            codex_commands::codex_realtime_stop,
            codex_commands::codex_realtime_voices_list,
            codex_commands::codex_device_key_create,
            codex_commands::codex_device_key_public,
            codex_commands::codex_device_key_sign,
            codex_commands::codex_fs_read_file,
            codex_commands::codex_fs_write_file,
            codex_commands::codex_fs_create_directory,
            codex_commands::codex_fs_get_metadata,
            codex_commands::codex_fs_read_directory,
            codex_commands::codex_fs_remove,
            codex_commands::codex_fs_copy,
            codex_commands::codex_fs_watch,
            codex_commands::codex_fs_unwatch,
            codex_commands::codex_fuzzy_file_search,
            codex_commands::codex_fuzzy_file_search_session_start,
            codex_commands::codex_fuzzy_file_search_session_update,
            codex_commands::codex_fuzzy_file_search_session_stop,
            codex_commands::codex_adapter_stop,
            codex_commands::codex_adapter_restart,
            provider_commands::update_provider,
            project_commands::list_projects,
            project_commands::add_project,
            project_commands::remove_project,
            session_commands::archive_session,
            session_commands::restore_session,
            session_commands::list_archived_sessions,
            provider_commands::list_provider_configs,
            provider_commands::save_provider_config,
            provider_commands::delete_provider_config,
            provider_commands::set_active_provider,
            provider_commands::switch_profile,
            permission_commands::resolve_permission_request,
            permission_commands::resolve_roo_permission_request,
            permission_commands::resolve_claude_permission_request,
            project_commands::pick_folder,
            agent_routing::list_available_agents,
            agent_routing::install_agent,
            agent_routing::uninstall_agent,
            provider_commands::transcribe_audio,
            crate::mobile::mobile_is_mobile,
            crate::mobile::mobile_haptic_notification,
            crate::mobile::mobile_biometric_check_availability,
            crate::mobile::mobile_biometric_authenticate,
            crate::mobile::mobile_secure_store_get,
            crate::mobile::mobile_secure_store_set,
            crate::mobile::mobile_secure_store_remove,
            crate::mobile::mobile_download_artifact,
            crate::mobile::mobile_share_file,
            crate::mobile::mobile_push_request_permission,
            crate::mobile::mobile_push_show,
            crate::mobile::mobile_push_get_token,
            crate::mobile::mobile_push_register_token,
            crate::mobile::mobile_check_file_downloaded,
            crate::mobile::mobile_read_downloaded_text,
            crate::mobile::mobile_delete_downloaded_file,
            crate::mobile::mobile_list_downloaded_files,
            crate::quic_bridge::quic_connect,
            crate::quic_bridge::quic_send_command,
            crate::quic_bridge::quic_disconnect,
            crate::quic_bridge::quic_state,
            crate::remote_runner::remote_get_status,
            crate::remote_runner::remote_set_password,
            crate::remote_runner::remote_set_username,
            crate::remote_runner::remote_set_credentials,
            crate::remote_runner::remote_get_username,
            crate::remote_runner::remote_get_connection_info,
            crate::remote_runner::remote_set_connection,
            crate::remote_runner::remote_start_service,
            crate::remote_runner::remote_has_password
        ])
        .run(tauri::generate_context!())
        .unwrap_or_else(|error| panic!("error while running tauri application: {error}"));
}
