use super::super::{find_account, store_error, validation_error, vault_error, ManagementError};
use crate::state::{now_ms, AccountCredential, AppState};
use axum::extract::{Path, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;
use std::collections::BTreeSet;
use std::sync::Arc;
use zenith_relay_core::accounts::{
    build_account_export, AccountExportCredential, AccountExportDocument, AccountExportRequest,
};
use zenith_relay_core::error_codes;
use zenith_relay_core::protocol::{AccountSummary, RevealedAccountIdentity};

pub(super) async fn list_accounts(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<AccountSummary>>, ManagementError> {
    Ok(Json(state.snapshot().map_err(store_error)?.accounts))
}

pub(super) async fn reveal_account_identity(
    Path(account_id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> Result<Response, ManagementError> {
    let account_record = find_account(&state, &account_id)?;
    let credential = load_account_credential(&state, &account_record.secret_ref)?;
    Ok(no_store_json(RevealedAccountIdentity {
        account_id,
        identity: credential.chatgpt_account_id,
    }))
}

pub(super) async fn export_accounts(
    State(state): State<Arc<AppState>>,
    Json(input): Json<AccountExportRequest>,
) -> Result<Response, ManagementError> {
    input
        .validate()
        .map_err(|error| validation_error(error.to_string()))?;
    let mut accounts = Vec::with_capacity(input.account_ids.len());
    for account_id in &input.account_ids {
        let account_record = find_account(&state, account_id)?;
        let credential = load_account_credential(&state, &account_record.secret_ref)?;
        accounts.push(AccountExportCredential {
            label: account_record.label,
            email: None,
            phone: None,
            password: None,
            totp_secret: None,
            access_token: credential.access_token,
            refresh_token: credential.refresh_token,
            id_token: credential.id_token,
            account_id: Some(credential.chatgpt_account_id),
            user_id: None,
            organization_id: None,
            plan_type: account_record.subscription.plan_type.clone(),
            expires_at_ms: credential.expires_at_ms,
            issued_at_ms: credential.issued_at_ms,
            subscription_active_until_ms: account_record.subscription.active_until_ms,
            created_at_ms: credential.issued_at_ms,
            priority: account_record.priority,
            enabled: account_record.enabled,
            tags: BTreeSet::new(),
        });
    }
    let document: AccountExportDocument = build_account_export(
        input.format,
        &accounts,
        now_ms(),
        input.description.as_deref(),
    )
    .map_err(|_| {
        ManagementError::internal(
            error_codes::ACCOUNT_EXPORT_FAILED,
            "account export could not be created",
        )
    })?;
    Ok(no_store_json(document))
}

fn load_account_credential(
    state: &AppState,
    secret_ref: &str,
) -> Result<AccountCredential, ManagementError> {
    let secret = state
        .vault
        .load(secret_ref)
        .map_err(vault_error)?
        .ok_or_else(|| {
            ManagementError::internal(
                error_codes::ACCOUNT_SECRET_MISSING,
                "stored account credential is unavailable",
            )
        })?;
    serde_json::from_str(&secret).map_err(|_| {
        ManagementError::internal(
            error_codes::ACCOUNT_SECRET_INVALID,
            "stored account credential is invalid",
        )
    })
}

fn no_store_json<T: Serialize>(response_body: T) -> Response {
    let mut response = Json(response_body).into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store, max-age=0"),
    );
    response
        .headers_mut()
        .insert(header::PRAGMA, header::HeaderValue::from_static("no-cache"));
    response
}
