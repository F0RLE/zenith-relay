mod batch;
mod confirm;
mod preview;
mod probe;

#[cfg(test)]
mod tests;

use crate::state::AppState;
use axum::routing::post;
use axum::Router;
use std::sync::Arc;

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/accounts/import/preview",
            post(preview::preview_account_import),
        )
        .route(
            "/accounts/import/confirm",
            post(confirm::confirm_account_import),
        )
        .route(
            "/accounts/import/batch/preview",
            post(batch::preview_account_batch_import),
        )
        .route(
            "/accounts/import/batch/confirm",
            post(batch::confirm_account_batch_import),
        )
}
