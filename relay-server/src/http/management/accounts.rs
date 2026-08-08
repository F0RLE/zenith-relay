use super::{
    account_summary, clean_label, find_account, normalized_values, runtime_error, store_error,
    valid_weight, validation_error, vault_error, ManagementError,
};
use crate::app::account_proxy_config;
use crate::jobs;
use crate::state::{now_ms, AccountCredential, AppState, ServerAccountRecord};
use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::sync::Arc;
use zenith_relay_core::accounts::{
    build_account_export, AccountExportCredential, AccountExportDocument, AccountExportRequest,
};
use zenith_relay_core::protocol::{
    account_candidate_enabled, account_operational_state, AccountOperationalInput, AccountSummary,
    RevealedAccountIdentity, RuntimeStateSnapshot,
};
use zenith_relay_core::quota::MAX_PURCHASE_COST_MICRO_USD;
use zenith_relay_core::{CandidateKind, RuntimeCandidatePolicy, WireApi};

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

pub async fn list_accounts(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<AccountSummary>>, ManagementError> {
    Ok(Json(state.snapshot().map_err(store_error)?.accounts))
}

pub async fn reveal_account_identity(
    Path(account_id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> Result<Response, ManagementError> {
    let record = find_account(&state, &account_id)?;
    let secret = state
        .vault
        .load(&record.secret_ref)
        .map_err(vault_error)?
        .ok_or_else(|| {
            ManagementError::internal(
                "account_secret_missing",
                "stored account credential is unavailable",
            )
        })?;
    let credential: AccountCredential = serde_json::from_str(&secret).map_err(|_| {
        ManagementError::internal(
            "account_secret_invalid",
            "stored account credential is invalid",
        )
    })?;
    Ok(no_store_json(RevealedAccountIdentity {
        account_id,
        identity: credential.chatgpt_account_id,
    }))
}

pub async fn export_accounts(
    State(state): State<Arc<AppState>>,
    Json(input): Json<AccountExportRequest>,
) -> Result<Response, ManagementError> {
    input
        .validate()
        .map_err(|error| validation_error(error.to_string()))?;
    let mut accounts = Vec::with_capacity(input.account_ids.len());
    for account_id in &input.account_ids {
        let record = find_account(&state, account_id)?;
        let secret = state
            .vault
            .load(&record.secret_ref)
            .map_err(vault_error)?
            .ok_or_else(|| {
                ManagementError::internal(
                    "account_secret_missing",
                    "stored account credential is unavailable",
                )
            })?;
        let credential: AccountCredential = serde_json::from_str(&secret).map_err(|_| {
            ManagementError::internal(
                "account_secret_invalid",
                "stored account credential is invalid",
            )
        })?;
        accounts.push(AccountExportCredential {
            label: record.label,
            email: None,
            access_token: credential.access_token,
            refresh_token: credential.refresh_token,
            id_token: credential.id_token,
            account_id: Some(credential.chatgpt_account_id),
            user_id: None,
            organization_id: None,
            plan_type: record.subscription.plan_type.clone(),
            expires_at_ms: credential.expires_at_ms,
            issued_at_ms: credential.issued_at_ms,
            subscription_active_until_ms: record.subscription.active_until_ms,
            created_at_ms: credential.issued_at_ms,
            priority: record.priority,
            enabled: record.enabled,
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
            "account_export_failed",
            "account export could not be created",
        )
    })?;
    Ok(no_store_json(document))
}

fn no_store_json<T: Serialize>(value: T) -> Response {
    let mut response = Json(value).into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store, max-age=0"),
    );
    response
        .headers_mut()
        .insert(header::PRAGMA, header::HeaderValue::from_static("no-cache"));
    response
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountPatch {
    label: Option<String>,
    enabled: Option<bool>,
    in_pool: Option<bool>,
    draining: Option<bool>,
    allowed_models: Option<Vec<String>>,
    excluded_models: Option<Vec<String>>,
    priority: Option<i32>,
    weight: Option<u32>,
    purchase_cost_micro_usd: Option<u64>,
}

pub async fn update_account(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(input): Json<AccountPatch>,
) -> Result<Json<AccountSummary>, ManagementError> {
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
                "account_purchase_cost_invalid",
                "account purchase cost is too large",
            ));
        }
        record
            .economics
            .set_purchase_cost_micro_usd((value > 0).then_some(value));
    }
    let policy_changed = account_runtime_policy_changed(&old, &record);
    state.store.save_account(&record).map_err(store_error)?;
    let runtime_applied = if policy_changed || old.in_pool != record.in_pool {
        match apply_account_policy_if_running(&state, &record) {
            Ok(applied) => applied,
            Err(error) => {
                state
                    .rollback_and_rebuild_runtime(|| state.store.save_account(&old))
                    .await
                    .map_err(|restore| runtime_error(format!("{error}; {restore}")))?;
                return Err(runtime_error(error));
            }
        }
    } else {
        true
    };
    if !runtime_applied {
        state
            .rebuild_runtime_or_rollback(|| state.store.save_account(&old))
            .await
            .map_err(runtime_error)?;
    }
    Ok(Json(account_summary(&state, &record)?))
}

