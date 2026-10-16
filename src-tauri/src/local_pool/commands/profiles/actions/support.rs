use super::super::*;

pub(super) async fn activate_account_profile(
    account_id: &str,
    profile_dir: Option<String>,
    state: &DesktopState,
) -> Result<ProfileActivation, CommandError> {
    let profile_dir = resolve_profile_dir(profile_dir)?;
    let sync_history = history_provider_changed(state, &profile_dir, CodexHistoryProvider::ChatGpt)
        .map_err(|message| LocalPoolError::new(ErrorCode::RecoveryRequired, message))?;
    // Validate and refresh the account before stopping ChatGPT. A bad or
    // expired credential must leave the current client session untouched.
    let prepared = prepare_account_credentials(state, account_id).await?;
    if !prepared.supports_native_codex() {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "Excel OAuth accounts can only be used through the Relay pool",
        )
        .into());
    }
    let stopped = stop_codex_and_sync_account_at(state, &profile_dir).await?;
    let activation_result: Result<ProfileActivation, CommandError> = async {
        let history_backup = if sync_history {
            synchronize_history_for_command(state, &profile_dir, CodexHistoryProvider::ChatGpt)?
        } else {
            None
        };
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

pub(in crate::local_pool::commands) fn verify_remote_profile_binding(
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

pub(in crate::local_pool::commands::profiles) fn append_profile_rollback_error(
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

pub(in crate::local_pool::commands::profiles) fn append_remote_cleanup_error(
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

pub(in crate::local_pool::commands::profiles) fn profile_rotation_commit_state(
    observed: Option<&RemoteProfileCredential>,
    active_credential: &RemoteProfileCredential,
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
    } else if observed.key_id == active_credential.key_id
        && observed.base_url == active_credential.base_url
        && observed.secret == active_credential.secret
    {
        ProfileRotationCommitState::NotCommitted
    } else {
        ProfileRotationCommitState::Unknown
    }
}

pub(super) fn resolve_profile_dir(profile_dir: Option<String>) -> Result<PathBuf, CommandError> {
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
