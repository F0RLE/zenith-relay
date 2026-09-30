use crate::state::AppState;
use axum::routing::{get, patch, post};
use axum::Router;
use std::sync::Arc;

mod delete;
mod membership;
mod policy;
mod read;
mod refresh;
mod update;

pub use delete::delete_account;
use membership::set_pool_membership;
#[cfg(test)]
use membership::PoolMembershipInput;
#[cfg(test)]
use policy::account_dispatch_permission_changed;
use read::{export_accounts, list_accounts, reveal_account_identity};
use refresh::{refresh_account, refresh_all_account_quotas};
use update::update_account;
#[cfg(test)]
use update::AccountPatch;

#[cfg(test)]
use crate::state::{AccountCredential, ServerAccountRecord};
#[cfg(test)]
use axum::extract::{Path, State};
#[cfg(test)]
use axum::Json;

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/accounts", get(list_accounts))
        .route("/accounts/export", post(export_accounts))
        .route(
            "/accounts/{id}/identity/reveal",
            post(reveal_account_identity),
        )
        .route(
            "/accounts/{id}",
            patch(update_account).delete(delete_account),
        )
        .route("/accounts/{id}/refresh", post(refresh_account))
        .route("/pool/members", post(set_pool_membership))
        .route("/pool/quota/refresh", post(refresh_all_account_quotas))
}
#[cfg(test)]
mod tests;
