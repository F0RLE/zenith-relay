use crate::{
    codex_config::load_api_key_for_launch,
    launcher::{is_codex_running, launch_codex_with_profile, stop_codex_and_wait},
    local_pool::{
        accounts::{
            credentials::CredentialStore,
            quota_refresh::{
                prepare_account_credentials, sync_managed_account_profile,
                PreparedAccountCredentials,
            },
            records::{candidate_health, candidate_quota_with_stale_after},
            NativeSecretBackend,
        },
        error::{CommandError, ErrorCode, LocalPoolError, Result as LocalResult},
        models::LocalPoolSnapshot,
        profiles::{codex, repair, snapshots},
        remote::client::RemoteProfileCredential,
        state::DesktopState,
        store::secret_store,
    },
    platform::default_codex_home,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tauri::State;
use zenith_relay_core::{
    protocol::{Feature, ProfileKeyRotation},
    DefaultServiceTier, QUOTA_STALE_AFTER_MS,
};

mod catalog;
mod policy;

use catalog::{
    fetch_codex_model_catalog, fetch_direct_source_model_manifest, load_direct_source_api_key,
    validate_direct_source,
};
pub(in crate::local_pool) use catalog::{refresh_active_codex_catalog, CodexCatalogRefreshStatus};
use policy::{
    gateway_oauth_binding_request, prioritize_account_candidates, profile_quota_rank,
    GatewayOAuthBindingRequest,
};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileActivation {
    binding: codex::ProfileBinding,
}

#[derive(Clone, Copy)]
pub(crate) enum CodexHistoryProvider {
    ChatGpt,
    LocalGateway,
    ReadyApi,
}

pub(crate) fn synchronize_codex_history(
    state: &DesktopState,
    profile_dir: &std::path::Path,
    provider: CodexHistoryProvider,
) -> Result<Option<String>, String> {
    let provider = match provider {
        CodexHistoryProvider::ChatGpt => repair::TargetProvider::Openai,
        CodexHistoryProvider::LocalGateway => repair::TargetProvider::ZenithRelayLocal,
        CodexHistoryProvider::ReadyApi => repair::TargetProvider::CodexLocalAccess,
    };
    repair::synchronize(
        &state.transient_root(),
        &state.history_repair_backup_root(),
        profile_dir,
        provider,
    )
    .map(|result| result.map(|result| result.backup_id))
}

pub(crate) fn rollback_codex_history(state: &DesktopState, backup_id: &str) -> Result<(), String> {
    repair::rollback(&state.history_repair_backup_root(), backup_id)?;
    repair::discard(&state.history_repair_backup_root(), backup_id)
}

pub(crate) fn discard_codex_history_backup(state: &DesktopState, backup_id: Option<&str>) {
    if let Some(backup_id) = backup_id {
        let _ = repair::discard(&state.history_repair_backup_root(), backup_id);
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateChatgptQuotaReserveInput {
    reserve_basis_points: u64,
}

#[tauri::command]
pub async fn update_chatgpt_interface_quota_reserve(
    input: UpdateChatgptQuotaReserveInput,
    state: State<'_, DesktopState>,
) -> Result<LocalPoolSnapshot, CommandError> {
    let _mutation = state.setup_guard().await;
    let profile_dir = default_codex_home();
    let protected_account_id =
        if codex::credential_kind(&profile_dir, &state.profile_backup_root())?
            == Some(codex::ProfileCredentialKind::LocalGateway)
        {
            codex::active_managed_account_id(&profile_dir, &state.profile_backup_root())?
        } else {
            None
        };
    let old_gateway = state.store()?.gateway().clone();
    if old_gateway.chatgpt_interface_quota_reserve_basis_points == input.reserve_basis_points {
        return state.snapshot().await.map_err(Into::into);
    }
    let mut gateway = old_gateway;
    gateway.chatgpt_interface_quota_reserve_basis_points = input.reserve_basis_points;
    gateway
        .validate()
        .map_err(|error| LocalPoolError::new(ErrorCode::InvalidState, error))?;
    state.store()?.replace_gateway(gateway)?;
    set_runtime_pool_interface_reserve(
        &state,
        protected_account_id.as_deref(),
        input.reserve_basis_points,
    )
    .await;
    state.snapshot().await.map_err(Into::into)
}

#[tauri::command]
pub async fn sync_codex_default_service_tier(
    default_service_tier: DefaultServiceTier,
    state: State<'_, DesktopState>,
) -> Result<(), CommandError> {
    let _mutation = state.setup_guard().await;
    codex::sync_default_service_tier(&default_codex_home(), default_service_tier)
        .map_err(Into::into)
}

#[tauri::command]
pub async fn attach_codex_to_local_gateway(
    bound_oauth_account_id: Option<String>,
    disable_oauth_binding: Option<bool>,
    state: State<'_, DesktopState>,
) -> Result<ProfileActivation, CommandError> {
    let _mutation = state.setup_guard().await;
    let key = super::pool::ensure_system_gateway_key(&state)?;
    let key_id = key.id.clone();
    let (port, reserve_basis_points, supports_websockets) = {
        let store = state.store()?;
        (
            store.gateway().port,
            store.gateway().chatgpt_interface_quota_reserve_basis_points,
            store.gateway().codex_websockets_enabled,
        )
    };
    if !key.enabled || !super::pool::has_usable_pool_candidate(&state)? {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "managed pool is not available for any enabled candidate",
        )
        .into());
    }
    let secret = super::pool::ensure_local_gateway_key_secret(&key)?;
    let profile_dir = default_codex_home();
    let stopped = stop_codex_and_sync_account(&state).await?;
    let result: Result<ProfileActivation, CommandError> = async {
        let binding_request = gateway_oauth_binding_request(
            disable_oauth_binding.unwrap_or(false),
            bound_oauth_account_id.as_deref(),
        )?;
        let bound_oauth =
            resolve_gateway_oauth_binding(&state, binding_request, &profile_dir).await?;
        let history_backup = synchronize_history_for_command(
            &state,
            &profile_dir,
            CodexHistoryProvider::LocalGateway,
        )?;
        let base_url = format!("http://127.0.0.1:{port}/v1");
        let catalog = fetch_codex_model_catalog(&base_url, &secret).await?;
        let attached: Result<_, CommandError> = match bound_oauth.as_ref() {
            Some((account_id, prepared)) => codex::attach_with_oauth_and_options(
                &profile_dir,
                &state.profile_backup_root(),
                &key_id,
                &base_url,
                &secret,
                codex::OAuthAttachOptions {
                    catalog_json: &catalog,
                    bound_oauth: codex::BoundOAuthProfile {
                        account_id,
                        tokens: prepared.tokens(),
                        provider_account_id: prepared.provider_account_id(),
                    },
                    supports_websockets,
                },
            ),
            None => codex::attach_with_catalog_and_websockets(
                &profile_dir,
                &state.profile_backup_root(),
                &key_id,
                &base_url,
                &secret,
                &catalog,
                supports_websockets,
            ),
        }
        .map_err(Into::into);
        let binding = rollback_history_on_error(&state, history_backup.as_deref(), attached)?;
        set_runtime_pool_interface_reserve(
            &state,
            binding.bound_oauth_account_id.as_deref(),
            reserve_basis_points,
        )
        .await;
        Ok(ProfileActivation { binding })
    }
    .await;
    restart_codex_after_failed_change(stopped, result, launch_codex_with_profile)
}

#[tauri::command]
pub async fn attach_codex_to_remote_gateway(
    state: State<'_, DesktopState>,
) -> Result<ProfileActivation, CommandError> {
    let _mutation = state.setup_guard().await;
    let Some((_, client)) = super::remote_server::active_client(&state)? else {
        return Err(
            LocalPoolError::new(ErrorCode::NotFound, "remote server is not connected").into(),
        );
    };
    let capabilities = client
        .capabilities()
        .await
        .map_err(super::remote_server::remote_error)?;
    if !capabilities.supports(Feature::ProfileAttach) {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "remote server does not support profile attachment",
        )
        .into());
    }
    let current_credential = client
        .profile_credential()
        .await
        .map_err(super::remote_server::remote_error)?;
    let supports_websockets = client
        .state()
        .await
        .map_err(super::remote_server::remote_error)?
        .gateway
        .codex_websockets_enabled;
    let rotate_profile_key = capabilities.supports(Feature::ProfileKeyRotation);
    let profile_dir = default_codex_home();
    let stopped = stop_codex_and_sync_account(&state).await?;
    let result: Result<ProfileActivation, CommandError> = async {
        let history_backup = synchronize_history_for_command(
            &state,
            &profile_dir,
            CodexHistoryProvider::LocalGateway,
        )?;
        let rotation = if rotate_profile_key {
            Some(
                client
                    .prepare_profile_key_rotation()
                    .await
                    .map_err(super::remote_server::remote_error)?,
            )
        } else {
            None
        };
        let (key_id, base_url, secret) = rotation
            .as_ref()
            .map(|rotation| {
                (
                    rotation.key_id.as_str(),
                    rotation.base_url.as_str(),
                    rotation.secret.as_str(),
                )
            })
            .unwrap_or((
                current_credential.key_id.as_str(),
                current_credential.base_url.as_str(),
                current_credential.secret.as_str(),
            ));
        let catalog = match fetch_codex_model_catalog(base_url, secret).await {
            Ok(catalog) => catalog,
            Err(mut error) => {
                if let Some(rotation) = rotation.as_ref() {
                    append_remote_cleanup_error(
                        &mut error,
                        client
                            .abort_profile_key_rotation(&rotation.rotation_id)
                            .await,
                    );
                }
                return Err(error);
            }
        };
        let attached = codex::attach_with_catalog_and_websockets(
            &profile_dir,
            &state.profile_backup_root(),
            key_id,
            base_url,
            secret,
            &catalog,
            supports_websockets,
        )
        .map_err(Into::into);
        let binding = match rollback_history_on_error(&state, history_backup.as_deref(), attached) {
            Ok(binding) => binding,
            Err(mut error) => {
                if let Some(rotation) = rotation.as_ref() {
                    append_remote_cleanup_error(
                        &mut error,
                        client
                            .abort_profile_key_rotation(&rotation.rotation_id)
                            .await,
                    );
                }
                return Err(error);
            }
        };
        if let Some(rotation) = rotation.as_ref() {
            if let Err(mut error) = verify_remote_profile_binding(
                &profile_dir,
                &state.profile_backup_root(),
                &rotation.key_id,
            ) {
                append_profile_rollback_error(
                    &mut error,
                    &profile_dir,
                    &state.profile_backup_root(),
                    &current_credential,
                );
                append_remote_cleanup_error(
                    &mut error,
                    client
                        .abort_profile_key_rotation(&rotation.rotation_id)
                        .await,
                );
                return Err(error);
            }
            if let Err(commit_error) = client
                .commit_profile_key_rotation(&rotation.rotation_id)
                .await
            {
                let mut error = super::remote_server::remote_error(commit_error);
                let observed = client.profile_credential().await;
                match profile_rotation_commit_state(
                    observed.as_ref().ok(),
                    &current_credential,
                    rotation,
                ) {
                    ProfileRotationCommitState::Committed => {
                        return Ok(ProfileActivation { binding });
                    }
                    ProfileRotationCommitState::NotCommitted => {
                        append_profile_rollback_error(
                            &mut error,
                            &profile_dir,
                            &state.profile_backup_root(),
                            &current_credential,
                        );
                        append_remote_cleanup_error(
                            &mut error,
                            client
                                .abort_profile_key_rotation(&rotation.rotation_id)
                                .await,
                        );
                    }
                    ProfileRotationCommitState::Unknown => {}
                }
                return Err(error);
            }
        }
        Ok(ProfileActivation { binding })
    }
    .await;
    restart_codex_after_failed_change(stopped, result, launch_codex_with_profile)
}

