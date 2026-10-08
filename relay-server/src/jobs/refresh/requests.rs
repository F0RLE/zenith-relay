use super::{request, AppState, RefreshKind, ServerAccountRecord};
use futures_util::{stream, StreamExt};
use std::sync::Arc;

pub(crate) async fn refresh_account_now(
    state: &Arc<AppState>,
    account: ServerAccountRecord,
) -> Result<ServerAccountRecord, String> {
    // Both reads are independent jobs. Quota failure must not erase a model
    // observation or prevent its scheduled/manual refresh from completing.
    let (quota, models) = tokio::join!(
        request(state, &account.id, RefreshKind::Quota),
        request(state, &account.id, RefreshKind::Models)
    );
    quota?;
    models?;
    state
        .store
        .account(&account.id)?
        .ok_or_else(|| "account not found".into())
}

/// The explicit quota action does not wait for the independent model catalog.
/// Both kinds still use the same refresh owner and preserve their own results.
pub(crate) async fn refresh_account_quota_now(
    state: &Arc<AppState>,
    account: ServerAccountRecord,
) -> Result<ServerAccountRecord, String> {
    let quota = request(state, &account.id, RefreshKind::Quota).await;
    let models_state = state.clone();
    let models_id = account.id.clone();
    tokio::spawn(async move {
        let _ = request(&models_state, &models_id, RefreshKind::Models).await;
    });
    quota?;
    state
        .store
        .account(&account.id)?
        .ok_or_else(|| "account not found".into())
}

pub(crate) async fn refresh_all_accounts_now(
    state: &Arc<AppState>,
) -> Result<(usize, usize), String> {
    let refresh_results = stream::iter(state.store.accounts()?.into_iter().map(|account| {
        let app_state = state.clone();
        async move { refresh_account_quota_now(&app_state, account).await }
    }))
    // This bounds retained callers only; all HTTP admission, including other
    // concurrent batches and background work, belongs to the shared service.
    .buffer_unordered(16)
    .collect::<Vec<_>>()
    .await;
    let refreshed = refresh_results
        .iter()
        .filter(|refresh_result| {
            refresh_result
                .as_ref()
                .is_ok_and(|account| account.quota.error.is_none())
        })
        .count();
    Ok((refreshed, refresh_results.len() - refreshed))
}
