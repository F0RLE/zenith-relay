//! Application preferences select existing pool routes, never provider capabilities.
use crate::launcher::settings::Application;
use crate::local_pool::{
    commands,
    error::{CommandError, ErrorCode, LocalPoolError},
    state::DesktopState,
};
use serde::Serialize;
use tauri::State;
use zenith_relay_core::{UpstreamProtocolPolicy, WireApi, protocol::ModelSummary};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApplicationProtocolSetting {
    policy: UpstreamProtocolPolicy,
    available: Vec<UpstreamProtocolPolicy>,
    reconnect_required: bool,
}

fn client_supported(application: Application, client: WireApi) -> bool {
    match application {
        Application::Chatgpt => client == WireApi::Responses,
        Application::ClaudeDesktop => client == WireApi::Messages,
        Application::Opencode => {
            WireApi::ALL_NATIVE.contains(&client) && client != WireApi::GeminiInteractions
        }
    }
}

fn available_protocols(
    application: Application,
    models: &[ModelSummary],
) -> Vec<UpstreamProtocolPolicy> {
    UpstreamProtocolPolicy::ALL
        .into_iter()
        .filter(|policy| {
            *policy == UpstreamProtocolPolicy::Auto
                || models
                    .iter()
                    .filter(|model| {
                        model.enabled && !zenith_relay_core::is_image_model_id(&model.id)
                    })
                    .flat_map(|model| &model.protocol_routes)
                    .any(|route| {
                        client_supported(application, route.client_wire_api)
                            && policy.allows(route.upstream_wire_api)
                    })
        })
        .collect()
}

/// Profile generation and request admission agree about upstream formats.
/// The pool's global catalog is never narrowed by an application's preference.
pub(super) fn filter_models(models: &mut Vec<ModelSummary>, policy: UpstreamProtocolPolicy) {
    if policy == UpstreamProtocolPolicy::Auto {
        return;
    }
    models.retain_mut(|model| {
        if zenith_relay_core::is_image_model_id(&model.id) {
            return true;
        }
        model
            .protocol_routes
            .retain(|route| policy.allows(route.upstream_wire_api));
        !model.protocol_routes.is_empty()
    });
}

pub(super) fn policy(
    state: &DesktopState,
    application: Application,
) -> Result<UpstreamProtocolPolicy, LocalPoolError> {
    Ok(state
        .store()?
        .gateway()
        .application_protocols
        .get(&application)
        .copied()
        .unwrap_or_default())
}

async fn setting(
    state: &DesktopState,
    application: Application,
) -> Result<ApplicationProtocolSetting, CommandError> {
    let snapshot = commands::state::build_streaming_client_state(state).await?;
    let connection_state = state.clone();
    let reconnect_required =
        commands::run_blocking(move || needs_reconnect(&connection_state, application)).await?;
    Ok(ApplicationProtocolSetting {
        policy: policy(state, application)?,
        available: available_protocols(application, &snapshot.gateway.models),
        reconnect_required,
    })
}

fn needs_reconnect(state: &DesktopState, application: Application) -> Result<bool, LocalPoolError> {
    let reconnect_required = match application {
        Application::Chatgpt => !super::codex::profile_bindings(
            &crate::platform::default_codex_home(),
            &state.profile_backup_root(),
        )?
        .iter()
        .any(|binding| binding.active && binding.credential_id == application.pool_key_id()),
        Application::ClaudeDesktop => !super::claude_desktop::uses_application_pool_key(state)?,
        Application::Opencode => !super::opencode::uses_application_pool_key(state)?,
    };
    Ok(reconnect_required)
}

#[tauri::command]
pub(crate) async fn get_application_upstream_protocol(
    application: Application,
    state: State<'_, DesktopState>,
) -> Result<ApplicationProtocolSetting, CommandError> {
    let _mutation = state.setup_guard().await;
    setting(&state, application).await
}

