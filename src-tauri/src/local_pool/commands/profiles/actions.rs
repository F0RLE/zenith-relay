use super::*;

mod support;
pub(in crate::local_pool::commands) use support::verify_remote_profile_binding;
use support::{activate_account_profile, resolve_profile_dir};
pub(super) use support::{
    append_profile_rollback_error, append_remote_cleanup_error, profile_rotation_commit_state,
    set_runtime_pool_interface_reserve,
};

#[tauri::command]
pub async fn restore_codex_profile(state: State<'_, DesktopState>) -> Result<(), CommandError> {
    let _mutation = state.setup_guard().await;
    let profile_dir = default_codex_home();
    if codex::credential_kind(&profile_dir, &state.profile_backup_root())?.is_none() {
        return Ok(());
    }
    let sync_history =
        history_provider_changed(&state, &profile_dir, CodexHistoryProvider::ChatGpt)
            .map_err(|message| LocalPoolError::new(ErrorCode::RecoveryRequired, message))?;
    let stopped = stop_codex_and_sync_account(&state).await?;
    let restore_result = (|| {
        let history_backup = if sync_history {
            synchronize_history_for_command(&state, &profile_dir, CodexHistoryProvider::ChatGpt)?
        } else {
            None
        };
        let profile_restore_result =
            codex::restore(&profile_dir, &state.profile_backup_root()).map_err(Into::into);
        rollback_history_on_error(&state, history_backup.as_deref(), profile_restore_result)
    })();
    if restore_result.is_ok() {
        set_runtime_pool_interface_reserve(&state, None, 0).await;
    }
    restart_codex_after_restore(stopped, restore_result, launch_codex_with_profile)
}

pub(crate) async fn prepare_ready_api_profile(state: &DesktopState) -> Result<bool, CommandError> {
    let _mutation = state.setup_guard().await;
    // Detach and attach belong to the same profile transaction, not a
    // preparatory UI call that can discard the active connection on failure.
    stop_codex_and_sync_account(state).await
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
    // Catalog overrides are loaded by Codex on startup. Finish deferred
    // updates before launching; do not rescan or rewrite conversation history.
    if is_codex_running() {
        return Ok(());
    }
    process::launch_after_catalog_refresh(
        state.catalog_refresh_warning().is_some(),
        catalog::refresh_active_codex_catalog(&state),
        |catalog_refresh_result| {
            super::super::record_catalog_refresh_result(&state, catalog_refresh_result)
        },
        launch_codex_with_profile,
    )
    .await
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
    let source_record = state
        .store()?
        .source(&source_id)
        .cloned()
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "source not found"))?;
    let response_models = validate_direct_source(&source_record)?;
    let api_key = load_direct_source_api_key(
        &source_record.base_url,
        &source_record.secret_ref,
        secret_store::load,
        load_api_key_for_launch,
        secret_store::save,
    )?;
    let profile_dir = default_codex_home();
    let catalog = codex::direct_source_model_catalog_with_capabilities(
        &profile_dir,
        &response_models,
        &state.model_metadata_catalog(),
    )?;
    if catalog.is_none() {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "source has no compatible text models",
        )
        .into());
    }
    let sync_history =
        history_provider_changed(&state, &profile_dir, CodexHistoryProvider::LocalGateway)
            .map_err(|message| LocalPoolError::new(ErrorCode::RecoveryRequired, message))?;
    let stopped = stop_codex_and_sync_account(&state).await?;
    let attach_result = (|| {
        let history_backup = if sync_history {
            synchronize_history_for_command(
                &state,
                &profile_dir,
                CodexHistoryProvider::LocalGateway,
            )?
        } else {
            None
        };
        let profile_attach_result = codex::attach_with_catalog(
            &profile_dir,
            &state.profile_backup_root(),
            &source_record.id,
            &source_record.base_url,
            &api_key,
            catalog.as_deref().expect("validated direct source catalog"),
        )
        .map(|binding| ProfileActivation { binding })
        .map_err(Into::into);
        rollback_history_on_error(&state, history_backup.as_deref(), profile_attach_result)
    })();
    let activation_result =
        restart_codex_after_failed_change(stopped, attach_result, launch_codex_with_profile);
    if activation_result.is_ok() {
        set_runtime_pool_interface_reserve(&state, None, 0).await;
        state.record_catalog_refresh_result(None);
    }
    activation_result
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
    let sync_history =
        history_provider_changed(&state, &profile_dir, CodexHistoryProvider::ChatGpt)
            .map_err(|message| LocalPoolError::new(ErrorCode::RecoveryRequired, message))?;
    let stopped = stop_codex_and_sync_account_at(&state, &profile_dir).await?;
    let restore_result = (|| {
        let history_backup = if sync_history {
            synchronize_history_for_command(&state, &profile_dir, CodexHistoryProvider::ChatGpt)?
        } else {
            None
        };
        let profile_restore_result =
            codex::restore_account_profile(&profile_dir, &state.profile_backup_root())
                .map_err(Into::into);
        rollback_history_on_error(&state, history_backup.as_deref(), profile_restore_result)
    })();
    restart_codex_after_restore(stopped, restore_result, launch_codex_with_profile)
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
    let snapshot_create_result =
        snapshots::create(&default_codex_home(), &state.profile_backup_root(), &name)
            .map_err(Into::into);
    restart_codex_after_restore(stopped, snapshot_create_result, launch_codex_with_profile)
}

#[tauri::command]
pub async fn restore_full_codex_profile_snapshot(
    snapshot_id: String,
    state: State<'_, DesktopState>,
) -> Result<(), CommandError> {
    let _mutation = state.setup_guard().await;
    let stopped = stop_codex_and_sync_account(&state).await?;
    let snapshot_restore_result = snapshots::restore_full(
        &default_codex_home(),
        &state.profile_backup_root(),
        &snapshot_id,
    )
    .map_err(Into::into);
    restart_codex_after_restore(stopped, snapshot_restore_result, launch_codex_with_profile)
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
    let restore_all_result: Result<(), CommandError> = async {
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
            let sync_history =
                history_provider_changed(state, profile, CodexHistoryProvider::ChatGpt)
                    .map_err(|message| LocalPoolError::new(ErrorCode::RecoveryRequired, message))?;
            let history_backup = if sync_history {
                synchronize_history_for_command(state, profile, CodexHistoryProvider::ChatGpt)?
            } else {
                None
            };
            let restored = codex::restore(profile, &backup_root).map_err(Into::into);
            rollback_history_on_error(state, history_backup.as_deref(), restored)?;
        }
        Ok(())
    }
    .await;
    let restart_result =
        restart_codex_after_restore(stopped, restore_all_result, launch_codex_with_profile);
    if restart_result.is_ok() {
        set_runtime_pool_interface_reserve(state, None, 0).await;
    }
    restart_result
}