async fn resolve_gateway_oauth_binding(
    state: &DesktopState,
    request: GatewayOAuthBindingRequest<'_>,
    profile_dir: &std::path::Path,
) -> LocalResult<Option<(String, PreparedAccountCredentials)>> {
    if request == GatewayOAuthBindingRequest::Disabled {
        return Ok(None);
    }
    let requested_account_id = match request {
        GatewayOAuthBindingRequest::Account(account_id) => Some(account_id),
        GatewayOAuthBindingRequest::Disabled | GatewayOAuthBindingRequest::Automatic => None,
    };
    let preferred_account_id = match requested_account_id {
        Some(account_id) => Some(account_id.to_string()),
        None => codex::active_managed_account_id(profile_dir, &state.profile_backup_root())?,
    };
    let automatic = requested_account_id.is_none();
    let mut candidates = {
        let store = state.store()?;
        let credentials = CredentialStore::from_backend(NativeSecretBackend);
        let mut candidates = Vec::new();
        for account in store.accounts() {
            let explicitly_requested = requested_account_id == Some(account.account.id.as_str());
            if !account.account.enabled
                || !account.account.in_pool
                || account.account.draining
                || account.account.auth_state
                    != zenith_relay_core::accounts::AccountAuthState::Active
                || !candidate_health(&account.account).is_eligible()
                || !matches!(
                    account.account.auth_mode,
                    zenith_relay_core::accounts::AccountAuthMode::OAuth
                        | zenith_relay_core::accounts::AccountAuthMode::ImportedToken
                )
            {
                continue;
            }
            if credentials
                .load(&account.account.id)
                .map_err(|error| {
                    LocalPoolError::new(ErrorCode::SecretStoreUnavailable, error.to_string())
                })?
                .is_some()
            {
                let Some(remaining) = profile_quota_rank(
                    candidate_quota_with_stale_after(
                        &account.account.quota,
                        super::current_time_ms(),
                        QUOTA_STALE_AFTER_MS,
                    ),
                    explicitly_requested,
                ) else {
                    continue;
                };
                candidates.push((account.account.id.clone(), remaining));
            }
        }
        candidates
    };
    prioritize_account_candidates(&mut candidates, preferred_account_id.as_deref(), automatic);
    if requested_account_id.is_some()
        && candidates
            .first()
            .is_none_or(|(candidate, _)| Some(candidate.as_str()) != requested_account_id)
    {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "selected OAuth account is not available to the local pool",
        ));
    }

    let mut last_error = None;
    for (account_id, _) in candidates {
        match prepare_account_credentials(state, &account_id).await {
            Ok(prepared)
                if prepared.tokens().refresh_token().is_some()
                    && prepared.tokens().id_token().is_some() =>
            {
                return Ok(Some((account_id, prepared)));
            }
            Ok(_) => {
                last_error = Some(LocalPoolError::new(
                    ErrorCode::InvalidState,
                    "OAuth binding requires refresh, identity, and account tokens",
                ));
            }
            Err(error) => last_error = Some(error),
        }
        if requested_account_id.is_some() {
            break;
        }
    }
    match last_error {
        Some(error) => Err(error),
        None => Ok(None),
    }
}

