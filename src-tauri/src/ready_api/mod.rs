use std::{env, time::Instant};
use tauri::{Manager, RunEvent, WindowEvent};

use crate::{
    codex_config::ensure_provider_on_launch,
    local_pool, platform,
    tray::{build_tray, close_main_window, AppState},
};

mod client;
mod commands;
mod models;
mod top_up;

#[cfg(test)]
use client::*;
use commands::*;
#[cfg(test)]
use models::*;

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::{
        api_key_page_url, parse_model_ids, sanitize_api_error_message,
        top_up::{
            extract_top_up_start, extract_top_up_start_from_url, is_allowed_top_up_url,
            telegram_start_url, validate_top_up_amount_cents, MAX_AMOUNT_CENTS,
        },
        TopUpIntentData, UiState,
    };
    #[test]
    fn top_up_opener_allows_only_telegram_app_deep_link() {
        assert!(is_allowed_top_up_url(
            "tg://resolve?domain=zenith_service_bot&start=ztu_0123456789abcdef0123456789abcdef0123"
        ));
        assert!(!is_allowed_top_up_url(
            "https://t.me/zenith_service_bot?start=ztu_0123456789abcdef0123456789abcdef0123"
        ));
        assert!(!is_allowed_top_up_url(
            "tg://resolve?domain=other_bot&start=ztu_0123456789abcdef0123456789abcdef0123"
        ));
    }

    #[test]
    fn api_key_pages_are_fixed_to_known_providers() {
        assert_eq!(
            api_key_page_url("zenith"),
            Some("https://t.me/zenith_service_bot")
        );
        assert_eq!(
            api_key_page_url("openai"),
            Some("https://platform.openai.com/api-keys")
        );
        assert_eq!(
            api_key_page_url("openrouter"),
            Some("https://openrouter.ai/settings/keys")
        );
        assert_eq!(api_key_page_url("custom"), None);
    }

    #[test]
    fn ui_state_never_serializes_the_saved_api_key() {
        let value = serde_json::to_value(UiState {
            provider_active: true,
            codex_running: false,
            has_saved_api_key: true,
        })
        .unwrap();
        let rendered = value.to_string();
        assert_eq!(value["hasSavedApiKey"], true);
        assert!(!rendered.contains("savedApiKey"));
        assert!(!rendered.contains("api_key"));
    }

    #[test]
    fn model_catalog_returns_only_bounded_single_line_ids() {
        let models = parse_model_ids(
            br#"{"data":[{"id":"gpt-test"},{"id":"gpt test"},{"id":"gpt-test"},{"id":"bad\nmodel"}]}"#,
        )
        .unwrap();
        assert_eq!(models, ["gpt-test"]);
        assert!(parse_model_ids(br#"{"data":[]}"#).is_err());
        assert!(parse_model_ids(b"not-json").is_err());
    }

    #[test]
    fn top_up_start_payload_is_converted_to_app_deep_link() {
        assert_eq!(
            extract_top_up_start_from_url(
                "https://t.me/zenith_service_bot?start=ztu_0123456789abcdef0123456789abcdef0123"
            )
            .as_deref(),
            Some("ztu_0123456789abcdef0123456789abcdef0123")
        );
        assert_eq!(
            telegram_start_url("ztu_0123456789abcdef0123456789abcdef0123"),
            "tg://resolve?domain=zenith_service_bot&start=ztu_0123456789abcdef0123456789abcdef0123"
        );
    }

    #[test]
    fn top_up_start_payload_rejects_malformed_backend_values() {
        assert!(extract_top_up_start(TopUpIntentData {
            code: Some("ztu_0123456789abcdef0123456789abcdef0123".to_string()),
            start_parameter: None,
            start_payload: None,
            bot_url: None,
            url: None,
        })
        .is_some());
        assert!(extract_top_up_start(TopUpIntentData {
            code: Some("ztu_short".to_string()),
            start_parameter: None,
            start_payload: None,
            bot_url: None,
            url: None,
        })
        .is_none());
        assert_eq!(
            extract_top_up_start(TopUpIntentData {
                code: Some("ztu_0123456789abcdef0123456789abcdef0123".to_string()),
                start_parameter: Some("ztu_short".to_string()),
                start_payload: None,
                bot_url: None,
                url: None,
            })
            .as_deref(),
            Some("ztu_0123456789abcdef0123456789abcdef0123")
        );
        assert!(extract_top_up_start(TopUpIntentData {
            code: None,
            start_parameter: Some("ztu_0123456789ABCDEF0123456789ABCDEF0123".to_string()),
            start_payload: None,
            bot_url: None,
            url: None,
        })
        .is_none());
        assert!(extract_top_up_start_from_url(
            "https://t.me/zenith_service_bot?start=ztu_0123456789abcdef0123456789abcdef012g"
        )
        .is_none());
    }

    #[test]
    fn top_up_amount_validation_rejects_invalid_ipc_amounts() {
        assert!(validate_top_up_amount_cents(100).is_ok());
        assert!(validate_top_up_amount_cents(MAX_AMOUNT_CENTS).is_ok());
        assert!(validate_top_up_amount_cents(0).is_err());
        assert!(validate_top_up_amount_cents(MAX_AMOUNT_CENTS + 1).is_err());
    }

    #[test]
    fn api_error_sanitizer_hides_backend_and_token_details() {
        assert_eq!(
            sanitize_api_error_message(
                "provider failed at https://upstream.example/v1 with token sk-secret and cf-ray abc",
                "Stats request failed."
            ),
            "Stats request failed."
        );
        assert_eq!(
            sanitize_api_error_message("Requested model is disabled", "Stats request failed."),
            "Requested model is disabled"
        );
        assert_eq!(
            sanitize_api_error_message(
                "Insufficient Zenith balance. Top up your Zenith API balance in the bot: https://t.me/zenith_service_bot",
                "Stats request failed."
            ),
            "Insufficient Zenith balance. Top up your Zenith API balance in the bot: https://t.me/zenith_service_bot"
        );
        assert_eq!(
            sanitize_api_error_message(
                "upstream token failed; contact https://t.me/zenith_service_bot",
                "Stats request failed."
            ),
            "Stats request failed."
        );
        assert_eq!(
            sanitize_api_error_message(
                "Insufficient Zenith balance. Top up at https://evil.example",
                "Stats request failed."
            ),
            "Stats request failed."
        );
    }
}

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
        .setup(move |app| {
            let handle = app.handle().clone();
            platform::resolve_codex_home().map_err(std::io::Error::other)?;
            let relay_state = local_pool::initialize(&handle)
                .map_err(|error| std::io::Error::other(error.to_string()))?;
            app.manage(relay_state);
            let native_startup_ms = started.elapsed().as_secs_f64() * 1_000.0;
            let relay_state = app.state::<local_pool::DesktopState>();
            let _ =
                relay_state.record_performance("native_startup", native_startup_ms, Some("cold"));
            if !start_in_tray {
                crate::tray::create_main_window(&handle)?;
                let window_ms = started.elapsed().as_secs_f64() * 1_000.0;
                let _ = relay_state.record_performance("window", window_ms, Some("cold"));
                if cfg!(debug_assertions) {
                    eprintln!(
                        "[startup] native_window={}ms",
                        started.elapsed().as_millis()
                    );
                }
            } else {
                relay_state.set_background_session_active(false);
            }
            local_pool::background::start(handle.clone());
            let relay_state = app.state::<local_pool::DesktopState>();
            let _ = ensure_provider_on_launch(&relay_state.ready_api_backup_root());
            let state = app.state::<AppState>();
            build_tray(&handle, &state)?;
            crate::portable_update::acknowledge_startup();
            tauri::async_runtime::spawn(async move {
                let state = handle.state::<local_pool::DesktopState>();
                let _ = local_pool::commands::gateway::start_if_enabled(&state).await;
                crate::tray::refresh_tray(&handle).await;
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if !crate::tray::is_main_window_label(window.label()) {
                return;
            }
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                close_main_window(window.app_handle());
            }
        })
        .invoke_handler(tauri::generate_handler![
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
            local_pool::commands::state::get_local_runtime_state,
            local_pool::commands::state::get_local_runtime_order,
            local_pool::commands::state::record_local_performance_sample,
            local_pool::commands::connections::create_local_source,
            local_pool::commands::connections::update_local_source,
            local_pool::commands::connections::set_local_source_enabled,
            local_pool::commands::connections::delete_local_source,
            local_pool::commands::connections::rotate_local_source_key,
            local_pool::commands::connections::test_local_source,
            local_pool::commands::connections::get_local_source_stats,
            local_pool::commands::remote_server::get_remote_source_stats,
            local_pool::accounts::import_orchestrator::start_local_account_import,
            local_pool::accounts::import_orchestrator::preview_local_account_import_files,
            local_pool::accounts::import_orchestrator::preview_current_codex_account_import,
            local_pool::accounts::import_orchestrator::current_chatgpt_profile_available,
            local_pool::accounts::import_orchestrator::resume_local_account_import,
            local_pool::accounts::import_orchestrator::prepare_local_account_import,
            local_pool::accounts::import_orchestrator::cancel_local_account_import,
            local_pool::accounts::import_orchestrator::confirm_local_account_import,
            local_pool::accounts::export_ops::reveal_local_account_identity,
            local_pool::accounts::export_ops::export_local_accounts,
            local_pool::accounts::mutations::update_local_account,
            local_pool::accounts::mutations::set_local_account_proxy,
            local_pool::commands::proxies::get_local_proxy_pool,
            local_pool::commands::proxies::import_local_proxy_pool,
            local_pool::commands::proxies::delete_local_stored_proxy,
            local_pool::commands::proxies::delete_local_stored_proxies,
            local_pool::commands::proxies::assign_local_stored_proxy,
            local_pool::commands::proxies::set_local_stored_proxy_accounts,
            local_pool::commands::proxies::assign_free_local_account_proxies,
            local_pool::accounts::mutations::set_local_account_enabled,
            local_pool::accounts::mutations::delete_local_account,
            local_pool::accounts::mutations::delete_local_accounts,
            local_pool::accounts::quota_refresh::refresh_local_account_quota,
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
            local_pool::commands::gateway::start_local_gateway,
            local_pool::commands::gateway::stop_local_gateway,
            local_pool::commands::gateway::restart_local_gateway,
            local_pool::commands::gateway::update_local_gateway_port,
            local_pool::commands::gateway::set_local_common_proxy,
            local_pool::commands::gateway::set_local_account_proxy_required,
            local_pool::commands::gateway::set_local_codex_background_tasks,
            local_pool::commands::gateway::set_local_codex_websockets,
            local_pool::commands::gateway::set_codex_profile_websockets,
            local_pool::commands::gateway::diagnose_local_gateway,
            local_pool::commands::usage::get_local_usage_page,
            local_pool::commands::usage::clear_local_usage,
            local_pool::commands::profiles::update_chatgpt_interface_quota_reserve,
            local_pool::commands::profiles::sync_codex_default_service_tier,
            local_pool::commands::profiles::attach_codex_to_local_gateway,
            local_pool::commands::profiles::attach_codex_to_remote_gateway,
            local_pool::commands::profiles::restore_codex_profile,
            local_pool::commands::profiles::stop_managed_codex_profile,
            local_pool::commands::profiles::launch_managed_codex_profile,
            local_pool::commands::profiles::attach_codex_to_account,
            local_pool::commands::profiles::launch_codex_account,
            local_pool::commands::profiles::launch_codex_source,
            local_pool::commands::profiles::list_codex_account_bindings,
            local_pool::commands::profiles::restore_codex_account_profile,
            local_pool::commands::profiles::list_codex_profile_snapshots,
            local_pool::commands::profiles::create_codex_profile_snapshot,
            local_pool::commands::profiles::restore_full_codex_profile_snapshot,
            local_pool::commands::profiles::delete_codex_profile_snapshot,
            local_pool::commands::recovery::get_relay_storage_info,
            local_pool::commands::recovery::open_relay_folder,
            local_pool::commands::recovery::reset_local_pool_data,
            local_pool::commands::recovery::export_usage,
            local_pool::commands::recovery::export_support_bundle,
            local_pool::commands::recovery::preview_support_bundle,
            local_pool::commands::remote_server::connect_remote_server,
            local_pool::commands::remote_server::get_remote_server_state,
            local_pool::commands::remote_server::get_remote_runtime_order,
            local_pool::commands::remote_server::get_remote_server_usage,
            local_pool::commands::remote_server::export_remote_configuration_preset,
            local_pool::commands::remote_server::preview_remote_configuration_preset,
            local_pool::commands::remote_server::apply_remote_configuration_preset,
            local_pool::commands::remote_server::diagnose_remote_gateway,
            local_pool::commands::remote_server::reveal_remote_account_identity,
            local_pool::commands::remote_server::export_remote_accounts,
            local_pool::commands::remote_server::refresh_remote_server_capabilities,
            local_pool::commands::remote_server::get_remote_linked_account_count,
            local_pool::commands::remote_server::disconnect_remote_server,
            local_pool::commands::remote_server::prepare_remote_server_deployment,
            local_pool::commands::remote_server::preview_remote_account_import_files,
            local_pool::commands::remote_server::move_local_accounts_to_remote,
            local_pool::commands::remote_server::return_remote_account_to_local,
            local_pool::commands::remote_server::force_activate_remote_account_locally,
            local_pool::commands::remote_server::execute_remote_server_action
        ])
        .build(tauri::generate_context!())
        .expect("failed to build Zenith Relay");

    app.run(|app_handle, event| {
        if let RunEvent::ExitRequested { api, code, .. } = event {
            let state = app_handle.state::<AppState>();
            if code.is_none() && state.should_prevent_exit() {
                api.prevent_exit();
            }
        }
    });
}
