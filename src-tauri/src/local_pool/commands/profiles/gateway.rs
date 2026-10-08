use super::*;

mod oauth_binding;
use oauth_binding::resolve_gateway_oauth_binding;

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
    let key = super::super::pool::ensure_system_gateway_key(&state)?;
    let key_id = key.id.clone();
    let (port, reserve_basis_points, supports_websockets) = {
        let store = state.store()?;
        (
            store.gateway().port,
            store.gateway().chatgpt_interface_quota_reserve_basis_points,
            store.gateway().codex_websockets_enabled,
        )
    };
    let prepared = super::super::state::build_local_runtime_state(&state).await?;
    let supports_websockets = supports_websockets
        && zenith_relay_core::protocol::codex_catalog_supports_websockets(&prepared.gateway.models);
    if !key.enabled || !super::super::pool::has_usable_pool_candidate(&state)? {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "managed pool is not available for any enabled candidate",
        )
        .into());
    }
    let secret = super::super::pool::ensure_local_gateway_key_secret(&key)?;
    let profile_dir = default_codex_home();
    let sync_history =
        history_provider_changed(&state, &profile_dir, CodexHistoryProvider::LocalGateway)
            .map_err(|message| LocalPoolError::new(ErrorCode::RecoveryRequired, message))?;
    // Validate the selection and obtain the catalog while the current client
    // is still usable. A catalog failure must not stop Codex or rewrite history.
    let binding_request = gateway_oauth_binding_request(
        disable_oauth_binding.unwrap_or(false),
        bound_oauth_account_id.as_deref(),
    )?;
    let base_url = format!("http://127.0.0.1:{port}/v1");
    let catalog = fetch_codex_model_catalog(&base_url, &secret).await?;
    let stopped = stop_codex_and_sync_account(&state).await?;
    let activation_result: Result<ProfileActivation, CommandError> = async {
        let bound_oauth =
            resolve_gateway_oauth_binding(&state, binding_request, &profile_dir).await?;
        let history_backup = if sync_history {
            synchronize_history_for_command(
                &state,
                &profile_dir,
                CodexHistoryProvider::LocalGateway,
            )?
        } else {
            None
        };
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
        state.record_catalog_refresh_result(None);
        Ok(ProfileActivation { binding })
    }
    .await;
    restart_codex_after_failed_change(stopped, activation_result, launch_codex_with_profile)
}

#[tauri::command]
pub async fn attach_codex_to_remote_gateway(
    state: State<'_, DesktopState>,
) -> Result<ProfileActivation, CommandError> {
    let _mutation = state.setup_guard().await;
    let Some((_, client)) = super::super::remote_server::active_client(&state)? else {
        return Err(
            LocalPoolError::new(ErrorCode::NotFound, "remote server is not connected").into(),
        );
    };
    let capabilities = client
        .capabilities()
        .await
        .map_err(super::super::remote_server::remote_error)?;
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
        .map_err(super::super::remote_server::remote_error)?;
    let mut remote_state = client
        .state()
        .await
        .map_err(super::super::remote_server::remote_error)?;
    zenith_relay_core::protocol::apply_model_protocol_routes(
        &mut remote_state.gateway.models,
        &remote_state.sources,
        &remote_state.accounts,
    );
    let supports_websockets = remote_state.gateway.codex_websockets_enabled
        && zenith_relay_core::protocol::codex_catalog_supports_websockets(
            &remote_state.gateway.models,
        );
    let rotate_profile_key = capabilities.supports(Feature::ProfileKeyRotation);
    let profile_dir = default_codex_home();
    let sync_history =
        history_provider_changed(&state, &profile_dir, CodexHistoryProvider::LocalGateway)
            .map_err(|message| LocalPoolError::new(ErrorCode::RecoveryRequired, message))?;
    let stopped = stop_codex_and_sync_account(&state).await?;
    let activation_result: Result<ProfileActivation, CommandError> = async {
        let rotation = if rotate_profile_key {
            Some(
                client
                    .prepare_profile_key_rotation()
                    .await
                    .map_err(super::super::remote_server::remote_error)?,
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
            Err(error) => {
                return Err(abort_profile_rotation(&client, rotation.as_ref(), error).await)
            }
        };
        let history_backup = if sync_history {
            match synchronize_history_for_command(
                &state,
                &profile_dir,
                CodexHistoryProvider::LocalGateway,
            ) {
                Ok(backup) => backup,
                Err(error) => {
                    return Err(abort_profile_rotation(&client, rotation.as_ref(), error).await);
                }
            }
        } else {
            None
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
            Err(error) => {
                return Err(abort_profile_rotation(&client, rotation.as_ref(), error).await);
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
                return Err(abort_profile_rotation(&client, Some(rotation), error).await);
            }
            if let Err(commit_error) = client
                .commit_profile_key_rotation(&rotation.rotation_id)
                .await
            {
                let mut error = super::super::remote_server::remote_error(commit_error);
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
                        error = abort_profile_rotation(&client, Some(rotation), error).await;
                    }
                    ProfileRotationCommitState::Unknown => {}
                }
                return Err(error);
            }
        }
        Ok(ProfileActivation { binding })
    }
    .await;
    restart_codex_after_failed_change(stopped, activation_result, launch_codex_with_profile)
}

async fn abort_profile_rotation(
    client: &crate::local_pool::remote::client::RemoteClient,
    rotation: Option<&zenith_relay_core::protocol::ProfileKeyRotation>,
    mut error: CommandError,
) -> CommandError {
    if let Some(rotation) = rotation {
        append_remote_cleanup_error(
            &mut error,
            client
                .abort_profile_key_rotation(&rotation.rotation_id)
                .await,
        );
    }
    error
}
