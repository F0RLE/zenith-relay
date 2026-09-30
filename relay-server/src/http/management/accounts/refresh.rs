use super::super::{account_summary, find_account, runtime_error, store_error, ManagementError};
use crate::jobs;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Serialize;
use std::sync::Arc;
use zenith_relay_core::error_codes;
use zenith_relay_core::protocol::{AccountSummary, RuntimeStateSnapshot};

pub(super) async fn refresh_account(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<AccountSummary>, ManagementError> {
    let record = find_account(&state, &id)?;
    let updated = jobs::refresh_account_quota_now(&state, record)
        .await
        .map_err(|_| {
            ManagementError::new(
                StatusCode::BAD_GATEWAY,
                error_codes::ACCOUNT_REFRESH_FAILED,
                "account metadata could not be refreshed",
                "quota",
                true,
            )
        })?;
    account_summary(&state, &updated).map(Json)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AccountQuotaRefreshResult {
    refreshed: usize,
    failed: usize,
    snapshot: RuntimeStateSnapshot,
}

pub(super) async fn refresh_all_account_quotas(
    State(state): State<Arc<AppState>>,
) -> Result<Json<AccountQuotaRefreshResult>, ManagementError> {
    let (refreshed, failed) = jobs::refresh_all_accounts_now(&state)
        .await
        .map_err(runtime_error)?;
    let snapshot = state.snapshot().map_err(store_error)?;
    Ok(Json(AccountQuotaRefreshResult {
        refreshed,
        failed,
        snapshot,
    }))
}