fn apply_account_policy_if_running(
    state: &AppState,
    account: &ServerAccountRecord,
) -> Result<bool, String> {
    apply_account_policies_if_running(state, std::slice::from_ref(account))
}

/// Applies account policies before widening or narrowing the internal key
/// scope. Account membership is part of an account candidate's operational
/// state, unlike API-source membership which is enforced solely by the key
/// scope. Updating the candidate first means a removed account cannot accept a
/// new request during the scope update, while an in-flight request keeps its
/// existing executor.
fn apply_account_policies_if_running(
    state: &AppState,
    accounts: &[ServerAccountRecord],
) -> Result<bool, String> {
    let Some(runtime) = state.runtime()? else {
        return Ok(!state.store.gateway_enabled()?);
    };
    let candidate_ids = runtime
        .candidate_runtime_order()
        .into_iter()
        .filter(|candidate| candidate.kind == CandidateKind::OAuthAccount)
        .map(|candidate| candidate.candidate_id)
        .collect::<BTreeSet<_>>();
    for account in accounts {
        let policy = account_runtime_policy(state, account)?;
        if !candidate_ids.contains(&account.id) {
            if policy.enabled {
                return Ok(false);
            }
            continue;
        }
        if !runtime.update_account_policy(&account.id, policy) {
            return Ok(false);
        }
    }
    state.refresh_internal_gateway_key_scopes(&runtime)
}

fn account_runtime_policy(
    state: &AppState,
    account: &ServerAccountRecord,
) -> Result<RuntimeCandidatePolicy, String> {
    let credential = state
        .vault
        .load(&account.secret_ref)?
        .and_then(|value| serde_json::from_str::<AccountCredential>(&value).ok());
    let secret_available = credential.is_some();
    let proxy_available = credential
        .as_ref()
        .is_some_and(|credential| account_proxy_config(state, account, credential).is_ok());
    let operational = account_operational_state(AccountOperationalInput {
        enabled: account.enabled,
        in_pool: account.in_pool,
        draining: account.draining,
        secret_available,
        proxy_available,
        auth_state: account.auth_state,
        health: account.health,
        subscription: &account.subscription,
        quota: &account.quota,
        last_error_code: account.last_error_code.as_deref(),
        now_ms: now_ms(),
        quota_stale_after_ms: zenith_relay_core::QUOTA_STALE_AFTER_MS,
    });
    Ok(RuntimeCandidatePolicy {
        enabled: account_candidate_enabled(account.enabled, operational.routing_block_reason),
        draining: account.draining,
        priority: account.priority,
        weight: account.weight,
        allowed_models: account.allowed_models.clone(),
        excluded_models: account.excluded_models.clone(),
    })
}

