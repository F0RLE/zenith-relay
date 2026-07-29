use super::{store_error, ManagementError};
use crate::state::AppState;
use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use std::sync::Arc;
use zenith_relay_core::quota::QuotaSnapshot;

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new().route("/quota", get(quota))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaItem {
    account_id: String,
    quota: QuotaSnapshot,
}

pub async fn quota(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<QuotaItem>>, ManagementError> {
    Ok(Json(
        state
            .store
            .accounts()
            .map_err(store_error)?
            .into_iter()
            .map(|record| QuotaItem {
                account_id: record.id,
                quota: record.quota,
            })
            .collect(),
    ))
}
