use super::super::{runtime_error, store_error, ManagementError};
use super::policy::apply_account_policies_if_running;
use crate::state::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use std::collections::BTreeSet;
use std::sync::Arc;
use zenith_relay_core::error_codes;
use zenith_relay_core::protocol::RuntimeStateSnapshot;

pub(super) use zenith_relay_core::protocol::PoolMembershipInput;

pub(super) async fn set_pool_membership(
    State(state): State<Arc<AppState>>,
    Json(input): Json<PoolMembershipInput>,
) -> Result<Json<RuntimeStateSnapshot>, ManagementError> {
    let account_ids = input.account_ids.into_iter().collect::<BTreeSet<_>>();
    let source_ids = input.source_ids.into_iter().collect::<BTreeSet<_>>();
    if account_ids.is_empty() && source_ids.is_empty() {
        return Err(ManagementError::validation(
            error_codes::POOL_MEMBERS_EMPTY,
            "at least one pool member is required",
        ));
    }
    if account_ids.len().saturating_add(source_ids.len()) > 2_048 {
        return Err(ManagementError::validation(
            error_codes::POOL_MEMBERS_TOO_MANY,
            "too many pool members were requested",
        ));
    }

    // Serialize validation, durable membership and runtime publication with
    // single-member edits. Final dispatch has no access to this host lock, so
    // changed members also need physical candidate fences before the commit.
    let _configuration = state.configuration_lock.lock().await;
    let build = state.lock_runtime_rebuild().await;
    let accounts = state.store.accounts().map_err(store_error)?;
    let sources = state.store.sources().map_err(store_error)?;
    let previous_accounts = account_ids
        .iter()
        .map(|account_id| {
            accounts
                .iter()
                .find(|account_record| &account_record.id == account_id)
                .map(|account_record| (account_id.clone(), account_record.in_pool))
                .ok_or_else(|| {
                    ManagementError::not_found(error_codes::ACCOUNT_NOT_FOUND, "account not found")
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let previous_sources = source_ids
        .iter()
        .map(|source_id| {
            sources
                .iter()
                .find(|source_record| &source_record.id == source_id)
                .map(|source_record| (source_id.clone(), source_record.in_pool))
                .ok_or_else(|| {
                    ManagementError::not_found(error_codes::SOURCE_NOT_FOUND, "source not found")
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    if input.in_pool {
        for source_id in &source_ids {
            let source_record = sources
                .iter()
                .find(|source_record| &source_record.id == source_id)
                .expect("source was validated above");
            if !source_record.supports_any_wire_api().map_err(|message| {
                ManagementError::validation(error_codes::SOURCE_PROTOCOL_INVALID, message)
            })? {
                return Err(ManagementError::new(
                    StatusCode::CONFLICT,
                    error_codes::SOURCE_POOL_PROTOCOL_UNSUPPORTED,
                    "source must expose at least one verified API route before joining the pool",
                    "pool",
                    false,
                ));
            }
        }
    }
    let updated_accounts = account_ids
        .iter()
        .map(|account_id| (account_id.clone(), input.in_pool))
        .collect::<Vec<_>>();
    let updated_sources = source_ids
        .iter()
        .map(|source_id| (source_id.clone(), input.in_pool))
        .collect::<Vec<_>>();
    let _dispatch_fences = state.runtime().map_err(runtime_error)?.map(|runtime| {
        let mut fences = previous_accounts
            .iter()
            .filter(|(_, previous_membership)| *previous_membership != input.in_pool)
            .filter_map(|(account_id, _)| runtime.fence_candidate_dispatch(account_id))
            .collect::<Vec<_>>();
        for (source_id, _) in previous_sources
            .iter()
            .filter(|(_, previous_membership)| *previous_membership != input.in_pool)
        {
            fences.extend(runtime.fence_source_dispatch(source_id));
        }
        fences
    });
    state
        .store
        .replace_pool_membership(&updated_sources, &updated_accounts)
        .map_err(store_error)?;
    let changed_accounts = accounts
        .iter()
        .filter(|account| account_ids.contains(&account.id))
        .cloned()
        .map(|mut account| {
            account.in_pool = input.in_pool;
            account
        })
        .collect::<Vec<_>>();
    let runtime_applied = match apply_account_policies_if_running(&state, &changed_accounts) {
        Ok(applied) => applied,
        Err(error) => {
            build
                .rollback_and_rebuild(&state, || {
                    state
                        .store
                        .replace_pool_membership(&previous_sources, &previous_accounts)
                })
                .await
                .map_err(|restore| runtime_error(format!("{error}; {restore}")))?;
            return Err(runtime_error(error));
        }
    };
    if !runtime_applied {
        build
            .rebuild_or_rollback(&state, || {
                state
                    .store
                    .replace_pool_membership(&previous_sources, &previous_accounts)
            })
            .await
            .map_err(runtime_error)?;
    }
    state.snapshot().map(Json).map_err(store_error)
}
