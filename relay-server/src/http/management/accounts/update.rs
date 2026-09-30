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
    let mut record = find_account(&state, &id)?;
    let old = record.clone();
    if let Some(value) = input.label {
        record.label = clean_label(&value, "account label")?;
    }
    if let Some(value) = input.enabled {
        record.enabled = value;
    }
    if let Some(value) = input.in_pool {
        record.in_pool = value;
    }
    if let Some(value) = input.draining {
        record.draining = value;
    }
    if let Some(value) = input.allowed_models {
        record.allowed_models = normalized_values(value);
    }
    if let Some(value) = input.excluded_models {
        record.excluded_models = normalized_values(value);
    }
    if let Some(value) = input.priority {
        record.priority = value;
    }
    if let Some(value) = input.weight {
        record.weight = valid_weight(value)?;
    }
    if let Some(value) = input.purchase_cost_micro_usd {
        if value > MAX_PURCHASE_COST_MICRO_USD {
            return Err(ManagementError::validation(
                error_codes::ACCOUNT_PURCHASE_COST_INVALID,
                "account purchase cost is too large",
            ));
        }
        record.purchase_cost_micro_usd = (value > 0).then_some(value);
    }
    let policy_changed = account_runtime_policy_changed(&old, &record);
    let runtime = state.runtime().map_err(runtime_error)?;
    let _dispatch_fence = if account_dispatch_permission_changed(&old, &record) {
        runtime
            .as_ref()
            .and_then(|runtime| runtime.fence_candidate_dispatch(&record.id))
    } else {
        None
    };
    state.store.save_account(&record).map_err(store_error)?;
    let runtime_applied = if policy_changed || old.in_pool != record.in_pool {
        match apply_account_policy_if_running(&state, &record) {
            Ok(applied) => applied,
            Err(error) => {
                build
                    .rollback_and_rebuild(&state, || state.store.save_account(&old))
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
            .rebuild_or_rollback(&state, || state.store.save_account(&old))
            .await
            .map_err(runtime_error)?;
    }
    Ok(Json(account_summary(&state, &record)?))
}
