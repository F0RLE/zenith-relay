use super::*;

pub(in crate::local_pool::accounts) fn ensure_accounts_exist(
    accounts: &[LocalAccountRecord],
    account_ids: &[String],
) -> LocalResult<()> {
    if let Some(account_id) = account_ids.iter().find(|account_id| {
        !accounts
            .iter()
            .any(|account| account.account.id == **account_id)
    }) {
        return Err(LocalPoolError::new(
            ErrorCode::NotFound,
            format!("account not found: {account_id}"),
        ));
    }
    Ok(())
}

pub(super) fn ensure_accounts_not_in_ownership_operation(
    state: &DesktopState,
    account_ids: &[String],
) -> CommandResult<()> {
    if state
        .store()?
        .ownership_operation()
        .is_some_and(|operation| {
            operation
                .local_account_ids
                .iter()
                .any(|account_id| account_ids.contains(account_id))
        })
    {
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "account ownership recovery must finish before deleting this local record",
        )
        .into());
    }
    Ok(())
}

pub(super) async fn delete_local_account_inner(
    account_id: &str,
    state: &DesktopState,
) -> CommandResult<()> {
    let runtime = state.gateway.runtime().await;
    let _dispatch_fences =
        fence_runtime_candidates(runtime.as_deref(), &[account_id.to_string()], &[]);
    let _credential_guards = acquire_delete_credential_guards(state, &[account_id]).await?;
    let credentials = CredentialStore::from_backend(NativeSecretBackend);
    let (old_accounts, old_keys, old_automations) = current_account_state(state)?;
    let deleted = prepare_delete_local_account(account_id, state, &credentials).await?;
    let accounts = old_accounts
        .iter()
        .filter(|account| account.account.id != account_id)
        .cloned()
        .collect::<Vec<_>>();
    let automations = prune_account_task_selectors(old_automations, account_id);
    let delete_result = state.store().and_then(|mut store| {
        store.delete_account_state(account_id, accounts, old_keys, automations)
    });
    if let Err(error) = delete_result {
        ensure_delete_rollback_or_fail_closed(
            state,
            rollback_prepared_delete(state, &credentials, &deleted, &error),
        )
        .await?;
        return Err(error.into());
    }
    if let Some(runtime) = state.gateway.runtime().await {
        runtime.remove_candidate(account_id);
    }
    state.token_authority().remove(account_id);
    let _ = state.remove_quota_account_lock(account_id);
    Ok(())
}

/// A token refresh owns this process lock until its secret write finishes.
/// Acquire it before any delete side effects and keep it through rollback or
/// authority retirement, so an old refresh cannot recreate a deleted secret.
pub(super) async fn acquire_delete_credential_guards(
    state: &DesktopState,
    account_ids: &[&str],
) -> LocalResult<Vec<ProcessAccountGuard>> {
    let locks =
        ProcessAccountLocks::with_config(state.transient_root(), ProcessLockConfig::default())
            .map_err(|_| {
                LocalPoolError::invalid_state("account credential locks are unavailable")
            })?;
    let mut sorted_account_ids = account_ids.to_vec();
    sorted_account_ids.sort_unstable();
    let mut guards = Vec::with_capacity(sorted_account_ids.len());
    for account_id in sorted_account_ids {
        guards.push(locks.acquire(account_id).await.map_err(|error| {
            let code = if error == ProcessLockError::Timeout {
                ErrorCode::Conflict
            } else {
                ErrorCode::Io
            };
            LocalPoolError::new(
                code,
                "account credential lock is unavailable; retry deletion",
            )
        })?);
    }
    Ok(guards)
}

pub(super) struct PreparedAccountDelete {
    pub(super) old_credential: Option<StoredCodexCredentials>,
    pub(super) previous_wake: zenith_relay_core::automations::WakeCoordinator,
    pub(super) old_automations: AutomationRecords,
    pub(super) restored_bindings: Vec<codex::ProfileBinding>,
    pub(super) previous_proxy_pool: Option<ProxyPool>,
}

