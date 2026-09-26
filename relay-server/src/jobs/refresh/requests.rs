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

pub(crate) async fn refresh_all_accounts_now(
    state: &Arc<AppState>,
) -> Result<(usize, usize), String> {
    let results = stream::iter(state.store.accounts()?.into_iter().map(|account| {
        let state = state.clone();
        async move { refresh_account_now(&state, account).await }
    }))
    // This bounds retained callers only; all HTTP admission, including other
    // concurrent batches and background work, belongs to the shared service.
    .buffer_unordered(16)
    .collect::<Vec<_>>()
    .await;
    let refreshed = results
        .iter()
        .filter(|result| {
            result
                .as_ref()
                .is_ok_and(|account| account.quota.error.is_none())
        })
        .count();
    Ok((refreshed, results.len() - refreshed))
}