#[tauri::command]
pub async fn restore_codex_profile(state: State<'_, DesktopState>) -> Result<(), CommandError> {
    let _mutation = state.setup_guard().await;
    let profile_dir = default_codex_home();
    let stopped = stop_codex_and_sync_account(&state).await?;
    let result =
        synchronize_history_for_command(&state, &profile_dir, CodexHistoryProvider::ChatGpt)
            .and_then(|history_backup| {
                let result =
                    codex::restore(&profile_dir, &state.profile_backup_root()).map_err(Into::into);
                rollback_history_on_error(&state, history_backup.as_deref(), result)
            });
    if result.is_ok() {
        set_runtime_pool_interface_reserve(&state, None, 0).await;
    }
    restart_codex_after_restore(stopped, result, launch_codex_with_profile)
}

pub(crate) async fn prepare_ready_api_profile(state: &DesktopState) -> Result<bool, CommandError> {
    let _mutation = state.setup_guard().await;
    let profile_dir = default_codex_home();
    let restore_local_gateway = codex::credential_kind(&profile_dir, &state.profile_backup_root())?
        == Some(codex::ProfileCredentialKind::LocalGateway);
    let stopped = stop_codex_and_sync_account(state).await?;
    if !restore_local_gateway {
        return Ok(stopped);
    }
    let result = codex::restore(&profile_dir, &state.profile_backup_root()).map_err(Into::into);
    let result = restart_codex_after_failed_change(stopped, result, launch_codex_with_profile);
    if result.is_ok() {
        set_runtime_pool_interface_reserve(state, None, 0).await;
    }
    result.map(|()| stopped)
}

