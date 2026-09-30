use super::super::{fence_runtime_candidates, restart_or_rollback};
use crate::local_pool::{
    accounts::{
        credentials::{credential_invalid_state_error, CredentialStore},
        proxy::COMMON_PROXY_SECRET_REF,
        NativeSecretBackend,
    },
    error::{CommandError, ErrorCode, LocalPoolError},
    models::{GatewaySettings, LocalPoolSnapshot},
    state::DesktopState,
    store::secret_store,
};
use serde::Deserialize;
use tauri::State;
use zenith_relay_core::GatewayRuntime;

#[tauri::command]
pub async fn set_local_tool_policy(
    input: zenith_relay_core::ToolPolicyUpdate,
    state: State<'_, DesktopState>,
) -> Result<LocalPoolSnapshot, CommandError> {
    let _mutation = state.setup_guard().await;
    let invalid = |message| LocalPoolError::new(ErrorCode::InvalidState, message);
    let policy = input.policy.normalized().map_err(invalid)?;
    let expected = input.expected_policy.normalized().map_err(invalid)?;
    let mut gateway = state.store()?.gateway().clone();
    if gateway.tool_policy != expected {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "tool policy changed; reload before saving",
        )
        .into());
    }
    let previous = gateway.clone();
    gateway.tool_policy = policy.clone();
    state.store()?.replace_gateway(gateway)?;
    if let Some(runtime) = state.gateway.runtime().await {
        if let Err(error) = runtime.set_tool_policy(policy) {
            state.store()?.replace_gateway(previous)?;
            return Err(LocalPoolError::new(ErrorCode::InvalidState, error.to_string()).into());
        }
    }
    state.snapshot().await.map_err(Into::into)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetCommonProxyInput {
    proxy_url: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetAccountProxyPolicyInput {
    required: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetCodexBackgroundTasksInput {
    enabled: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetCodexWebsocketsInput {
    enabled: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetChatgptRetryUntilAvailableInput {
    enabled: bool,
}

#[tauri::command]
pub async fn set_local_common_proxy(
    input: SetCommonProxyInput,
    state: State<'_, DesktopState>,
) -> Result<LocalPoolSnapshot, CommandError> {
    let _mutation = state.setup_guard().await;
    let next_secret = input
        .proxy_url
        .map(|value| zenith_relay_core::normalize_proxy_url(&value))
        .transpose()
        .map_err(|message| LocalPoolError::new(ErrorCode::InvalidState, message))?;
    let old_gateway = state.store()?.gateway().clone();
    let old_secret = secret_store::load(COMMON_PROXY_SECRET_REF)?;
    if old_gateway.common_proxy_configured == next_secret.is_some()
        && old_secret.as_deref() == next_secret.as_deref()
    {
        return state.snapshot().await.map_err(Into::into);
    }
    let affected_accounts = accounts_without_explicit_proxy(&state, false)?;
    let runtime = state.gateway.runtime().await;
    let _dispatch_fences = fence_runtime_candidates(runtime.as_deref(), &affected_accounts, &[]);
    state.store()?.invalidate_refresh_configuration()?;
    save_optional_proxy(next_secret.as_deref())?;
    let mut next_gateway = old_gateway.clone();
    next_gateway.common_proxy_configured = next_secret.is_some();
    if let Err(error) = state.store()?.replace_gateway(next_gateway) {
        restore_common_proxy(old_secret.as_deref())?;
        return Err(error.into());
    }
    restart_or_rollback(&state, || {
        restore_common_proxy(old_secret.as_deref())?;
        state.store()?.replace_gateway(old_gateway)
    })
    .await?;
    state.snapshot().await.map_err(Into::into)
}

#[tauri::command]
pub async fn set_local_account_proxy_required(
    input: SetAccountProxyPolicyInput,
    state: State<'_, DesktopState>,
) -> Result<LocalPoolSnapshot, CommandError> {
    let _mutation = state.setup_guard().await;
    let old_gateway = state.store()?.gateway().clone();
    if old_gateway.account_proxy_required == input.required {
        return state.snapshot().await.map_err(Into::into);
    }
    let affected_accounts = accounts_without_explicit_proxy(&state, true)?;
    let runtime = state.gateway.runtime().await;
    let _dispatch_fences = fence_runtime_candidates(runtime.as_deref(), &affected_accounts, &[]);
    let mut next_gateway = old_gateway.clone();
    next_gateway.account_proxy_required = input.required;
    state.store()?.replace_gateway(next_gateway)?;
    restart_or_rollback(&state, || state.store()?.replace_gateway(old_gateway)).await?;
    state.snapshot().await.map_err(Into::into)
}

#[tauri::command]
pub async fn set_local_codex_background_tasks(
    input: SetCodexBackgroundTasksInput,
    state: State<'_, DesktopState>,
) -> Result<LocalPoolSnapshot, CommandError> {
    save_gateway_flag(
        state.inner(),
        input.enabled,
        |gateway| gateway.codex_background_tasks_enabled,
        |gateway, enabled| gateway.codex_background_tasks_enabled = enabled,
        GatewayRuntime::set_codex_background_tasks_enabled,
    )
    .await
}

/// Updates route recovery for text API requests without rebuilding the gateway.
/// Existing candidate rotation, cooldowns, health state, and affinity remain
/// owned by the running runtime and are observed by new and waiting requests.
#[tauri::command]
pub async fn set_local_chatgpt_retry_until_available(
    input: SetChatgptRetryUntilAvailableInput,
    state: State<'_, DesktopState>,
) -> Result<LocalPoolSnapshot, CommandError> {
    save_gateway_flag(
        state.inner(),
        input.enabled,
        |gateway| gateway.chatgpt_retry_until_available,
        |gateway, enabled| gateway.chatgpt_retry_until_available = enabled,
        GatewayRuntime::set_route_recovery_enabled,
    )
    .await
}

#[tauri::command]
pub async fn set_local_codex_websockets(
    input: SetCodexWebsocketsInput,
    state: State<'_, DesktopState>,
) -> Result<LocalPoolSnapshot, CommandError> {
    let _mutation = state.setup_guard().await;
    let previous_gateway = state.store()?.gateway().clone();
    let snapshot = super::super::state::build_local_runtime_state(&state).await?;
    let profile_websockets = input.enabled
        && zenith_relay_core::protocol::codex_catalog_supports_websockets(&snapshot.gateway.models);
    let profile_dir = crate::platform::default_codex_home();
    let backup_root = state.profile_backup_root();
    let local_keys = state.store()?.keys().to_vec();
    let local_binding =
        crate::local_pool::profiles::codex::profile_bindings(&profile_dir, &backup_root)?
            .into_iter()
            .find(|binding| {
                binding.active
                    && binding.credential_kind
                        == crate::local_pool::profiles::codex::ProfileCredentialKind::LocalGateway
                    && local_keys
                        .iter()
                        .any(|key| key.system && key.id == binding.credential_id)
            });
    let previous_profile = local_binding
        .as_ref()
        .map(|binding| {
            crate::local_pool::profiles::codex::set_local_gateway_websockets_with_previous(
                &profile_dir,
                &backup_root,
                profile_websockets,
                Some(&binding.credential_id),
            )
        })
        .transpose()?
        .flatten();
    let restore_profile = || -> Result<(), CommandError> {
        let Some(previous) = previous_profile else {
            return Ok(());
        };
        crate::local_pool::profiles::codex::set_local_gateway_websockets_with_previous(
            &profile_dir,
            &backup_root,
            previous,
            local_binding
                .as_ref()
                .map(|binding| binding.credential_id.as_str()),
        )
        .map(|_| ())
        .map_err(CommandError::from)
    };
    let previous_enabled = previous_gateway.codex_websockets_enabled;
    let mut gateway = previous_gateway.clone();
    gateway.codex_websockets_enabled = input.enabled;
    if let Err(error) = state.store()?.replace_gateway(gateway) {
        let _ = restore_profile();
        return Err(error.into());
    }
    if let Some(runtime) = state.gateway.runtime().await {
        runtime.set_codex_websockets_enabled(input.enabled);
    }
    match state.snapshot().await {
        Ok(snapshot) => Ok(snapshot),
        Err(error) => {
            let store_rollback = state
                .store()
                .and_then(|mut store| store.replace_gateway(previous_gateway));
            if let Some(runtime) = state.gateway.runtime().await {
                runtime.set_codex_websockets_enabled(previous_enabled);
            }
            let profile_rollback = restore_profile();
            if store_rollback.is_err() || profile_rollback.is_err() {
                return Err(CommandError::from(LocalPoolError::new(
                    ErrorCode::RecoveryRequired,
                    format!(
                        "WebSocket setting failed and rollback was incomplete: {}",
                        error.message
                    ),
                )));
            }
            Err(error.into())
        }
    }
}

async fn save_gateway_flag(
    state: &DesktopState,
    enabled: bool,
    read: impl Fn(&GatewaySettings) -> bool,
    write: impl Fn(&mut GatewaySettings, bool),
    apply: impl Fn(&GatewayRuntime, bool),
) -> Result<LocalPoolSnapshot, CommandError> {
    let _mutation = state.setup_guard().await;
    let mut gateway = state.store()?.gateway().clone();
    if read(&gateway) == enabled {
        return state.snapshot().await.map_err(Into::into);
    }
    write(&mut gateway, enabled);
    state.store()?.replace_gateway(gateway)?;
    if let Some(runtime) = state.gateway.runtime().await {
        apply(runtime.as_ref(), enabled);
    }
    state.snapshot().await.map_err(Into::into)
}

#[tauri::command]
pub async fn set_codex_profile_websockets(
    input: SetCodexWebsocketsInput,
    state: State<'_, DesktopState>,
) -> Result<(), CommandError> {
    let _mutation = state.setup_guard().await;
    let Some((_, client)) = super::super::remote_server::active_client(&state)? else {
        return Err(
            LocalPoolError::new(ErrorCode::NotFound, "remote server is not connected").into(),
        );
    };
    let credential = client
        .profile_credential()
        .await
        .map_err(super::super::remote_server::remote_error)?;
    super::super::profiles::verify_remote_profile_binding(
        &crate::platform::default_codex_home(),
        &state.profile_backup_root(),
        &credential.key_id,
    )?;
    let mut snapshot = client
        .state()
        .await
        .map_err(super::super::remote_server::remote_error)?;
    zenith_relay_core::protocol::apply_model_protocol_routes(
        &mut snapshot.gateway.models,
        &snapshot.sources,
        &snapshot.accounts,
    );
    let enabled = input.enabled
        && zenith_relay_core::protocol::codex_catalog_supports_websockets(&snapshot.gateway.models);
    crate::local_pool::profiles::codex::set_local_gateway_websockets_with_previous(
        &crate::platform::default_codex_home(),
        &state.profile_backup_root(),
        enabled,
        Some(&credential.key_id),
    )
    .and_then(|previous| {
        previous.map(|_| ()).ok_or_else(|| {
            LocalPoolError::new(
                ErrorCode::Conflict,
                "the active profile changed during the update",
            )
        })
    })
    .map_err(Into::into)
}

fn save_optional_proxy(value: Option<&str>) -> crate::local_pool::error::Result<()> {
    match value {
        Some(value) => secret_store::save(COMMON_PROXY_SECRET_REF, value),
        None => secret_store::delete(COMMON_PROXY_SECRET_REF),
    }
}

fn restore_common_proxy(value: Option<&str>) -> crate::local_pool::error::Result<()> {
    save_optional_proxy(value)
}

/// A common proxy affects only inherited routes. Requiring an account proxy
/// additionally affects direct/bypassed routes, but never explicit proxies.
pub(in crate::local_pool::commands) fn accounts_without_explicit_proxy(
    state: &DesktopState,
    include_bypassed: bool,
) -> crate::local_pool::error::Result<Vec<String>> {
    let account_ids = state
        .store()?
        .accounts()
        .iter()
        .map(|account| account.account.id.clone())
        .collect::<Vec<_>>();
    let credentials = CredentialStore::from_backend(NativeSecretBackend);
    let mut affected = Vec::new();
    for id in account_ids {
        let Some(credential) = credentials
            .load(&id)
            .map_err(credential_invalid_state_error)?
        else {
            continue;
        };
        if credential.proxy_url().is_none()
            && (include_bypassed || !credential.bypass_common_proxy())
        {
            affected.push(id);
        }
    }
    Ok(affected)
}
