use super::super::*;

pub(super) async fn activate_account_profile(
    account_id: &str,
    state: &DesktopState,
) -> Result<ProfileActivation, CommandError> {
    let profile_dir = default_codex_home();
    // Validate and refresh the account before stopping ChatGPT.
    let prepared = prepare_account_credentials(state, account_id).await?;
    if !prepared.supports_native_codex() {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "SIWC and Excel OAuth accounts can only be used through the Relay pool",
        )
        .into());
    }
    let stopped = stop_codex_and_sync_account_at(state, &profile_dir).await?;
    let activation_result: Result<ProfileActivation, CommandError> = async {
        let history_backup =
            synchronize_history_for_command(state, &profile_dir, CodexHistoryProvider::ChatGpt)?;
        let attached = codex::attach_account_explicit(
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
    let restart_result =
        restart_codex_after_failed_change(stopped, activation_result, launch_codex_with_profile);
    if restart_result.is_ok() {
        set_runtime_pool_interface_reserve(state, None, 0).await;
    }
    restart_result
}

pub(in crate::local_pool::commands::profiles) async fn set_runtime_pool_interface_reserve(
    state: &DesktopState,
    account_id: Option<&str>,
    reserve_basis_points: u64,
) {
    if let Some(runtime) = state.gateway.runtime().await {
        runtime.set_protected_candidate(account_id, reserve_basis_points);
    }
}
