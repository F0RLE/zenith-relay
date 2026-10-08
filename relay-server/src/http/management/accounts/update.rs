use super::super::{
    account_summary, clean_label, find_account, normalized_values, runtime_error, store_error,
    valid_weight, ManagementError,
};
use super::policy::{
    account_dispatch_permission_changed, account_runtime_policy_changed,
    apply_account_policy_if_running,
};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::Json;
use serde::Deserialize;
use std::sync::Arc;
use zenith_relay_core::accounts::MAX_PURCHASE_COST_MICRO_USD;
use zenith_relay_core::error_codes;
use zenith_relay_core::protocol::AccountSummary;

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AccountPatch {
    pub(super) label: Option<String>,
    pub(super) enabled: Option<bool>,
    pub(super) in_pool: Option<bool>,
    pub(super) draining: Option<bool>,
    pub(super) allowed_models: Option<Vec<String>>,
    pub(super) excluded_models: Option<Vec<String>>,
    pub(super) priority: Option<i32>,
    pub(super) weight: Option<u32>,
    pub(super) purchase_cost_micro_usd: Option<u64>,
}

pub(super) async fn update_account(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(input): Json<AccountPatch>,
) -> Result<Json<AccountSummary>, ManagementError> {
    // Keep other runtime publications out of the durable-save -> hot-apply
    // window. The dispatch fence is acquired before writing the new policy.
    let _configuration = state.configuration_lock.lock().await;
    let build = state.lock_runtime_rebuild().await;
    let mut account_record = find_account(&state, &id)?;
    let previous_account_record = account_record.clone();
    if let Some(label) = input.label {
        account_record.label = clean_label(&label, "account label")?;
    }
    if let Some(enabled) = input.enabled {
        account_record.enabled = enabled;
    }
    if let Some(in_pool) = input.in_pool {
        account_record.in_pool = in_pool;
    }
    if let Some(draining) = input.draining {
        account_record.draining = draining;
    }
    if let Some(allowed_models) = input.allowed_models {
        account_record.allowed_models = normalized_values(allowed_models);
    }
    if let Some(excluded_models) = input.excluded_models {
        account_record.excluded_models = normalized_values(excluded_models);
    }
    if let Some(priority) = input.priority {
        account_record.priority = priority;
    }
    if let Some(weight) = input.weight {
        account_record.weight = valid_weight(weight)?;
    }
    if let Some(purchase_cost_micro_usd) = input.purchase_cost_micro_usd {
        if purchase_cost_micro_usd > MAX_PURCHASE_COST_MICRO_USD {
            return Err(ManagementError::validation(
                error_codes::ACCOUNT_PURCHASE_COST_INVALID,
                "account purchase cost is too large",
            ));
        }
        account_record.purchase_cost_micro_usd =
            (purchase_cost_micro_usd > 0).then_some(purchase_cost_micro_usd);
    }
    let policy_changed = account_runtime_policy_changed(&previous_account_record, &account_record);
    let runtime = state.runtime().map_err(runtime_error)?;
    let _dispatch_fence =
        if account_dispatch_permission_changed(&previous_account_record, &account_record) {
            runtime
                .as_ref()
                .and_then(|runtime| runtime.fence_candidate_dispatch(&account_record.id))
        } else {
            None
        };
    state
        .store
        .save_account(&account_record)
        .map_err(store_error)?;
    let runtime_applied =
        if policy_changed || previous_account_record.in_pool != account_record.in_pool {
            match apply_account_policy_if_running(&state, &account_record) {
                Ok(applied) => applied,
                Err(error) => {
                    build
                        .rollback_and_rebuild(&state, || {
                            state.store.save_account(&previous_account_record)
                        })
                        .await
                        .map_err(|restore| runtime_error(format!("{error}; {restore}")))?;
                    return Err(runtime_error(error));
                }
            }
        } else {
            true
        };
    if !runtime_applied {
        build
            .rebuild_or_rollback(&state, || {
                state.store.save_account(&previous_account_record)
            })
            .await
            .map_err(runtime_error)?;
    }
    Ok(Json(account_summary(&state, &account_record)?))
}
