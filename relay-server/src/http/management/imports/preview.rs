use super::super::{
    clean_label, default_weight, normalized_values, store_error, valid_weight, validate_secret,
    validation_error, vault_error, ManagementError,
};
use super::confirm::cleanup_expired_imports;
use super::probe::{
    authenticate_import_account, clean_identifier, contains_sensitive, imported_account_id_hints,
    nonempty, redact_import_label, safe_plan_type, validate_account_responses_url,
};

use crate::state::{identity_hint, now_ms, AccountCredential, AppState};
use crate::store::PendingImport;
use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use zenith_relay_core::accounts::AccountAuthState;
use zenith_relay_core::error_codes;

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountImportInput {
    pub(super) label: String,
    #[serde(default)]
    pub(super) access_token: String,
    #[serde(default)]
    pub(super) agent_private_key: Option<String>,
    #[serde(default)]
    pub(super) agent_runtime_id: Option<String>,
    #[serde(default)]
    pub(super) agent_task_id: Option<String>,
    pub(super) refresh_token: Option<String>,
    pub(super) id_token: Option<String>,
    pub(super) expires_at_ms: Option<u64>,
    pub(super) plan_type: Option<String>,
    pub(super) subscription_active_until_ms: Option<u64>,
    #[serde(default)]
    pub(super) chatgpt_account_id: Option<String>,
    pub(super) responses_url: Option<String>,
    #[serde(default)]
    pub(super) models: Vec<String>,
    #[serde(default)]
    pub(super) allowed_models: Vec<String>,
    #[serde(default)]
    pub(super) excluded_models: Vec<String>,
    #[serde(default)]
    pub(super) priority: i32,
    #[serde(default = "default_weight")]
    pub(super) weight: u32,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountImportPreview {
    pub(super) session_id: String,
    pub(super) account_id: String,
    pub(super) duplicate_account_id: Option<String>,
    pub(super) label: String,
    pub(super) identity_hint: String,
    pub(super) models: Vec<String>,
    pub(super) auth_state: AccountAuthState,
    pub(super) expires_at_ms: Option<u64>,
    pub(super) plan_type: Option<String>,
    pub(super) subscription_active_until_ms: Option<u64>,
    pub(super) allowed_models: Vec<String>,
    pub(super) excluded_models: Vec<String>,
    pub(super) priority: i32,
    pub(super) weight: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) batch_session_id: Option<String>,
}

pub async fn preview_account_import(
    State(state): State<Arc<AppState>>,
    Json(input): Json<AccountImportInput>,
) -> Result<(StatusCode, Json<AccountImportPreview>), ManagementError> {
    cleanup_expired_imports(&state)?;
    let preview = prepare_account_import(&state, input, None).await?;
    Ok((StatusCode::CREATED, Json(preview)))
}