fn account_runtime_policy_changed(
    previous: &ServerAccountRecord,
    next: &ServerAccountRecord,
) -> bool {
    previous.enabled != next.enabled
        || previous.draining != next.draining
        || previous.priority != next.priority
        || previous.weight != next.weight
        || previous.allowed_models != next.allowed_models
        || previous.excluded_models != next.excluded_models
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PoolMembershipInput {
    #[serde(default)]
    account_ids: Vec<String>,
    #[serde(default)]
    source_ids: Vec<String>,
    in_pool: bool,
}

pub async fn set_pool_membership(
    State(state): State<Arc<AppState>>,
    Json(input): Json<PoolMembershipInput>,
) -> Result<Json<RuntimeStateSnapshot>, ManagementError> {
    let account_ids = input.account_ids.into_iter().collect::<BTreeSet<_>>();
    let source_ids = input.source_ids.into_iter().collect::<BTreeSet<_>>();
    if account_ids.is_empty() && source_ids.is_empty() {
        return Err(ManagementError::validation(
            "pool_members_empty",
            "at least one pool member is required",
        ));
    }
    if account_ids.len().saturating_add(source_ids.len()) > 2_048 {
        return Err(ManagementError::validation(
            "pool_members_too_many",
            "too many pool members were requested",
        ));
    }

    let accounts = state.store.accounts().map_err(store_error)?;
    let sources = state.store.sources().map_err(store_error)?;
    let old_accounts = account_ids
        .iter()
        .map(|id| {
            accounts
                .iter()
                .find(|record| &record.id == id)
                .map(|record| (id.clone(), record.in_pool))
                .ok_or_else(|| ManagementError::not_found("account_not_found", "account not found"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let old_sources = source_ids
        .iter()
        .map(|id| {
            sources
                .iter()
                .find(|record| &record.id == id)
                .map(|record| (id.clone(), record.in_pool))
                .ok_or_else(|| ManagementError::not_found("source_not_found", "source not found"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if input.in_pool {
        for source_id in &source_ids {
            let source = sources
                .iter()
                .find(|record| &record.id == source_id)
                .expect("source was validated above");
            if !source
                .supports_wire_api(WireApi::Responses)
                .map_err(|message| {
                    ManagementError::validation("source_protocol_invalid", message)
                })?
            {
                return Err(ManagementError::new(
                    StatusCode::CONFLICT,
                    "source_pool_protocol_unsupported",
                    "only Responses API sources can join the ChatGPT pool",
                    "pool",
                    false,
                ));
            }
        }
    }
    let next_accounts = account_ids
        .iter()
        .map(|id| (id.clone(), input.in_pool))
        .collect::<Vec<_>>();
    let next_sources = source_ids
        .iter()
        .map(|id| (id.clone(), input.in_pool))
        .collect::<Vec<_>>();
    state
        .store
        .replace_pool_membership(&next_sources, &next_accounts)
        .map_err(store_error)?;
    let changed_accounts = accounts
        .iter()
        .filter(|account| account_ids.contains(&account.id))
        .cloned()
        .map(|mut account| {
            account.in_pool = input.in_pool;
            account
        })
        .collect::<Vec<_>>();
    let runtime_applied = match apply_account_policies_if_running(&state, &changed_accounts) {
        Ok(applied) => applied,
        Err(error) => {
            state
                .rollback_and_rebuild_runtime(|| {
                    state
                        .store
                        .replace_pool_membership(&old_sources, &old_accounts)
                })
                .await
                .map_err(|restore| runtime_error(format!("{error}; {restore}")))?;
            return Err(runtime_error(error));
        }
    };
    if !runtime_applied {
        state
            .rebuild_runtime_or_rollback(|| {
                state
                    .store
                    .replace_pool_membership(&old_sources, &old_accounts)
            })
            .await
            .map_err(runtime_error)?;
    }
    state.snapshot().map(Json).map_err(store_error)
}

pub async fn refresh_account(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<AccountSummary>, ManagementError> {
    let record = find_account(&state, &id)?;
    let updated = jobs::refresh_account_now(&state, record)
        .await
        .map_err(|_| {
            ManagementError::new(
                StatusCode::BAD_GATEWAY,
                "account_refresh_failed",
                "account metadata could not be refreshed",
                "quota",
                true,
            )
        })?;
    account_summary(&state, &updated).map(Json)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountQuotaRefreshResult {
    refreshed: usize,
    failed: usize,
    snapshot: RuntimeStateSnapshot,
}

pub async fn refresh_all_account_quotas(
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

pub async fn delete_account(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ManagementError> {
    let _wake_guard = state.wake_lock.lock().await;
    let record = find_account(&state, &id)?;
    let secret = state
        .vault
        .load(&record.secret_ref)
        .map_err(vault_error)?
        .ok_or_else(|| {
            ManagementError::not_found("account_secret_missing", "account secret missing")
        })?;
    state.store.delete_account(&id).map_err(store_error)?;
    state
        .vault
        .delete(&record.secret_ref)
        .map_err(vault_error)?;
    state.token_authority.remove(&id);
    if let Some(runtime) = state.runtime().map_err(runtime_error)? {
        runtime.remove_candidate(&id);
    }
    state
        .rebuild_runtime_or_rollback(|| {
            state.vault.save(&record.secret_ref, &secret)?;
            state.store.save_account(&record)?;
            Ok(())
        })
        .await
        .map_err(runtime_error)?;
    state
        .store
        .remove_account_from_wake_tasks(&id, now_ms())
        .map_err(store_error)?;
    Ok(StatusCode::NO_CONTENT)
}