#[tauri::command]
pub async fn stop_managed_codex_profile(
    state: State<'_, DesktopState>,
) -> Result<bool, CommandError> {
    let _mutation = state.setup_guard().await;
    stop_codex_and_sync_account(&state).await
}

#[tauri::command]
pub async fn launch_managed_codex_profile(
    state: State<'_, DesktopState>,
) -> Result<(), CommandError> {
    let _mutation = state.setup_guard().await;
    if !is_codex_running() {
        let profile_dir = default_codex_home();
        let provider = match codex::credential_kind(&profile_dir, &state.profile_backup_root())? {
            Some(codex::ProfileCredentialKind::LocalGateway) => {
                Some(CodexHistoryProvider::LocalGateway)
            }
            Some(codex::ProfileCredentialKind::OAuthAccount) => Some(CodexHistoryProvider::ChatGpt),
            Some(codex::ProfileCredentialKind::ApiKey) | None => None,
        };
        if let Some(provider) = provider {
            let backup = synchronize_history_for_command(&state, &profile_dir, provider)?;
            discard_codex_history_backup(&state, backup.as_deref());
        }
    }
    launch_codex_with_profile().map_err(|error| {
        LocalPoolError::new(ErrorCode::Io, format!("failed to launch ChatGPT: {error}")).into()
    })
}

#[tauri::command]
pub async fn attach_codex_to_account(
    account_id: String,
    profile_dir: Option<String>,
    state: State<'_, DesktopState>,
) -> Result<ProfileActivation, CommandError> {
    let _mutation = state.setup_guard().await;
    activate_account_profile(&account_id, profile_dir, &state).await
}

#[tauri::command]
pub async fn launch_codex_account(
    account_id: String,
    state: State<'_, DesktopState>,
) -> Result<ProfileActivation, CommandError> {
    let _mutation = state.setup_guard().await;
    activate_account_profile(&account_id, None, &state).await
}

#[tauri::command]
pub async fn launch_codex_source(
    source_id: String,
    state: State<'_, DesktopState>,
) -> Result<ProfileActivation, CommandError> {
    let _mutation = state.setup_guard().await;
    let source = state
        .store()?
        .source(&source_id)
        .cloned()
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "source not found"))?;
    let response_models = validate_direct_source(&source)?;
    let api_key = load_direct_source_api_key(
        &source.base_url,
        &source.secret_ref,
        secret_store::load,
        load_api_key_for_launch,
        secret_store::save,
    )?;
    let profile_dir = default_codex_home();
    let manifest = fetch_direct_source_model_manifest(&source.base_url, &api_key)
        .await
        .ok();
    let catalog = codex::direct_source_model_catalog_with_manifest(
        &profile_dir,
        &response_models,
        manifest.as_ref(),
    )?;
    if catalog.is_none() {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "source has no compatible text models",
        )
        .into());
    }
    let stopped = stop_codex_and_sync_account(&state).await?;
    let result =
        synchronize_history_for_command(&state, &profile_dir, CodexHistoryProvider::LocalGateway)
            .and_then(|history_backup| {
                let result = codex::attach_with_catalog(
                    &profile_dir,
                    &state.profile_backup_root(),
                    &source.id,
                    &source.base_url,
                    &api_key,
                    catalog.as_deref().expect("validated direct source catalog"),
                )
                .map(|binding| ProfileActivation { binding })
                .map_err(Into::into);
                rollback_history_on_error(&state, history_backup.as_deref(), result)
            });
    let result = restart_codex_after_failed_change(stopped, result, launch_codex_with_profile);
    if result.is_ok() {
        set_runtime_pool_interface_reserve(&state, None, 0).await;
    }
    result
}

#[tauri::command]
pub fn list_codex_account_bindings(
    state: State<'_, DesktopState>,
) -> Result<Vec<codex::ProfileBinding>, CommandError> {
    codex::profile_bindings(&default_codex_home(), &state.profile_backup_root()).map_err(Into::into)
}