#[tauri::command]
pub(crate) async fn set_application_upstream_protocol(
    application: Application,
    policy: UpstreamProtocolPolicy,
    expected: UpstreamProtocolPolicy,
    state: State<'_, DesktopState>,
) -> Result<ApplicationProtocolSetting, CommandError> {
    let _mutation = state.setup_guard().await;
    let mut current = setting(&state, application).await?;
    if current.policy != expected {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "application protocol changed; reload the setting",
        )
        .into());
    }
    if current.policy == policy {
        return Ok(current);
    }
    if !current.available.contains(&policy) {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "the pool has no compatible route for the selected application protocol",
        )
        .into());
    }
    let previous = state.store()?.gateway().clone();
    let mut updated = previous.clone();
    if policy == UpstreamProtocolPolicy::Auto {
        updated.application_protocols.remove(&application);
    } else {
        updated.application_protocols.insert(application, policy);
    }
    state.store()?.replace_gateway(updated)?;
    if let Some(runtime) = state.gateway.runtime().await
        && !runtime.set_key_upstream_protocol(application.pool_key_id(), policy)
    {
        state.store()?.replace_gateway(previous)?;
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "restart the local API before changing application protocols",
        )
        .into());
    }
    current.policy = policy;
    crate::local_pool::runtime::queue_active_client_catalog_refresh(&state);
    Ok(current)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn application_protocol_choices_require_an_executable_client_route() {
        // Empty pools retain Auto; unknown/unsupported endpoints grant no choice.
        for application in Application::ALL {
            assert_eq!(
                available_protocols(application, &[]),
                vec![UpstreamProtocolPolicy::Auto]
            );
        }
        assert!(client_supported(Application::Chatgpt, WireApi::Responses));
        assert!(!client_supported(Application::Chatgpt, WireApi::Messages));
        assert!(client_supported(
            Application::ClaudeDesktop,
            WireApi::Messages
        ));
        assert!(!client_supported(
            Application::Opencode,
            WireApi::GeminiInteractions
        ));
    }

    #[test]
    fn application_protocol_storage_defaults_to_auto_and_rejects_unknown_formats() {
        let settings = crate::local_pool::models::GatewaySettings::default();
        let mut value = serde_json::to_value(&settings).unwrap();
        assert!(value.get("applicationProtocols").is_none());
        value["applicationProtocols"] =
            serde_json::json!({"chatgpt":"messages", "opencode":"gemini"});
        let saved: crate::local_pool::models::GatewaySettings =
            serde_json::from_value(value.clone()).unwrap();
        assert_eq!(
            saved.application_protocols[&Application::Chatgpt],
            UpstreamProtocolPolicy::Messages
        );
        value["applicationProtocols"]["chatgpt"] = "invented".into();
        assert!(
            serde_json::from_value::<crate::local_pool::models::GatewaySettings>(value).is_err()
        );
    }

    #[test]
    fn application_protocol_keys_are_distinct_reused_and_not_retired() {
        use crate::local_pool::store::{gateway_keys, secret_store};
        let root = crate::files::test_temp_root("application-protocol-keys");
        let state = DesktopState::open(root.clone()).unwrap();
        let mut secrets = std::collections::BTreeSet::new();
        // Creating app keys first must not let shared-key recovery adopt one.
        for application in Application::ALL {
            let key = gateway_keys::ensure_application_gateway_key(&state, application).unwrap();
            assert_eq!(key.id, application.pool_key_id());
            let secret = secret_store::load(&key.secret_ref).unwrap().unwrap();
            assert!(secrets.insert(secret.clone()));
            let reused = gateway_keys::ensure_application_gateway_key(&state, application).unwrap();
            assert_eq!(reused.secret_ref, key.secret_ref);
            assert_eq!(
                secret_store::load(&reused.secret_ref).unwrap(),
                Some(secret)
            );
        }
        let shared = gateway_keys::ensure_system_gateway_key(&state).unwrap();
        assert_eq!(shared.id, gateway_keys::SYSTEM_GATEWAY_KEY_ID);
        assert!(secrets.insert(secret_store::load(&shared.secret_ref).unwrap().unwrap()));
        gateway_keys::retire_user_gateway_keys(&state).unwrap();
        assert_eq!(state.store().unwrap().keys().len(), 4);
        for key in state.store().unwrap().keys() {
            secret_store::delete(&key.secret_ref).unwrap();
        }
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }
}
