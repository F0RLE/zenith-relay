use std::{env, time::Instant};
use tauri::{Emitter, Manager, RunEvent, WindowEvent};

use crate::{
    local_pool, platform,
    tray::{build_tray, close_main_window, AppState},
};

mod client;
mod commands;
mod models;
mod top_up;

use commands::*;
pub fn run() {
    let started = Instant::now();
    let start_in_tray = env::args().any(|arg| arg == "--tray");
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            let shown_at = Instant::now();
            crate::tray::show_main_window(app);
            if let Some(state) = app.try_state::<local_pool::DesktopState>() {
                let _ = state.record_performance(
                    "window",
                    shown_at.elapsed().as_secs_f64() * 1_000.0,
                    Some("warm"),
                );
            }
        }))
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(AppState::new())
        .setup(move |app| setup_desktop(app, started, start_in_tray))
        .on_window_event(on_main_window_event)
        .invoke_handler(desktop_commands())
        .build(tauri::generate_context!())
        .expect("failed to build Zenith Relay");

    app.run(on_app_event);
}

fn setup_desktop(
    app: &mut tauri::App<tauri::Wry>,
    started: Instant,
    start_in_tray: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let handle = app.handle().clone();
    if let Err(error) = platform::resolve_codex_home() {
        crate::diagnostics::record_error(
            "desktop-startup",
            Some("codex_home_unavailable"),
            &error,
            &[],
        );
        return Err(std::io::Error::other(error).into());
    }
    let relay_root = match platform::relay_dir(&handle) {
        Ok(root) => root,
        Err(error) => {
            crate::diagnostics::record_error(
                "desktop-startup",
                Some("relay_path_unavailable"),
                &error,
                &[],
            );
            return Err(std::io::Error::other(error).into());
        }
    };
    crate::diagnostics::initialize(&relay_root);
    let relay_state = match local_pool::initialize(&handle) {
        Ok(state) => state,
        Err(error) => {
            crate::diagnostics::record_error(
                "desktop-startup",
                Some("local_pool_initialize_failed"),
                &error.message,
                &[],
            );
            return Err(std::io::Error::other(error.to_string()).into());
        }
    };
    app.manage(relay_state);
    local_pool::start_client_auth_watchdog(handle.clone());
    let native_startup_ms = started.elapsed().as_secs_f64() * 1_000.0;
    let relay_state = app.state::<local_pool::DesktopState>();
    let _ = relay_state.record_performance("native_startup", native_startup_ms, Some("cold"));
    if !start_in_tray {
        crate::tray::create_main_window(&handle)?;
        let window_ms = started.elapsed().as_secs_f64() * 1_000.0;
        let _ = relay_state.record_performance("window", window_ms, Some("cold"));
    }
    local_pool::background::start(handle.clone());
    let state = app.state::<AppState>();
    build_tray(&handle, &state)?;
    crate::portable_update::acknowledge_startup();
    tauri::async_runtime::spawn(async move {
        let state = handle.state::<local_pool::DesktopState>();
        let _ = local_pool::commands::gateway::lifecycle::start_if_enabled(&state).await;
        // Auto-start runs after the WebView is created. Notify every
        // renderer once the runtime exists so an initial snapshot that
        // raced startup cannot leave the pool UI with an empty order.
        let _ = handle.emit("zenith-state-changed", ());
        crate::tray::refresh_tray(&handle).await;
    });
    Ok(())
}

fn on_main_window_event(window: &tauri::Window<tauri::Wry>, event: &WindowEvent) {
    if !crate::tray::is_main_window_label(window.label()) {
        return;
    }
    if let WindowEvent::CloseRequested { api, .. } = event {
        api.prevent_close();
        close_main_window(window.app_handle());
    }
}

fn on_app_event(app_handle: &tauri::AppHandle<tauri::Wry>, event: RunEvent) {
    match event {
        RunEvent::ExitRequested { api, code, .. } => {
            let state = app_handle.state::<AppState>();
            let prevent = code.is_none() && state.should_prevent_exit();
            crate::diagnostics::breadcrumb(
                "desktop",
                if prevent {
                    "exit_requested_prevented"
                } else {
                    "exit_requested"
                },
                &[(
                    "code",
                    code.map_or_else(|| "none".to_string(), |value| value.to_string()),
                )],
            );
            if prevent {
                api.prevent_exit();
            }
        }
        RunEvent::Exit => crate::diagnostics::shutdown(),
        _ => {}
    }
}

