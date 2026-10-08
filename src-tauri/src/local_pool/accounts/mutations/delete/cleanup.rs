use crate::local_pool::accounts::credentials::{CredentialStore, StoredCodexCredentials};
use crate::local_pool::accounts::proxy::ProxyPool;
use crate::local_pool::accounts::NativeSecretBackend;
use crate::local_pool::commands::current_time_ms;
use crate::local_pool::error::{ErrorCode, LocalPoolError, Result as LocalResult};
use crate::local_pool::models::AutomationRecords;
use crate::local_pool::profiles::codex;
use crate::local_pool::state::DesktopState;
use std::path::Path;
use zenith_relay_core::automations::AccountSelector;

pub(in crate::local_pool::accounts) fn prune_account_task_selectors(
    mut automations: AutomationRecords,
    account_id: &str,
) -> AutomationRecords {
    let now_ms = current_time_ms();
    automations.tasks.retain_mut(|task| {
        let AccountSelector::AccountIds(account_ids) = &mut task.account_selector else {
            return true;
        };
        if !account_ids.remove(account_id) {
            return true;
        }
        task.updated_at_ms = now_ms;
        !account_ids.is_empty()
    });
    automations
}

pub(super) fn restore_credential_local(
    credential_store: &CredentialStore<NativeSecretBackend>,
    old_credential: Option<&StoredCodexCredentials>,
    cause: &LocalPoolError,
) -> LocalResult<()> {
    let restored = match old_credential {
        Some(credentials) => credential_store.save(credentials),
        None => Ok(()),
    };
    restored.map_err(|_| {
        LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            format!(
                "{}; failed to restore previous account credentials",
                cause.message
            ),
        )
    })?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(in crate::local_pool::accounts) fn rollback_deleted_account_side_effects(
    state: &DesktopState,
    credential_store: &CredentialStore<NativeSecretBackend>,
    old_credential: Option<&StoredCodexCredentials>,
    previous_wake: zenith_relay_core::automations::WakeCoordinator,
    old_automations: AutomationRecords,
    restored_bindings: &[codex::ProfileBinding],
    previous_proxy_pool: Option<&ProxyPool>,
    cause: &LocalPoolError,
) -> LocalResult<()> {
    restore_credential_local(credential_store, old_credential, cause)?;
    state.store()?.notify_refresh_changed();
    state
        .restore_wake(previous_wake, old_automations)
        .map_err(|error| recovery_after_delete(cause, "wake state", error))?;
    reattach_account_profiles(state, restored_bindings, old_credential, cause)?;
    if let Some(pool) = previous_proxy_pool {
        pool.save()
            .map_err(|error| recovery_after_delete(cause, "proxy assignment", error))?;
    }
    Ok(())
}

pub(in crate::local_pool::accounts) fn restore_bound_account_profiles(
    state: &DesktopState,
    bindings: &[codex::ProfileBinding],
    credentials: Option<&StoredCodexCredentials>,
) -> LocalResult<Vec<codex::ProfileBinding>> {
    let mut restored = Vec::with_capacity(bindings.len());
    for binding in bindings {
        match codex::restore_account_profile(
            Path::new(&binding.profile_dir),
            &state.profile_backup_root(),
        ) {
            Ok(Some(binding)) => restored.push(binding),
            Ok(None) => {
                let error = LocalPoolError::new(
                    ErrorCode::RecoveryRequired,
                    "account profile binding disappeared during deletion",
                );
                reattach_account_profiles(state, &restored, credentials, &error)?;
                return Err(error);
            }
            Err(error) => {
                reattach_account_profiles(state, &restored, credentials, &error)?;
                return Err(error);
            }
        }
    }
    Ok(restored)
}

pub(super) fn reattach_account_profiles(
    state: &DesktopState,
    bindings: &[codex::ProfileBinding],
    credentials: Option<&StoredCodexCredentials>,
    cause: &LocalPoolError,
) -> LocalResult<()> {
    if bindings.is_empty() {
        return Ok(());
    }
    let credentials = credentials.ok_or_else(|| {
        LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            format!("{}; account profile credentials are missing", cause.message),
        )
    })?;
    let tokens = credentials.to_token_set().map_err(|_| {
        LocalPoolError::new(ErrorCode::RecoveryRequired, "account tokens are invalid")
    })?;
    let provider_account_id = credentials.provider_account_id().ok_or_else(|| {
        LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "account provider identity is missing",
        )
    })?;
    for binding in bindings {
        codex::attach_account(
            Path::new(&binding.profile_dir),
            &state.profile_backup_root(),
            &binding.credential_id,
            &tokens,
            provider_account_id,
        )
        .map_err(|error| recovery_after_delete(cause, "profile binding", error))?;
    }
    Ok(())
}

pub(super) fn recovery_after_delete(
    cause: &LocalPoolError,
    state: &str,
    error: LocalPoolError,
) -> LocalPoolError {
    LocalPoolError::new(
        ErrorCode::RecoveryRequired,
        format!(
            "{}; failed to restore account {state}: {}",
            cause.message, error.message
        ),
    )
}
