use super::*;
use std::collections::BTreeSet;

/// Caller holds setup_guard. Split from the Tauri event wrapper so the
/// durable membership and live routing transaction can be exercised locally.
pub(in crate::local_pool::commands) async fn apply_local_pool_membership(
    input: PoolMembershipInput,
    state: &DesktopState,
) -> CommandResult<(LocalPoolSnapshot, bool, Vec<String>)> {
    let account_ids = input.account_ids.into_iter().collect::<BTreeSet<_>>();
    let source_ids = input.source_ids.into_iter().collect::<BTreeSet<_>>();
    if account_ids.is_empty() && source_ids.is_empty() {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "at least one pool member is required",
        )
        .into());
    }

    let (old_sources, old_accounts, old_keys) = {
        let store = state.store()?;
        (
            store.sources().to_vec(),
            store.accounts().to_vec(),
            store.keys().to_vec(),
        )
    };
    if source_ids.iter().any(|source_id| {
        !old_sources
            .iter()
            .any(|source_record| &source_record.id == source_id)
    }) || account_ids.iter().any(|account_id| {
        !old_accounts
            .iter()
            .any(|account_record| &account_record.account.id == account_id)
    }) {
        return Err(LocalPoolError::new(ErrorCode::NotFound, "pool member not found").into());
    }
    if input.in_pool
        && old_accounts.iter().any(|account_record| {
            account_ids.contains(&account_record.account.id)
                && account_record.remote_location.is_some()
        })
    {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "an account managed by a remote server cannot join the local pool",
        )
        .into());
    }
    if input.in_pool {
        for source in old_sources
            .iter()
            .filter(|source| source_ids.contains(&source.id))
        {
            let supports_any_protocol = source.supports_any_wire_api().unwrap_or(false);
            if !supports_any_protocol {
                return Err(LocalPoolError::new(
                    ErrorCode::Conflict,
                    "source must expose at least one verified API route before joining the local pool",
                )
                .into());
            }
        }
    }

    let mut sources = old_sources.clone();
    let mut accounts = old_accounts.clone();
    let model_refresh_account_ids = if input.in_pool {
        old_accounts
            .iter()
            .filter(|account| account_ids.contains(&account.account.id) && !account.account.in_pool)
            .map(|account| account.account.id.clone())
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    for source in &mut sources {
        if source_ids.contains(&source.id) {
            source.in_pool = input.in_pool;
        }
    }
    for account in &mut accounts {
        if account_ids.contains(&account.account.id) {
            account.account.in_pool = input.in_pool;
        }
    }
    if sources == old_sources && accounts == old_accounts {
        return Ok((state.snapshot().await?, false, Vec::new()));
    }

    let changed_accounts = accounts
        .iter()
        .filter(|account| account_ids.contains(&account.account.id))
        .cloned()
        .collect::<Vec<_>>();
    let fenced_accounts = old_accounts
        .iter()
        .filter(|account| {
            account_ids.contains(&account.account.id) && account.account.in_pool != input.in_pool
        })
        .map(|account| account.account.id.clone())
        .collect::<Vec<_>>();
    let fenced_sources = old_sources
        .iter()
        .filter(|source| source_ids.contains(&source.id) && source.in_pool != input.in_pool)
        .map(|source| source.id.clone())
        .collect::<Vec<_>>();
    let runtime = state.gateway.runtime().await;
    let _dispatch_fences =
        fence_runtime_candidates(runtime.as_deref(), &fenced_accounts, &fenced_sources);
    state
        .store()?
        .replace_pool_records(sources, accounts, old_keys.clone())?;
    let policy_now_ms = super::super::current_time_ms();
    let updated_in_place = if let Some(runtime) = state.gateway.runtime().await {
        changed_accounts.iter().all(|account| {
            runtime.update_account_policy(
                &account.account.id,
                runtime_account_policy(account, policy_now_ms),
            )
        }) && super::super::apply_local_gateway_key_scope(state, &runtime).unwrap_or(false)
    } else {
        false
    };
    if !updated_in_place {
        restart_or_rollback(state, || {
            state
                .store()?
                .replace_pool_records(old_sources, old_accounts, old_keys)
        })
        .await?;
    }
    let now_ms = policy_now_ms;
    for account_id in account_ids {
        state.sync_account_quota_refresh(&account_id, now_ms)?;
    }
    let snapshot = state.snapshot().await?;
    Ok((snapshot, updated_in_place, model_refresh_account_ids))
}