#[tauri::command]
pub async fn restore_codex_account_profile(
    profile_dir: Option<String>,
    state: State<'_, DesktopState>,
) -> Result<Option<codex::ProfileBinding>, CommandError> {
    let _mutation = state.setup_guard().await;
    let profile_dir = resolve_profile_dir(profile_dir)?;
    let stopped = stop_codex_and_sync_account_at(&state, &profile_dir).await?;
    let result =
        synchronize_history_for_command(&state, &profile_dir, CodexHistoryProvider::ChatGpt)
            .and_then(|history_backup| {
                let result =
                    codex::restore_account_profile(&profile_dir, &state.profile_backup_root())
                        .map_err(Into::into);
                rollback_history_on_error(&state, history_backup.as_deref(), result)
            });
    restart_codex_after_restore(stopped, result, launch_codex_with_profile)
}

#[tauri::command]
pub fn list_codex_profile_snapshots(
    state: State<'_, DesktopState>,
) -> Result<snapshots::ProfileSnapshotList, CommandError> {
    snapshots::list(&state.profile_backup_root()).map_err(Into::into)
}

#[tauri::command]
pub async fn create_codex_profile_snapshot(
    name: String,
    state: State<'_, DesktopState>,
) -> Result<snapshots::ProfileSnapshotSummary, CommandError> {
    let _mutation = state.setup_guard().await;
    let stopped = stop_codex_and_sync_account(&state).await?;
    let result = snapshots::create(&default_codex_home(), &state.profile_backup_root(), &name)
        .map_err(Into::into);
    restart_codex_after_restore(stopped, result, launch_codex_with_profile)
}

#[tauri::command]
pub async fn restore_full_codex_profile_snapshot(
    snapshot_id: String,
    state: State<'_, DesktopState>,
) -> Result<(), CommandError> {
    let _mutation = state.setup_guard().await;
    let stopped = stop_codex_and_sync_account(&state).await?;
    let result = snapshots::restore_full(
        &default_codex_home(),
        &state.profile_backup_root(),
        &snapshot_id,
    )
    .map_err(Into::into);
    restart_codex_after_restore(stopped, result, launch_codex_with_profile)
}

#[tauri::command]
pub async fn delete_codex_profile_snapshot(
    snapshot_id: String,
    state: State<'_, DesktopState>,
) -> Result<(), CommandError> {
    let _mutation = state.setup_guard().await;
    snapshots::delete(&state.profile_backup_root(), &snapshot_id).map_err(Into::into)
}

pub(crate) async fn restore_managed_profiles_before_reset(
    state: &DesktopState,
) -> Result<(), CommandError> {
    let profile_dir = default_codex_home();
    let backup_root = state.profile_backup_root();
    let bindings = codex::profile_bindings(&profile_dir, &backup_root)?;
    if bindings.is_empty() {
        return Ok(());
    }

    let stopped = stop_codex_for_profile_change()?;
    let result: Result<(), CommandError> = async {
        let mut account_ids = bindings
            .iter()
            .filter(|binding| binding.credential_kind == codex::ProfileCredentialKind::OAuthAccount)
            .map(|binding| binding.credential_id.clone())
            .collect::<Vec<_>>();
        account_ids.sort();
        account_ids.dedup();
        for account_id in account_ids {
            if state.store()?.account(&account_id).is_some() {
                sync_managed_account_profile(state, &account_id).await?;
            }
        }

        let bindings = codex::profile_bindings(&profile_dir, &backup_root)?;
        if bindings.iter().any(|binding| !binding.active) {
            return Err(LocalPoolError::new(
                ErrorCode::ProfileRestoreBlocked,
                "ChatGPT profile changed after the automatic backup; local data was not reset",
            )
            .into());
        }
        for binding in bindings {
            let profile = Path::new(&binding.profile_dir);
            let history_backup =
                synchronize_history_for_command(state, profile, CodexHistoryProvider::ChatGpt)?;
            let restored = codex::restore(profile, &backup_root).map_err(Into::into);
            rollback_history_on_error(state, history_backup.as_deref(), restored)?;
        }
        Ok(())
    }
    .await;
    let result = restart_codex_after_restore(stopped, result, launch_codex_with_profile);
    if result.is_ok() {
        set_runtime_pool_interface_reserve(state, None, 0).await;
    }
    result
}

async fn activate_account_profile(
    account_id: &str,
    profile_dir: Option<String>,
    state: &DesktopState,
) -> Result<ProfileActivation, CommandError> {
    let profile_dir = resolve_profile_dir(profile_dir)?;
    let stopped = stop_codex_and_sync_account_at(state, &profile_dir).await?;
    let result: Result<ProfileActivation, CommandError> = async {
        let prepared = prepare_account_credentials(state, account_id).await?;
        let history_backup =
            synchronize_history_for_command(state, &profile_dir, CodexHistoryProvider::ChatGpt)?;
        let attached = codex::attach_account(
            &profile_dir,
            &state.profile_backup_root(),
            account_id,
            prepared.tokens(),
            prepared.provider_account_id(),
        )
        .map_err(Into::into);
        let binding = rollback_history_on_error(state, history_backup.as_deref(), attached)?;
        Ok(ProfileActivation { binding })
    }
    .await;
    let result = restart_codex_after_failed_change(stopped, result, launch_codex_with_profile);
    if result.is_ok() {
        set_runtime_pool_interface_reserve(state, None, 0).await;
    }
    result
}

