use super::{runtime_error, store_error, vault_error, ManagementError};
use crate::state::{
    generate_pool_key, now_ms, AppState, GatewayKeyRecord, PROFILE_KEY_ROTATION_PREFIX,
    SYSTEM_GATEWAY_KEY_ID,
};
use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;
use std::sync::Arc;
use zenith_relay_core::error_codes;
use zenith_relay_core::protocol::{ProfileKeyRotation, PROFILE_KEY_ROTATION_SCHEMA_VERSION};

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/profile/credential", get(profile_credential))
        .route(
            "/profile/credential/rotations",
            post(prepare_profile_key_rotation),
        )
        .route(
            "/profile/credential/rotations/{id}",
            post(commit_profile_key_rotation).delete(abort_profile_key_rotation),
        )
}

mod rotation;
use rotation::{
    abort_profile_key_rotation, commit_profile_key_rotation, prepare_profile_key_rotation,
    profile_credential,
};
#[cfg(test)]
mod tests;
