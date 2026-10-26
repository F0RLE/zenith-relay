use super::*;
use zenith_relay_core::protocol::TrashedAccountSummary;

#[tauri::command]
pub async fn list_local_account_trash(
    state: State<'_, DesktopState>,
) -> CommandResult<Vec<TrashedAccountSummary>> {
    let state = state.inner().clone();
    crate::local_pool::commands::run_blocking(move || {
        Ok(state
            .store()?
            .account_trash()
            .iter()
            .map(|entry| TrashedAccountSummary {
                id: entry.record.account.id.clone(),
                label: entry.record.account.label.clone(),
                trashed_at_ms: entry.trashed_at_ms,
            })
            .collect())
    })
    .await
    .map_err(Into::into)
}

#[tauri::command]
pub async fn trash_local_accounts(
    account_ids: Vec<String>,
    app: AppHandle,
    state: State<'_, DesktopState>,
) -> CommandResult<()> {
    let account_ids = normalize_account_ids(account_ids)?;
    let mutation = state.setup_guard().await;
    ensure_accounts_exist(state.store()?.accounts(), &account_ids)?;
    let runtime = state.gateway.runtime().await;
    let _dispatch_fences = fence_runtime_candidates(runtime.as_deref(), &account_ids, &[]);
    let account_id_refs = account_ids.iter().map(String::as_str).collect::<Vec<_>>();
    let _credential_guards = acquire_delete_credential_guards(&state, &account_id_refs).await?;
    let credentials = CredentialStore::from_backend(NativeSecretBackend);
    let initial_wake = state.wake_snapshot()?;
    let initial_automations = state.store()?.automations().clone();
    let initial_proxy_pool = ProxyPool::load()?;
    let mut automations = initial_automations.clone();
    for account_id in &account_ids {
        automations = prune_account_task_selectors(automations, account_id);
    }
    let mut prepared = Vec::with_capacity(account_ids.len());
    let removal_result = async {
        for account_id in &account_ids {
            prepared.push(
                prepare::prepare_trash_local_account(account_id, &state, &credentials).await?,
            );
        }
        state.store()?.trash_accounts(
            &account_ids,
            automations,
            crate::local_pool::runtime::current_time_ms(),
        )
    }
    .await;
    if let Err(error) = removal_result {
        ensure_delete_rollback_or_fail_closed(
            &state,
            rollback_batch_delete(
                &state,
                &credentials,
                &prepared,
                initial_wake,
                initial_automations,
                &initial_proxy_pool,
                &error,
            ),
        )
        .await?;
        return Err(error.into());
    }
    for account_id in &account_ids {
        if let Some(runtime) = &runtime {
            runtime.remove_candidate(account_id);
        }
        state.token_authority().remove(account_id);
        state.remove_account_refresh(account_id);
        let _ = state.remove_quota_account_lock(account_id);
    }
    drop(mutation);
    refresh_active_codex_catalog_in_background(app);
    Ok(())
}

#[tauri::command]
pub async fn restore_local_account_trash(
    account_ids: Vec<String>,
    state: State<'_, DesktopState>,
) -> CommandResult<()> {
    let account_ids = normalize_account_ids(account_ids)?;
    let _mutation = state.setup_guard().await;
    state.store()?.restore_trashed_accounts(&account_ids)?;
    Ok(())
}

#[tauri::command]
pub async fn purge_local_account_trash(
    account_ids: Vec<String>,
    state: State<'_, DesktopState>,
) -> CommandResult<()> {
    let account_ids = normalize_account_ids(account_ids)?;
    let _mutation = state.setup_guard().await;
    {
        let store = state.store()?;
        for account_id in &account_ids {
            if !store
                .account_trash()
                .iter()
                .any(|entry| entry.record.account.id == *account_id)
            {
                return Err(
                    LocalPoolError::new(ErrorCode::NotFound, "trashed account not found").into(),
                );
            }
        }
    }
    let account_id_refs = account_ids.iter().map(String::as_str).collect::<Vec<_>>();
    let _credential_guards = acquire_delete_credential_guards(&state, &account_id_refs).await?;
    let credentials = CredentialStore::from_backend(NativeSecretBackend);
    let previous_credentials = account_ids
        .iter()
        .map(|account_id| credentials.load(account_id).map_err(credential_local_error))
        .collect::<LocalResult<Vec<_>>>()?;
    let delete_result = (|| -> LocalResult<()> {
        for account_id in &account_ids {
            credentials
                .delete(account_id)
                .map_err(credential_local_error)?;
        }
        state.store()?.purge_trashed_accounts(&account_ids)
    })();
    if let Err(error) = delete_result {
        let rollback = previous_credentials.iter().try_for_each(|previous| {
            cleanup::restore_credential_local(&credentials, previous.as_ref(), &error)
        });
        ensure_delete_rollback_or_fail_closed(&state, rollback).await?;
        return Err(error.into());
    }
    for account_id in &account_ids {
        state.token_authority().remove(account_id);
    }
    Ok(())
}