async fn set_runtime_pool_interface_reserve(
    state: &DesktopState,
    account_id: Option<&str>,
    reserve_basis_points: u64,
) {
    if let Some(runtime) = state.gateway.runtime().await {
        runtime.set_protected_candidate(account_id, reserve_basis_points);
    }
}

fn synchronize_history_for_command(
    state: &DesktopState,
    profile_dir: &std::path::Path,
    provider: CodexHistoryProvider,
) -> Result<Option<String>, CommandError> {
    synchronize_codex_history(state, profile_dir, provider)
        .map_err(|message| LocalPoolError::new(ErrorCode::RecoveryRequired, message).into())
}

fn rollback_history_on_error<T>(
    state: &DesktopState,
    backup_id: Option<&str>,
    result: Result<T, CommandError>,
) -> Result<T, CommandError> {
    match result {
        Ok(value) => {
            discard_codex_history_backup(state, backup_id);
            Ok(value)
        }
        Err(mut error) => {
            if let Some(backup_id) = backup_id {
                if let Err(rollback) = rollback_codex_history(state, backup_id) {
                    error.message = format!(
                        "{}; automatic history rollback failed: {rollback}",
                        error.message
                    );
                }
            }
            Err(error)
        }
    }
}

fn verify_remote_profile_binding(
    profile_dir: &std::path::Path,
    backup_root: &std::path::Path,
    key_id: &str,
) -> Result<(), CommandError> {
    let active = codex::profile_bindings(profile_dir, backup_root)?
        .into_iter()
        .any(|binding| {
            binding.active
                && binding.credential_kind == codex::ProfileCredentialKind::LocalGateway
                && binding.credential_id == key_id
        });
    if active {
        Ok(())
    } else {
        Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "the updated ChatGPT profile could not be verified",
        )
        .into())
    }
}

fn append_profile_rollback_error(
    error: &mut CommandError,
    profile_dir: &std::path::Path,
    backup_root: &std::path::Path,
    credential: &RemoteProfileCredential,
) {
    if let Err(rollback) = codex::attach(
        profile_dir,
        backup_root,
        &credential.key_id,
        &credential.base_url,
        &credential.secret,
    ) {
        error.message = format!(
            "{}; automatic ChatGPT profile rollback failed: {}",
            error.message, rollback.message
        );
    }
}