pub(super) async fn prepare_account_import(
    state: &AppState,
    input: AccountImportInput,
    batch_session_id: Option<&str>,
) -> Result<AccountImportPreview, ManagementError> {
    let has_agent_identity = input.agent_private_key.is_some()
        || input.agent_runtime_id.is_some()
        || input.agent_task_id.is_some();
    if has_agent_identity {
        let private_key = input.agent_private_key.clone().unwrap_or_default();
        let runtime_id = input.agent_runtime_id.clone().unwrap_or_default();
        match input.agent_task_id.clone() {
            Some(task_id) => zenith_relay_core::providers::chatgpt::AgentIdentityCredential::new(
                private_key,
                runtime_id,
                task_id,
            ),
            None => zenith_relay_core::providers::chatgpt::AgentIdentityCredential::unregistered(
                private_key,
                runtime_id,
            ),
        }
        .map_err(|_| {
            ManagementError::validation(
                error_codes::AGENT_IDENTITY_INVALID,
                "Agent Identity credential is invalid",
            )
        })?;
        if !input.access_token.is_empty() {
            validate_secret(&input.access_token, "access token")?;
        }
    } else {
        validate_secret(&input.access_token, "access token")?;
    }
    if let Some(refresh_token) = input.refresh_token.as_deref() {
        validate_secret(refresh_token, "refresh token")?;
    }
    if let Some(id_token) = input.id_token.as_deref() {
        validate_secret(id_token, "ID token")?;
    }
    let account_id_hints = imported_account_id_hints(
        input.chatgpt_account_id.as_deref(),
        input.id_token.as_deref(),
        &input.access_token,
    )?;
    let chatgpt_account_id = if has_agent_identity && input.access_token.is_empty() {
        account_id_hints.first().cloned().ok_or_else(|| {
            validation_error("ChatGPT account id is required for Agent Identity imports")
        })?
    } else {
        authenticate_import_account(state, &input.access_token, &account_id_hints).await?
    };
    let label = redact_import_label(
        clean_label(&input.label, "account label")?,
        &[
            Some(input.access_token.as_str()),
            input.refresh_token.as_deref(),
            input.id_token.as_deref(),
            input.agent_private_key.as_deref(),
            Some(chatgpt_account_id.as_str()),
        ],
    );
    let plan_type = input
        .plan_type
        .as_deref()
        .filter(|plan_type_text| {
            !contains_sensitive(
                plan_type_text,
                &[
                    Some(input.access_token.as_str()),
                    input.refresh_token.as_deref(),
                    input.id_token.as_deref(),
                    input.agent_private_key.as_deref(),
                    Some(chatgpt_account_id.as_str()),
                ],
            )
        })
        .map(str::to_string)
        .and_then(safe_plan_type);
    let chatgpt_account_id = clean_identifier(&chatgpt_account_id, "account id")?;
    let responses_url = validate_account_responses_url(input.responses_url.as_deref())?;
    let identity_hint = identity_hint(&chatgpt_account_id);
    let duplicate_account = state
        .store
        .accounts()
        .map_err(store_error)?
        .into_iter()
        .find(|account_record| account_record.identity_hint == identity_hint);
    let duplicate_account_id = duplicate_account
        .as_ref()
        .map(|account_record| account_record.id.clone());
    let account_id = duplicate_account_id
        .clone()
        .unwrap_or_else(|| format!("account_{}", uuid::Uuid::new_v4().simple()));
    let session_id = format!("import_{}", uuid::Uuid::new_v4().simple());
    let secret_ref = format!("account:{account_id}:{}", uuid::Uuid::new_v4().simple());
    let existing_credential = match duplicate_account.as_ref() {
        Some(account_record) => match state
            .vault
            .load(&account_record.secret_ref)
            .map_err(vault_error)?
        {
            Some(credential_json) => Some(
                serde_json::from_str::<AccountCredential>(&credential_json).map_err(|_| {
                    ManagementError::internal(
                        error_codes::ACCOUNT_SECRET_INVALID,
                        "account secret is invalid",
                    )
                })?,
            ),
            None => None,
        },
        None => None,
    };
    let proxy_url = existing_credential
        .as_ref()
        .and_then(|credential| credential.proxy_url.clone());
    let credential = AccountCredential {
        access_token: input.access_token,
        refresh_token: nonempty(input.refresh_token).or_else(|| {
            existing_credential
                .as_ref()
                .and_then(|credential| credential.refresh_token.clone())
        }),
        id_token: nonempty(input.id_token),
        expires_at_ms: input.expires_at_ms,
        issued_at_ms: now_ms(),
        generation: 0,
        chatgpt_account_id,
        responses_url,
        proxy_url,
        agent_private_key: input.agent_private_key,
        agent_runtime_id: input.agent_runtime_id,
        agent_task_id: input.agent_task_id,
    };
    if credential.is_agent_identity() {
        credential.agent_identity().map_err(validation_error)?;
    }
    if credential.has_oauth() {
        credential.tokens().map_err(validation_error)?;
    }
    if !credential.is_agent_identity() && !credential.has_oauth() {
        return Err(validation_error(
            "account credential has no authorization method",
        ));
    }
    let auth_state = if credential.is_agent_identity() || credential.refresh_token.is_some() {
        AccountAuthState::Active
    } else {
        AccountAuthState::DegradedAccessOnly
    };
    let preview = AccountImportPreview {
        session_id: session_id.clone(),
        account_id,
        duplicate_account_id,
        label,
        identity_hint,
        models: normalized_values(input.models),
        auth_state,
        expires_at_ms: credential.expires_at_ms,
        plan_type,
        subscription_active_until_ms: input.subscription_active_until_ms,
        allowed_models: normalized_values(input.allowed_models),
        excluded_models: normalized_values(input.excluded_models),
        priority: input.priority,
        weight: valid_weight(input.weight)?,
        batch_session_id: batch_session_id.map(str::to_string),
    };
    state
        .vault
        .save(
            &secret_ref,
            &serde_json::to_string(&credential).map_err(|_| {
                ManagementError::internal(
                    error_codes::IMPORT_SERIALIZE,
                    "import could not be prepared",
                )
            })?,
        )
        .map_err(vault_error)?;
    let pending = PendingImport {
        id: session_id,
        preview_json: serde_json::to_string(&preview).map_err(|_| {
            ManagementError::internal(error_codes::PREVIEW_SERIALIZE, "preview could not be saved")
        })?,
        secret_ref,
        created_at_ms: now_ms(),
    };
    if let Err(error) = state.store.save_pending_import(&pending) {
        let _ = state.vault.delete(&pending.secret_ref);
        return Err(store_error(error));
    }
    Ok(preview)
}