fn desktop_commands() -> impl Fn(tauri::ipc::Invoke<tauri::Wry>) -> bool + Send + Sync + 'static {
    tauri::generate_handler![
        get_state,
        get_platform,
        get_system_locale,
        crate::portable_update::get_portable_update_target,
        crate::portable_update::install_portable_update,
        get_saved_key_models,
        top_up::create_top_up_intent_and_open,
        top_up::create_saved_top_up_intent_and_open,
        top_up::prepare_top_up_amount,
        save_key,
        activate_ready_api_profile,
        deactivate_ready_api_profile,
        reset_key,
        launch_saved_codex,
        open_api_key_page,
        top_up::open_top_up_url,
        local_pool::commands::state::get_local_pool_state,
        local_pool::commands::state::refresh_local_pricing_catalog,
        local_pool::commands::state::get_local_runtime_state,
        local_pool::commands::state::get_local_runtime_order,
        local_pool::commands::state::record_local_performance_sample,
        local_pool::commands::connections::edit::create_local_source,
        local_pool::commands::connections::edit::update_local_source,
        local_pool::commands::connections::edit::set_local_source_enabled,
        local_pool::commands::connections::edit::delete_local_source,
        local_pool::commands::connections::edit::rotate_local_source_key,
        local_pool::commands::connections::inspect::test_local_source,
        local_pool::commands::connections::inspect::probe_local_source,
        local_pool::commands::connections::inspect::refresh_local_source_data,
        local_pool::commands::connections::inspect::get_local_source_stats,
        local_pool::commands::remote_server::session::get_remote_source_stats,
        local_pool::accounts::import_orchestrator::start_local_account_import,
        local_pool::accounts::import_orchestrator::preview_local_account_import_files,
        local_pool::accounts::import_orchestrator::preview_current_codex_account_import,
        local_pool::accounts::import_orchestrator::current_chatgpt_profile_available,
        local_pool::accounts::import_orchestrator::resume_local_account_import,
        local_pool::accounts::import_orchestrator::prepare_local_account_import,
        local_pool::accounts::import_orchestrator::cancel_local_account_import,
        local_pool::accounts::import_orchestrator::confirm_local_account_import,
        local_pool::accounts::export_ops::reveal_local_account_identity,
        local_pool::accounts::login::reveal_local_account_login,
        local_pool::accounts::login::update_local_account_login,
        local_pool::accounts::login::preview_totp_code,
        local_pool::accounts::export_ops::export_local_accounts,
        local_pool::accounts::mutations::edit::update_local_account,
        local_pool::accounts::mutations::edit::set_local_account_proxy,
        local_pool::commands::proxies::get_local_proxy_pool,
        local_pool::commands::proxies::check_local_stored_proxy,
        local_pool::commands::proxies::import_local_proxy_pool,
        local_pool::commands::proxies::delete_local_stored_proxy,
        local_pool::commands::proxies::delete_local_stored_proxies,
        local_pool::commands::proxies::assign_local_stored_proxy,
        local_pool::commands::proxies::set_local_stored_proxy_accounts,
        local_pool::commands::proxies::assign_free_local_account_proxies,
        local_pool::accounts::mutations::edit::set_local_account_enabled,
        local_pool::accounts::mutations::delete::delete_local_account,
        local_pool::accounts::mutations::delete::delete_local_accounts,
        local_pool::accounts::quota_refresh::refresh_local_account_quota,
        local_pool::accounts::quota_refresh::force_refresh_local_account_credentials,
        local_pool::accounts::quota_refresh::refresh_all_local_account_quotas,
        local_pool::accounts::reset_credits::consume_local_reset_credit,
        local_pool::commands::oauth::start_codex_oauth,
        local_pool::commands::oauth::resume_codex_oauth,
        local_pool::commands::oauth::get_codex_oauth_status,
        local_pool::commands::oauth::submit_codex_oauth_callback,
        local_pool::commands::oauth::cancel_codex_oauth,
        local_pool::commands::oauth::complete_codex_oauth,
        local_pool::commands::automations::create_quota_wake_automation,
        local_pool::commands::automations::update_quota_wake_automation,
        local_pool::commands::automations::set_quota_wake_automation_enabled,
        local_pool::commands::automations::delete_quota_wake_automation,
        local_pool::commands::automations::run_due_quota_wake_confirmations,
        local_pool::commands::automations::test_quota_wake_automation,
        local_pool::commands::pool::set_local_pool_membership,
        local_pool::commands::pool::set_local_model_enabled,
        local_pool::commands::pool::set_local_model_price,
        local_pool::commands::pool::set_local_model_reasoning,
        local_pool::commands::pool::set_local_model_service_tier,
        local_pool::commands::pool::set_local_model_display_order,
        local_pool::commands::pool::export_local_configuration_preset,
        local_pool::commands::pool::preview_local_configuration_preset,
        local_pool::commands::pool::apply_local_configuration_preset,
        local_pool::commands::pool::update_local_routing,
        local_pool::commands::gateway::lifecycle::start_local_gateway,
        local_pool::commands::gateway::lifecycle::stop_local_gateway,
        local_pool::commands::gateway::lifecycle::restart_local_gateway,
        local_pool::commands::gateway::lifecycle::update_local_gateway_port,
        local_pool::commands::gateway::lifecycle::reveal_local_gateway_api_key,
        local_pool::commands::gateway::lifecycle::rotate_local_gateway_api_key,
        local_pool::commands::gateway::settings::set_local_common_proxy,
        local_pool::commands::gateway::settings::set_local_account_proxy_required,
        local_pool::commands::gateway::settings::set_local_codex_background_tasks,
        local_pool::commands::gateway::settings::set_local_tool_policy,
        local_pool::commands::gateway::settings::set_local_chatgpt_retry_until_available,
        local_pool::commands::gateway::settings::set_local_codex_websockets,
        local_pool::commands::gateway::settings::set_codex_profile_websockets,
        local_pool::commands::gateway::diagnostics::diagnose_local_gateway,
        local_pool::commands::usage::get_local_usage_page,
        local_pool::commands::usage::get_local_cache_sessions,
        local_pool::commands::usage::clear_local_usage,
        local_pool::commands::profiles::gateway::update_chatgpt_interface_quota_reserve,
        local_pool::commands::profiles::gateway::sync_codex_default_service_tier,
        local_pool::commands::profiles::gateway::attach_codex_to_local_gateway,
        local_pool::commands::profiles::gateway::attach_codex_to_remote_gateway,
        local_pool::commands::profiles::actions::restore_codex_profile,
        local_pool::commands::profiles::actions::list_codex_profile_snapshots,
        local_pool::commands::profiles::actions::create_codex_profile_snapshot,
        local_pool::commands::profiles::actions::restore_full_codex_profile_snapshot,
        local_pool::commands::profiles::actions::delete_codex_profile_snapshot,
        local_pool::commands::profiles::actions::stop_managed_codex_profile,
        local_pool::commands::profiles::actions::launch_managed_codex_profile,
        local_pool::commands::profiles::actions::attach_codex_to_account,
        local_pool::commands::profiles::actions::launch_codex_account,
        local_pool::commands::profiles::actions::launch_codex_source,
        local_pool::commands::profiles::actions::list_codex_account_bindings,
        local_pool::commands::profiles::actions::restore_codex_account_profile,
        local_pool::commands::opencode::get_opencode_config_status,
        local_pool::commands::opencode::create_opencode_snapshot,
        local_pool::commands::opencode::connect_opencode_to_local_gateway,
        local_pool::commands::opencode::launch_opencode_source,
        local_pool::commands::opencode::restart_opencode_app,
        local_pool::commands::opencode::restore_opencode_config,
        local_pool::commands::recovery::get_relay_storage_info,
        local_pool::commands::recovery::open_relay_folder,
        local_pool::commands::recovery::reset_local_pool_data,
        local_pool::commands::recovery::export_usage,
        local_pool::commands::recovery::export_support_bundle,
        local_pool::commands::recovery::preview_support_bundle,
        crate::diagnostics::record_frontend_diagnostic,
        crate::diagnostics::get_diagnostic_paths,
        crate::diagnostics::get_diagnostic_settings,
        crate::diagnostics::set_diagnostic_debug_mode,
        local_pool::commands::remote_server::session::connect_remote_server,
        local_pool::commands::remote_server::session::get_remote_server_state,
        local_pool::commands::remote_server::session::get_remote_runtime_order,
        local_pool::commands::remote_server::session::get_remote_server_usage,
        local_pool::commands::remote_server::gateway_key::reveal_remote_gateway_api_key,
        local_pool::commands::remote_server::gateway_key::rotate_remote_gateway_api_key,
        local_pool::commands::remote_server::preset::export_remote_configuration_preset,
        local_pool::commands::remote_server::preset::preview_remote_configuration_preset,
        local_pool::commands::remote_server::preset::apply_remote_configuration_preset,
        local_pool::commands::remote_server::session::diagnose_remote_gateway,
        local_pool::commands::remote_server::accounts::reveal_remote_account_identity,
        local_pool::commands::remote_server::accounts::export_remote_accounts,
        local_pool::commands::remote_server::session::refresh_remote_server_capabilities,
        local_pool::commands::remote_server::accounts::get_remote_linked_account_count,
        local_pool::commands::remote_server::session::disconnect_remote_server,
        local_pool::commands::remote_server::accounts::prepare_remote_server_deployment,
        local_pool::commands::remote_server::accounts::preview_remote_account_import_files,
        local_pool::commands::remote_server::accounts::move_local_accounts_to_remote,
        local_pool::commands::remote_server::accounts::return_remote_account_to_local,
        local_pool::commands::remote_server::accounts::force_activate_remote_account_locally,
        local_pool::commands::remote_server::actions::execute_remote_server_action
    ]
}

#[cfg(test)]
mod tests;
