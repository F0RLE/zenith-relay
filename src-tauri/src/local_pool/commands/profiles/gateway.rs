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
    if !key.enabled || !super::super::pool::has_usable_pool_candidate(&state)? {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "managed pool is not available for any enabled candidate",
        )
        .into());
    }
    let secret = super::super::pool::ensure_local_gateway_key_secret(&key)?;
    let profile_dir = default_codex_home();
    // Validate the selection and obtain the catalog before stopping the client.
    let binding_request = gateway_oauth_binding_request(
        disable_oauth_binding.unwrap_or(false),
        bound_oauth_account_id.as_deref(),
    )?;
    let base_url = format!("http://127.0.0.1:{port}/v1");
    let catalog = fetch_local_codex_model_catalog(&base_url, &secret).await?;
    let catalog = super::catalog::context_catalog(&state, &catalog)?;
    let stopped = stop_codex_and_sync_account(&state).await?;
    let activation_result: Result<ProfileActivation, CommandError> = async {
        let bound_oauth = resolve_gateway_oauth_binding(&state, binding_request).await?;
        let binding = with_chatgpt_context(&state, &profile_dir, || {
            let history_backup = synchronize_history_for_command(
                &state,
                &profile_dir,
                CodexHistoryProvider::LocalGateway,
            )?;
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
            rollback_history_on_error(&state, history_backup.as_deref(), attached)
        })?;
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