pub(super) async fn prepare_delete_local_account(
    account_id: &str,
    state: &DesktopState,
    credentials: &CredentialStore<NativeSecretBackend>,
) -> LocalResult<PreparedAccountDelete> {
    let old_accounts = state.store()?.accounts().to_vec();
    if !old_accounts
        .iter()
        .any(|account| account.account.id == account_id)
    {
        return Err(LocalPoolError::new(
            ErrorCode::NotFound,
            "account not found",
        ));
    }
    let old_credential = credentials
        .load(account_id)
        .map_err(credential_local_error)?;
    let old_automations = state.store()?.automations().clone();
    let previous_wake = state.wake_snapshot()?;
    let bindings = codex::account_bindings(&state.profile_backup_root())?
        .into_iter()
        .filter(|binding| binding.credential_id == account_id)
        .collect::<Vec<_>>();
    if !bindings.is_empty() && old_credential.is_none() {
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "account profile binding exists without stored credentials",
        ));
    }
    state.store()?.invalidate_account_refresh(&[account_id])?;
    state.remove_account_refresh(account_id);
    let restored_bindings =
        match restore_bound_account_profiles(state, &bindings, old_credential.as_ref()) {
            Ok(restored) => restored,
            // A failed profile reattach may leave credentials and the saved route
            // out of sync. A recovery error must not reopen the old runtime.
            Err(error) if error.code == ErrorCode::RecoveryRequired => {
                return Err(fail_closed(state, error.to_string()).await);
            }
            Err(error) => return Err(error),
        };
    if let Err(error) = state.remove_pending_wakes_for_account(account_id) {
        return rollback_failed_delete_step(
            state,
            credentials,
            old_credential.as_ref(),
            previous_wake,
            old_automations,
            &restored_bindings,
            None,
            error,
        )
        .await;
    }
    let previous_proxy_pool = match release_account_proxy(account_id) {
        Ok(previous_account) => previous_account,
        Err(error) => {
            return rollback_failed_delete_step(
                state,
                credentials,
                old_credential.as_ref(),
                previous_wake,
                old_automations,
                &restored_bindings,
                None,
                error,
            )
            .await;
        }
    };
    if let Err(error) = credentials
        .delete(account_id)
        .map_err(credential_local_error)
    {
        return rollback_failed_delete_step(
            state,
            credentials,
            old_credential.as_ref(),
            previous_wake,
            old_automations,
            &restored_bindings,
            previous_proxy_pool.as_ref(),
            error,
        )
        .await;
    }
    Ok(PreparedAccountDelete {
        old_credential,
        previous_wake,
        old_automations,
        restored_bindings,
        previous_proxy_pool,
    })
}

/// Keep the dispatch fence held until an incomplete delete rollback has
/// retired the old runtime and stopped its listener. A failed credential or
/// profile restore must never turn into a newly dispatchable old account.
pub(super) async fn ensure_delete_rollback_or_fail_closed(
    state: &DesktopState,
    rollback: LocalResult<()>,
) -> LocalResult<()> {
    match rollback {
        Ok(()) => Ok(()),
        Err(error) => Err(fail_closed(state, error.to_string()).await),
    }
}

#[allow(clippy::too_many_arguments)]
async fn rollback_failed_delete_step(
    state: &DesktopState,
    credentials: &CredentialStore<NativeSecretBackend>,
    old_credential: Option<&StoredCodexCredentials>,
    previous_wake: zenith_relay_core::automations::WakeCoordinator,
    old_automations: AutomationRecords,
    restored_bindings: &[codex::ProfileBinding],
    previous_proxy_pool: Option<&ProxyPool>,
    error: LocalPoolError,
) -> LocalResult<PreparedAccountDelete> {
    ensure_delete_rollback_or_fail_closed(
        state,
        rollback_deleted_account_side_effects(
            state,
            credentials,
            old_credential,
            previous_wake,
            old_automations,
            restored_bindings,
            previous_proxy_pool,
            &error,
        ),
    )
    .await?;
    Err(error)
}

pub(super) fn rollback_prepared_delete(
    state: &DesktopState,
    credentials: &CredentialStore<NativeSecretBackend>,
    deleted: &PreparedAccountDelete,
    cause: &LocalPoolError,
) -> LocalResult<()> {
    rollback_deleted_account_side_effects(
        state,
        credentials,
        deleted.old_credential.as_ref(),
        deleted.previous_wake.clone(),
        deleted.old_automations.clone(),
        &deleted.restored_bindings,
        deleted.previous_proxy_pool.as_ref(),
        cause,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn rollback_batch_delete(
    state: &DesktopState,
    credentials: &CredentialStore<NativeSecretBackend>,
    deleted: &[PreparedAccountDelete],
    previous_wake: zenith_relay_core::automations::WakeCoordinator,
    old_automations: AutomationRecords,
    previous_proxy_pool: &ProxyPool,
    cause: &LocalPoolError,
) -> LocalResult<()> {
    for account in deleted.iter().rev() {
        cleanup::restore_credential_local(credentials, account.old_credential.as_ref(), cause)?;
        cleanup::reattach_account_profiles(
            state,
            &account.restored_bindings,
            account.old_credential.as_ref(),
            cause,
        )?;
    }
    state.store()?.notify_refresh_changed();
    state
        .restore_wake(previous_wake, old_automations)
        .map_err(|error| cleanup::recovery_after_delete(cause, "wake state", error))?;
    previous_proxy_pool
        .save()
        .map_err(|error| cleanup::recovery_after_delete(cause, "proxy assignment", error))?;
    Ok(())
}

pub(super) fn release_account_proxy(account_id: &str) -> LocalResult<Option<ProxyPool>> {
    let previous_proxy_pool = ProxyPool::load()?;
    let mut updated_proxy_pool = previous_proxy_pool.clone();
    updated_proxy_pool.release(account_id);
    if updated_proxy_pool == previous_proxy_pool {
        return Ok(None);
    }
    updated_proxy_pool.save()?;
    Ok(Some(previous_proxy_pool))
}

pub(super) fn current_account_state(
    state: &DesktopState,
) -> LocalResult<(
    Vec<LocalAccountRecord>,
    Vec<LocalGatewayKeyRecord>,
    AutomationRecords,
)> {
    let store = state.store()?;
    Ok((
        store.accounts().to_vec(),
        store.keys().to_vec(),
        store.automations().clone(),
    ))
}
