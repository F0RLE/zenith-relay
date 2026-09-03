use super::ManagementError;
use crate::state::AppState;
use axum::extract::State;
use axum::routing::post;
use axum::{Json, Router};
use std::sync::Arc;
use zenith_relay_core::pricing::CatalogRefreshOutcome;

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new().route("/pricing/refresh", post(refresh))
}

/// Refreshes the server-owned LiteLLM cache without exposing the catalog
/// document or upstream response details through the management API.
async fn refresh(
    State(state): State<Arc<AppState>>,
) -> Result<Json<CatalogRefreshOutcome>, ManagementError> {
    state
        .pricing_loader()
        .refresh(true)
        .await
        .map(Json)
        .map_err(|_| {
            ManagementError::internal(
                "pricing_catalog_refresh_failed",
                "pricing catalog could not be refreshed",
            )
        })
}