fn append_remote_cleanup_error(
    error: &mut CommandError,
    cleanup: Result<(), impl std::fmt::Display>,
) {
    if let Err(cleanup) = cleanup {
        error.message = format!(
            "{}; remote profile key cleanup failed: {cleanup}",
            error.message
        );
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProfileRotationCommitState {
    Committed,
    NotCommitted,
    Unknown,
}

fn profile_rotation_commit_state(
    observed: Option<&RemoteProfileCredential>,
    current: &RemoteProfileCredential,
    rotation: &ProfileKeyRotation,
) -> ProfileRotationCommitState {
    let Some(observed) = observed else {
        return ProfileRotationCommitState::Unknown;
    };
    if observed.key_id == rotation.key_id
        && observed.base_url == rotation.base_url
        && observed.secret == rotation.secret
    {
        ProfileRotationCommitState::Committed
    } else if observed.key_id == current.key_id
        && observed.base_url == current.base_url
        && observed.secret == current.secret
    {
        ProfileRotationCommitState::NotCommitted
    } else {
        ProfileRotationCommitState::Unknown
    }
}

fn stop_codex_for_profile_change() -> Result<bool, CommandError> {
    stop_codex_and_wait().map_err(|error| {
        LocalPoolError::new(
            ErrorCode::Io,
            format!("failed to stop ChatGPT before changing its profile: {error}"),
        )
        .into()
    })
}

async fn stop_codex_and_sync_account(state: &DesktopState) -> Result<bool, CommandError> {
    stop_codex_and_sync_account_at(state, &default_codex_home()).await
}

async fn stop_codex_and_sync_account_at(
    state: &DesktopState,
    profile_dir: &std::path::Path,
) -> Result<bool, CommandError> {
    let stopped = stop_codex_for_profile_change()?;
    let result: Result<(), CommandError> = async {
        if let Some(account_id) =
            codex::active_managed_account_id(profile_dir, &state.profile_backup_root())?
        {
            if state.store()?.account(&account_id).is_some() {
                sync_managed_account_profile(state, &account_id).await?;
            }
        }
        Ok(())
    }
    .await;
    restart_codex_after_failed_change(stopped, result, launch_codex_with_profile)?;
    Ok(stopped)
}

fn restart_codex_after_failed_change<T>(
    stopped: bool,
    result: Result<T, CommandError>,
    launch: impl FnOnce() -> Result<(), String>,
) -> Result<T, CommandError> {
    match result {
        Err(mut error) if stopped => {
            if let Err(launch_error) = launch() {
                error.message = format!(
                    "{}; failed to restart ChatGPT: {launch_error}",
                    error.message
                );
            }
            Err(error)
        }
        result => result,
    }
}

fn restart_codex_after_restore<T>(
    stopped: bool,
    result: Result<T, CommandError>,
    launch: impl FnOnce() -> Result<(), String>,
) -> Result<T, CommandError> {
    match result {
        Ok(value) if stopped => launch().map(|()| value).map_err(|error| {
            LocalPoolError::new(
                ErrorCode::Io,
                format!("profile restored, but ChatGPT failed to restart: {error}"),
            )
            .into()
        }),
        result => restart_codex_after_failed_change(stopped, result, launch),
    }
}

fn resolve_profile_dir(profile_dir: Option<String>) -> Result<PathBuf, CommandError> {
    let Some(profile_dir) = profile_dir else {
        return Ok(default_codex_home());
    };
    let profile_dir = profile_dir.trim();
    if profile_dir.is_empty() || profile_dir.chars().any(char::is_control) {
        return Err(LocalPoolError::new(ErrorCode::InvalidState, "profile path is invalid").into());
    }
    let path = PathBuf::from(profile_dir);
    if !path.is_absolute() {
        return Err(
            LocalPoolError::new(ErrorCode::InvalidState, "profile path must be absolute").into(),
        );
    }
    let canonical = std::fs::canonicalize(&path).map_err(|error| {
        LocalPoolError::new(
            ErrorCode::Io,
            format!("failed to access profile path: {error}"),
        )
    })?;
    if !canonical.is_dir() {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "profile path is not a directory",
        )
        .into());
    }
    Ok(canonical)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_pool::models::{LocalGatewayKeyRecord, ProviderSourceRecord};
    use std::cell::{Cell, RefCell};
    use std::collections::BTreeMap;
    use zenith_relay_core::{MessagesReasoningMode, SourceAdapter, SourceProtocolBinding, WireApi};

    fn source_record(id: &str) -> ProviderSourceRecord {
        ProviderSourceRecord {
            id: id.into(),
            name: "Provider".into(),
            enabled: true,
            in_pool: false,
            draining: false,
            base_url: "https://provider.test/v1".into(),
            secret_ref: "source:test".into(),
            wire_api: WireApi::Responses,
            protocol_bindings: Vec::new(),
            models: vec!["provider-model".into()],
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
            priority: 0,
            weight: 1,
            recovery_delay_seconds: 0,
            model_price_overrides: BTreeMap::new(),
            detected_model_prices: BTreeMap::new(),
            last_used_at: None,
            last_test_at: None,
            last_test_status: None,
            last_error: None,
        }
    }

    #[test]
    fn active_catalog_refresh_targets_a_direct_source_unless_a_system_gateway_key_owns_it() {
        let binding = codex::ProfileBinding {
            profile_dir: "profile".into(),
            credential_kind: codex::ProfileCredentialKind::LocalGateway,
            credential_id: "source".into(),
            bound_oauth_account_id: None,
            active: true,
        };
        let source = source_record("source");

        assert!(matches!(
            catalog::active_catalog_refresh_target(&binding, &[], std::slice::from_ref(&source)),
            Some(catalog::CodexCatalogRefreshTarget::DirectSource(candidate))
                if candidate.id == source.id
        ));

        let system_key = LocalGatewayKeyRecord {
            id: "source".into(),
            label: "Local gateway".into(),
            enabled: true,
            system: true,
            secret_ref: "key:source".into(),
            created_at: "2026-08-01T00:00:00Z".into(),
            last_used_at: None,
        };
        assert!(matches!(
            catalog::active_catalog_refresh_target(&binding, &[system_key], &[source]),
            Some(catalog::CodexCatalogRefreshTarget::LocalGateway(_))
        ));
    }

    #[test]
    fn active_catalog_refresh_ignores_sources_without_an_enabled_native_responses_binding() {
        let binding = codex::ProfileBinding {
            profile_dir: "profile".into(),
            credential_kind: codex::ProfileCredentialKind::LocalGateway,
            credential_id: "source".into(),
            bound_oauth_account_id: None,
            active: true,
        };
        let mut source = source_record("source");

        source.enabled = false;
        assert!(catalog::active_catalog_refresh_target(&binding, &[], &[source.clone()]).is_none());

        source.enabled = true;
        source.protocol_bindings = vec![SourceProtocolBinding {
            wire_api: WireApi::Responses,
            adapter: SourceAdapter::ResponsesToMessages,
            reasoning_mode: MessagesReasoningMode::Disabled,
            cache_write_ttl: Default::default(),
            model_ids: vec!["bridge-only".to_string()],
        }];
        source.models = vec!["bridge-only".to_string()];
        assert!(catalog::active_catalog_refresh_target(&binding, &[], &[source]).is_none());
    }

    #[test]
    fn lost_profile_rotation_commit_response_is_reconciled_without_a_blind_rollback() {
        let system_credential_id = "key_system";
        let current = RemoteProfileCredential {
            key_id: system_credential_id.into(),
            base_url: "https://relay.example/v1".into(),
            secret: "zrs_current_secret_value_000000".into(),
        };
        let rotation = ProfileKeyRotation {
            schema_version: 1,
            rotation_id: "key_profile_rotation_test".into(),
            key_id: system_credential_id.into(),
            base_url: current.base_url.clone(),
            secret: "zrs_rotated_secret_value_000000".into(),
        };
        let committed = RemoteProfileCredential {
            key_id: rotation.key_id.clone(),
            base_url: rotation.base_url.clone(),
            secret: rotation.secret.clone(),
        };
        let unrelated = RemoteProfileCredential {
            secret: "zrs_unrelated_secret_value_0000".into(),
            ..current.clone()
        };

        assert_eq!(
            profile_rotation_commit_state(Some(&committed), &current, &rotation),
            ProfileRotationCommitState::Committed
        );
        assert_eq!(
            profile_rotation_commit_state(Some(&current), &current, &rotation),
            ProfileRotationCommitState::NotCommitted
        );
        assert_eq!(
            profile_rotation_commit_state(Some(&unrelated), &current, &rotation),
            ProfileRotationCommitState::Unknown
        );
        assert_eq!(
            profile_rotation_commit_state(None, &current, &rotation),
            ProfileRotationCommitState::Unknown
        );
    }

    #[test]
    fn failed_profile_change_restarts_a_previously_running_codex() {
        let launched = Cell::new(false);
        let error = restart_codex_after_failed_change::<()>(
            true,
            Err(LocalPoolError::new(ErrorCode::Conflict, "profile conflict").into()),
            || {
                launched.set(true);
                Ok(())
            },
        )
        .unwrap_err();

        assert!(launched.get());
        assert!(matches!(error.code, ErrorCode::Conflict));
    }

    #[test]
    fn successful_restore_restarts_a_previously_running_codex() {
        let launched = Cell::new(false);
        restart_codex_after_restore(true, Ok(()), || {
            launched.set(true);
            Ok(())
        })
        .unwrap();

        assert!(launched.get());
    }

    #[test]
    fn direct_source_launch_requires_an_enabled_responses_binding() {
        let source = source_record("source");
        assert_eq!(
            validate_direct_source(&source).unwrap(),
            vec!["provider-model".to_string()]
        );

        let mut disabled = source.clone();
        disabled.enabled = false;
        assert!(validate_direct_source(&disabled).is_err());

        let mut messages_only = source.clone();
        messages_only.wire_api = WireApi::Messages;
        messages_only.protocol_bindings = vec![SourceProtocolBinding {
            wire_api: WireApi::Messages,
            adapter: SourceAdapter::Native,
            reasoning_mode: MessagesReasoningMode::Disabled,
            cache_write_ttl: Default::default(),
            model_ids: vec!["claude-native".to_string()],
        }];
        assert!(validate_direct_source(&messages_only).is_err());

        let mut bridged_only = source;
        bridged_only.protocol_bindings = vec![SourceProtocolBinding {
            wire_api: WireApi::Responses,
            adapter: SourceAdapter::ResponsesToMessages,
            reasoning_mode: MessagesReasoningMode::Disabled,
            cache_write_ttl: Default::default(),
            model_ids: vec!["claude-bridge".to_string()],
        }];
        bridged_only.models = vec!["claude-bridge".to_string()];
        assert!(validate_direct_source(&bridged_only).is_err());
    }

    #[test]
    fn direct_source_launch_uses_only_models_bound_to_responses() {
        let mut source = source_record("source");
        source.wire_api = WireApi::Messages;
        source.models = vec![
            "claude-native".to_string(),
            "claude-responses".to_string(),
            "gpt-responses".to_string(),
        ];
        source.protocol_bindings = vec![
            SourceProtocolBinding {
                wire_api: WireApi::Messages,
                adapter: SourceAdapter::Native,
                reasoning_mode: MessagesReasoningMode::Disabled,
                cache_write_ttl: Default::default(),
                model_ids: vec!["claude-native".to_string()],
            },
            SourceProtocolBinding {
                wire_api: WireApi::Responses,
                adapter: SourceAdapter::Native,
                reasoning_mode: MessagesReasoningMode::Disabled,
                cache_write_ttl: Default::default(),
                model_ids: vec!["claude-responses".to_string(), "gpt-responses".to_string()],
            },
        ];

        assert_eq!(
            validate_direct_source(&source).unwrap(),
            vec!["claude-responses".to_string(), "gpt-responses".to_string()]
        );
    }

    #[test]
    fn missing_zenith_source_secret_is_recovered_once() {
        let saved = RefCell::new(None);
        let api_key = load_direct_source_api_key(
            "https://api.zenithmarket.dev/v1/",
            "source:zenith",
            |_| Ok(None),
            || Some("znt_legacy_key".into()),
            |secret_ref, value| {
                saved.replace(Some((secret_ref.to_string(), value.to_string())));
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(api_key, "znt_legacy_key");
        assert_eq!(
            saved.into_inner(),
            Some(("source:zenith".into(), "znt_legacy_key".into()))
        );
    }

    #[test]
    fn custom_source_does_not_reuse_the_legacy_zenith_key() {
        let error = load_direct_source_api_key(
            "https://api.example.com/v1",
            "source:custom",
            |_| Ok(None),
            || Some("znt_legacy_key".into()),
            |_, _| panic!("custom source secret must not be synthesized"),
        )
        .unwrap_err();

        assert_eq!(error.code, ErrorCode::NotFound);
    }
}
