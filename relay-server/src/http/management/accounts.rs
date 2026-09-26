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
    MAX_PURCHASE_COST_MICRO_USD,
};
use zenith_relay_core::error_codes;
use zenith_relay_core::protocol::{
    account_candidate_enabled, account_operational_state, AccountOperationalInput, AccountSummary,
    RevealedAccountIdentity, RuntimeStateSnapshot,
};
use zenith_relay_core::{CandidateKind, RuntimeCandidatePolicy};

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
                error_codes::ACCOUNT_SECRET_MISSING,
                "stored account credential is unavailable",
            )
        })?;
    let credential: AccountCredential = serde_json::from_str(&secret).map_err(|_| {
        ManagementError::internal(
            error_codes::ACCOUNT_SECRET_INVALID,
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
                    error_codes::ACCOUNT_SECRET_MISSING,
                    "stored account credential is unavailable",
                )
            })?;
        let credential: AccountCredential = serde_json::from_str(&secret).map_err(|_| {
            ManagementError::internal(
                error_codes::ACCOUNT_SECRET_INVALID,
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

fn account_dispatch_permission_changed(
    previous: &ServerAccountRecord,
    next: &ServerAccountRecord,
) -> bool {
    previous.enabled != next.enabled
        || previous.in_pool != next.in_pool
        || previous.draining != next.draining
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
            error_codes::POOL_MEMBERS_EMPTY,
            "at least one pool member is required",
        ));
    }
    if account_ids.len().saturating_add(source_ids.len()) > 2_048 {
        return Err(ManagementError::validation(
            error_codes::POOL_MEMBERS_TOO_MANY,
            "too many pool members were requested",
        ));
    }

    // Serialize validation, durable membership and runtime publication with
    // single-member edits. Final dispatch has no access to this host lock, so
    // changed members also need physical candidate fences before the commit.
    let _configuration = state.configuration_lock.lock().await;
    let build = state.lock_runtime_rebuild().await;
    let accounts = state.store.accounts().map_err(store_error)?;
    let sources = state.store.sources().map_err(store_error)?;
    let old_accounts = account_ids
        .iter()
        .map(|id| {
            accounts
                .iter()
                .find(|record| &record.id == id)
                .map(|record| (id.clone(), record.in_pool))
                .ok_or_else(|| {
                    ManagementError::not_found(error_codes::ACCOUNT_NOT_FOUND, "account not found")
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let old_sources = source_ids
        .iter()
        .map(|id| {
            sources
                .iter()
                .find(|record| &record.id == id)
                .map(|record| (id.clone(), record.in_pool))
                .ok_or_else(|| {
                    ManagementError::not_found(error_codes::SOURCE_NOT_FOUND, "source not found")
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    if input.in_pool {
        for source_id in &source_ids {
            let source = sources
                .iter()
                .find(|record| &record.id == source_id)
                .expect("source was validated above");
            if !source.supports_any_wire_api().map_err(|message| {
                ManagementError::validation(error_codes::SOURCE_PROTOCOL_INVALID, message)
            })? {
                return Err(ManagementError::new(
                    StatusCode::CONFLICT,
                    error_codes::SOURCE_POOL_PROTOCOL_UNSUPPORTED,
                    "source must expose at least one verified API route before joining the pool",
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
    let _dispatch_fences = state.runtime().map_err(runtime_error)?.map(|runtime| {
        let mut fences = old_accounts
            .iter()
            .filter(|(_, previous)| *previous != input.in_pool)
            .filter_map(|(id, _)| runtime.fence_candidate_dispatch(id))
            .collect::<Vec<_>>();
        for (id, _) in old_sources
            .iter()
            .filter(|(_, previous)| *previous != input.in_pool)
        {
            fences.extend(runtime.fence_source_dispatch(id));
        }
        fences
    });
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
            build
                .rollback_and_rebuild(&state, || {
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
        build
            .rebuild_or_rollback(&state, || {
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
    let configuration = state.configuration_lock.lock().await;
    let build = state.lock_runtime_rebuild().await;
    let credential = state.account_credential_lock.lock().await;
    let record = find_account(&state, &id)?;
    let secret = state
        .vault
        .load(&record.secret_ref)
        .map_err(vault_error)?
        .ok_or_else(|| {
            ManagementError::not_found(
                error_codes::ACCOUNT_SECRET_MISSING,
                "account secret missing",
            )
        })?;
    // Close the old login before either durable store or vault changes. Hold
    // the fence through replacement/rollback; only already-started attempts
    // may settle after deletion begins.
    let previous_runtime = state.runtime().map_err(runtime_error)?;
    let _dispatch_fence = previous_runtime
        .as_ref()
        .and_then(|runtime| runtime.fence_candidate_dispatch(&id));
    state.store.delete_account(&id).map_err(store_error)?;
    if let Err(error) = state.vault.delete(&record.secret_ref) {
        drop(credential);
        drop(configuration);
        build
            .rollback_and_rebuild(&state, || state.store.save_account(&record))
            .await
            .map_err(|restore| runtime_error(format!("{error}; {restore}")))?;
        return Err(vault_error(error));
    }
    state.token_authority.remove(&id);
    if let Some(runtime) = previous_runtime.as_ref() {
        runtime.remove_candidate(&id);
    }
    drop(credential);
    drop(configuration);
    if let Err(error) = build.rebuild(&state).await {
        build
            .rollback_and_rebuild(&state, || {
                state.vault.save(&record.secret_ref, &secret)?;
                state.store.save_account(&record)
            })
            .await
            .map_err(|restore| runtime_error(format!("{error}; {restore}")))?;
        return Err(runtime_error(error));
    }
    drop(build);
    state
        .store
        .remove_account_from_wake_tasks(&id, now_ms())
        .map_err(store_error)?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixtures::pooled_source;
    use crate::{
        config::Config,
        store::{Store, Vault},
    };
    use std::collections::BTreeMap;
    use tempfile::TempDir;
    use zenith_relay_core::{
        accounts::{AccountAuthState, AccountHealthState},
        quota::{QuotaSnapshot, Subscription},
    };

    fn test_state(root: &TempDir) -> Arc<AppState> {
        let config = Config::for_test(root.path().to_path_buf(), "127.0.0.1:0".parse().unwrap());
        let store = Arc::new(Store::open(root.path().join("relay.sqlite")).unwrap());
        let vault = Arc::new(Vault::open(&root.path().join("vault"), config.vault_key).unwrap());
        AppState::new(config, store, vault).unwrap()
    }

    fn test_account(id: &str) -> ServerAccountRecord {
        ServerAccountRecord {
            id: id.into(),
            label: "Synthetic account".into(),
            identity_hint: "synthetic-hint".into(),
            enabled: true,
            in_pool: true,
            draining: false,
            source_id: "openai_codex".into(),
            secret_ref: format!("account:{id}"),
            provider_family: Some("openai".into()),
            auth_state: AccountAuthState::Active,
            health: AccountHealthState::Healthy,
            models: vec!["gpt-test".into()],
            discovered_models: None,
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
            priority: 0,
            weight: 1,
            subscription: Subscription::default(),
            quota: QuotaSnapshot::default(),
            purchase_cost_micro_usd: None,
            cooldowns: BTreeMap::new(),
            consecutive_failures: 0,
            created_at_ms: 1,
            last_used_at_ms: None,
            last_error_code: None,
            proxy_id: None,
            bypass_common_proxy: false,
        }
    }

    fn test_credential() -> AccountCredential {
        AccountCredential {
            access_token: "synthetic-access".into(),
            refresh_token: None,
            id_token: None,
            expires_at_ms: None,
            issued_at_ms: 1,
            generation: 0,
            chatgpt_account_id: "synthetic-provider".into(),
            responses_url: "http://127.0.0.1:9/v1/responses".into(),
            proxy_url: None,
            agent_private_key: None,
            agent_runtime_id: None,
            agent_task_id: None,
        }
    }

    #[tokio::test]
    async fn failed_vault_delete_restores_account_and_retires_old_runtime() {
        let root = TempDir::new().unwrap();
        let state = test_state(&root);
        let record = test_account("delete-rollback");
        let secret = serde_json::to_string(&test_credential()).unwrap();
        state.store.save_account(&record).unwrap();
        state.vault.save(&record.secret_ref, &secret).unwrap();
        state.rebuild_runtime().await.unwrap();
        let previous = state.runtime().unwrap().unwrap();
        assert!(previous
            .candidate_runtime_order()
            .iter()
            .any(|candidate| candidate.candidate_id == record.id && candidate.available));

        // Force the vault's atomic replace to fail before it writes anything.
        // This is synthetic filesystem state, never a real credential.
        let backup = root.path().join("vault/secrets.enc.bak");
        if backup.is_file() {
            std::fs::remove_file(&backup).unwrap();
        }
        std::fs::create_dir(&backup).unwrap();
        assert!(
            delete_account(State(state.clone()), Path(record.id.clone()))
                .await
                .is_err()
        );
        assert_eq!(
            state.store.account(&record.id).unwrap().unwrap().secret_ref,
            record.secret_ref
        );
        assert_eq!(
            state.vault.load(&record.secret_ref).unwrap().as_deref(),
            Some(secret.as_str())
        );
        let restored = state.runtime().unwrap().unwrap();
        assert!(!Arc::ptr_eq(&previous, &restored));
        assert!(previous
            .candidate_runtime_order()
            .iter()
            .all(|candidate| candidate.candidate_id != record.id || !candidate.available));
        assert!(restored
            .candidate_runtime_order()
            .iter()
            .any(|candidate| candidate.candidate_id == record.id && candidate.available));
        std::fs::remove_dir(&backup).unwrap();
        state.shutdown_runtime().await.unwrap();
    }

    #[tokio::test]
    async fn account_disable_updates_the_live_runtime_without_a_replacement() {
        let root = TempDir::new().unwrap();
        let state = test_state(&root);
        let record = test_account("synthetic-account");
        let credential = test_credential();
        let mut weighting = record.clone();
        weighting.weight = 3;
        weighting.priority = 2;
        assert!(!account_dispatch_permission_changed(&record, &weighting));
        let mut removed = record.clone();
        removed.in_pool = false;
        assert!(account_dispatch_permission_changed(&record, &removed));
        state.store.save_account(&record).unwrap();
        state
            .vault
            .save(
                &record.secret_ref,
                &serde_json::to_string(&credential).unwrap(),
            )
            .unwrap();
        state.rebuild_runtime().await.unwrap();
        let runtime = state.runtime().unwrap().unwrap();
        assert!(runtime
            .candidate_runtime_order()
            .iter()
            .any(|candidate| candidate.available));

        let Json(summary) = update_account(
            State(state.clone()),
            Path(record.id.clone()),
            Json(AccountPatch {
                enabled: Some(false),
                ..Default::default()
            }),
        )
        .await
        .unwrap();
        assert!(!summary.enabled);
        assert!(!state.store.account(&record.id).unwrap().unwrap().enabled);
        assert!(Arc::ptr_eq(&runtime, &state.runtime().unwrap().unwrap()));
        assert!(runtime
            .candidate_runtime_order()
            .iter()
            .all(|candidate| !candidate.available));
        state.shutdown_runtime().await.unwrap();
    }

    #[tokio::test]
    async fn mixed_membership_batch_updates_scopes_without_replacing_the_runtime() {
        let root = TempDir::new().unwrap();
        let state = test_state(&root);
        let account = test_account("batch-account");
        let source = pooled_source("batch-source", "gpt-test");
        state.store.save_account(&account).unwrap();
        state
            .vault
            .save(
                &account.secret_ref,
                &serde_json::to_string(&test_credential()).unwrap(),
            )
            .unwrap();
        state.store.save_source(&source).unwrap();
        state
            .vault
            .save(&source.secret_ref, "synthetic-source-key")
            .unwrap();
        state.rebuild_runtime().await.unwrap();
        let runtime = state.runtime().unwrap().unwrap();
        let next = || {
            runtime
                .candidate_runtime_order_for_key(crate::state::SYSTEM_GATEWAY_KEY_ID)
                .into_iter()
                .any(|candidate| candidate.next_for_new_request)
        };
        assert!(next());

        // Validation must happen before any candidate is fenced or any durable
        // member is changed, even when another id in the same batch exists.
        let missing = set_pool_membership(
            State(state.clone()),
            Json(PoolMembershipInput {
                account_ids: vec![account.id.clone()],
                source_ids: vec!["missing-source".into()],
                in_pool: false,
            }),
        )
        .await;
        assert!(missing.is_err());
        assert!(next());
        assert!(state.store.account(&account.id).unwrap().unwrap().in_pool);

        let membership = |in_pool| PoolMembershipInput {
            account_ids: vec![account.id.clone()],
            source_ids: vec![source.id.clone()],
            in_pool,
        };
        let Json(removed) = set_pool_membership(State(state.clone()), Json(membership(false)))
            .await
            .unwrap();
        assert!(removed.accounts.iter().all(|account| !account.in_pool));
        assert!(removed.sources.iter().all(|source| !source.in_pool));
        assert!(!next());
        assert!(Arc::ptr_eq(&runtime, &state.runtime().unwrap().unwrap()));

        let Json(joined) = set_pool_membership(State(state.clone()), Json(membership(true)))
            .await
            .unwrap();
        assert!(joined.accounts.iter().all(|account| account.in_pool));
        assert!(joined.sources.iter().all(|source| source.in_pool));
        assert!(next());
        assert!(Arc::ptr_eq(&runtime, &state.runtime().unwrap().unwrap()));
        state.shutdown_runtime().await.unwrap();
    }
}
