use crate::{
    app::{account_proxy_config, prepare_server_account_authorization},
    state::{now_ms, AccountCredential, AppState, ServerAccountRecord},
};
use reqwest::header::{HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE, REFERER, USER_AGENT};
use serde_json::Value;
use std::{sync::Arc, time::Duration};
use tokio::{sync::watch, task::JoinHandle};
use zenith_relay_core::{
    accounts::{reduce_account_quota, AccountAuthState, AccountHealthState, AccountQuotaOutcome},
    providers::chatgpt::{
        is_agent_identity_task_invalid_failure, subscription_refresh_due, CodexModelsClient,
        CodexQuotaClient, ModelDiscoveryFailure, ModelDiscoveryFailureCode,
        CODEX_MODELS_CLIENT_VERSION,
    },
    quota::{QuotaRefreshFailure, QuotaRefreshResult, QuotaTransition},
};

const IDLE_QUOTA_REFRESH_SECONDS: u64 = 15 * 60;
const MODEL_REFRESH_INTERVAL_SECONDS: u64 = 8 * 60 * 60;
const RESET_CREDITS_URL: &str = "https://chatgpt.com/backend-api/wham/rate-limit-reset-credits";
const RESET_CREDITS_CONSUME_URL: &str =
    "https://chatgpt.com/backend-api/wham/rate-limit-reset-credits/consume";
const MAX_RESET_RESPONSE_BYTES: usize = 256 * 1024;

pub fn start(state: Arc<AppState>, mut shutdown: watch::Receiver<bool>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut next_model_refresh = tokio::time::Instant::now();
        loop {
            let refresh_models = tokio::time::Instant::now() >= next_model_refresh;
            tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        break;
                    }
                }
                _ = async {
                    let _ = run(&state, refresh_models).await;
                    tokio::time::sleep(Duration::from_secs(IDLE_QUOTA_REFRESH_SECONDS)).await;
                } => {
                    if refresh_models {
                        next_model_refresh = tokio::time::Instant::now()
                            + Duration::from_secs(MODEL_REFRESH_INTERVAL_SECONDS);
                    }
                }
            }
        }
    })
}

async fn run(state: &Arc<AppState>, refresh_models: bool) -> Result<(), String> {
    super::refresh_automatic_accounts_now(state, refresh_models)
        .await
        .map(|_| ())
}

/// Redeem one ChatGPT reset credit after a weekly window reaches zero.  This
/// is intentionally server-side and independent from the desktop credential
/// store; the per-account lock and persisted fingerprint make retries
/// idempotent across concurrent refresh workers.
pub(crate) async fn try_auto_reset_weekly(
    state: &Arc<AppState>,
    account: &ServerAccountRecord,
    transitions: &[QuotaTransition],
) -> Result<bool, String> {
    if account.quota.reset_credits_available.unwrap_or(0) == 0 {
        return Ok(false);
    }
    let selector_account = super::wake_automation::core_account(account)?;
    let weekly = state.store.wake_tasks()?.into_iter().any(|task| {
        task.enabled
            && task.trigger == zenith_relay_core::automations::WakeTrigger::Weekly
            && task.account_selector.matches(&selector_account)
    });
    if !weekly {
        return Ok(false);
    }
    let transition = transitions.iter().find(|transition| {
        transition.window_kind == zenith_relay_core::quota::QuotaWindowKind::Secondary
    });
    let Some(transition) = transition else {
        return Ok(false);
    };
    if state
        .store
        .weekly_reset_was_applied(&account.id, &transition.fingerprint)?
    {
        return Ok(false);
    }
    let lock = state.quota_reset_lock(&account.id);
    let _guard = lock.lock().await;
    if state
        .store
        .weekly_reset_was_applied(&account.id, &transition.fingerprint)?
    {
        return Ok(false);
    }
    let secret = state
        .vault
        .load(&account.secret_ref)?
        .ok_or_else(|| "reset_credits_secret_missing".to_string())?;
    let credential: AccountCredential =
        serde_json::from_str(&secret).map_err(|_| "reset_credits_secret_invalid".to_string())?;
    let proxy = account_proxy_config(state, account, &credential)?;
    let client = match proxy.as_ref() {
        Some(proxy) => proxy
            .apply(reqwest::Client::builder())
            .build()
            .map_err(|_| "reset_credits_client_init".to_string())?,
        None => reqwest::Client::builder()
            .build()
            .map_err(|_| "reset_credits_client_init".to_string())?,
    };
    let (credential, authorization) =
        prepare_server_account_authorization(state, account, credential, None).await?;
    let headers = |request: reqwest::RequestBuilder| {
        request
            .header(AUTHORIZATION, authorization.clone())
            .header("ChatGPT-Account-Id", &credential.chatgpt_account_id)
            .header(ACCEPT, "application/json")
            .header(CONTENT_TYPE, "application/json")
            .header(REFERER, "https://chatgpt.com/")
            .header(USER_AGENT, "Zenith Relay Server")
    };
    let snapshot = headers(client.get(RESET_CREDITS_URL))
        .send()
        .await
        .map_err(|_| "reset_credits_fetch_failed".to_string())?;
    if !snapshot.status().is_success() {
        return Ok(false);
    }
    let body = snapshot
        .bytes()
        .await
        .map_err(|_| "reset_credits_fetch_failed".to_string())?;
    if body.len() > MAX_RESET_RESPONSE_BYTES {
        return Err("reset_credits_response_too_large".to_string());
    }
    let available = serde_json::from_slice::<Value>(&body)
        .ok()
        .and_then(|value| find_available_reset_credits(&value))
        .unwrap_or(0);
    if available == 0 {
        return Ok(false);
    }
    let redeem_id = uuid::Uuid::new_v4().to_string();
    let response = headers(
        client
            .post(RESET_CREDITS_CONSUME_URL)
            .json(&serde_json::json!({"redeem_request_id": redeem_id})),
    )
    .send()
    .await
    .map_err(|_| "reset_credits_consume_failed".to_string())?;
    if !response.status().is_success() {
        return Ok(false);
    }
    state
        .store
        .mark_weekly_reset_applied(&account.id, &transition.fingerprint)?;
    Ok(true)
}

fn find_available_reset_credits(value: &Value) -> Option<u32> {
    match value {
        Value::Object(object) => {
            for key in ["available_count", "availableCount", "count"] {
                if let Some(number) = object.get(key).and_then(Value::as_u64) {
                    return u32::try_from(number).ok();
                }
            }
            object.values().find_map(find_available_reset_credits)
        }
        Value::Array(values) => values.iter().find_map(find_available_reset_credits),
        _ => None,
    }
}

pub async fn refresh_account_metadata(
    state: &Arc<AppState>,
    account: ServerAccountRecord,
    force_subscription_refresh: bool,
    refresh_models: bool,
) -> Result<(ServerAccountRecord, Vec<QuotaTransition>), String> {
    if !refresh_models {
        return refresh_one(state, account, force_subscription_refresh).await;
    }
    let previous_account = account.clone();
    let (mut account, transitions) =
        match refresh_one(state, account, force_subscription_refresh).await {
            Ok(result) => result,
            Err(error) => {
                // Model discovery has its own eight-hour lifecycle. Preserve its
                // result even when the quota endpoint is temporarily unavailable.
                let mut model_account = previous_account;
                refresh_models_best_effort(state, &mut model_account).await;
                state.store.save_account(&model_account)?;
                return Err(error);
            }
        };
    refresh_models_best_effort(state, &mut account).await;
    state.store.save_account(&account)?;
    Ok((account, transitions))
}

async fn refresh_models_best_effort(state: &Arc<AppState>, account: &mut ServerAccountRecord) {
    let mut model_result = discover_account_models(state, account).await;
    let reauth_state = if model_discovery_was_unauthorized(&model_result) {
        match state
            .recover_account_tokens_after_unauthorized(&account.id)
            .await
        {
            Ok(_) => {
                model_result = discover_account_models(state, account).await;
                None
            }
            Err(_) => state
                .token_authority
                .auth_state(&account.id)
                .await
                .filter(|auth_state| matches!(auth_state, AccountAuthState::RequiresReauth(_))),
        }
    } else {
        None
    };
    apply_discovered_models(account, model_result);
    if let Some(auth_state) = reauth_state {
        account.auth_state = auth_state;
    }
}

pub async fn refresh_one(
    state: &Arc<AppState>,
    mut account: ServerAccountRecord,
    force_subscription_refresh: bool,
) -> Result<(ServerAccountRecord, Vec<QuotaTransition>), String> {
    let result = refresh_data(state, &account, force_subscription_refresh).await;
    let access_only_rejected = result.as_ref().err().is_some_and(|failure| {
        failure.http_status() == Some(401) || failure.code == "quota_token_prepare"
    });
    let update = reduce_account_quota(
        &account.quota,
        &account.subscription,
        account.health,
        account.last_error_code.as_deref(),
        result,
        now_ms(),
    )
    .map_err(|error| error.to_string())?;
    let transitions = match &update.outcome {
        AccountQuotaOutcome::Updated { transitions } => transitions.clone(),
        AccountQuotaOutcome::Failed { .. } => Vec::new(),
    };
    account.quota = update.quota;
    account.subscription = update.subscription;
    account.health = update.health;
    account.last_error_code = update.last_error_code;
    if let Some(auth_state) = state.token_authority.auth_state(&account.id).await {
        account.auth_state = auth_state;
    }
    if access_only_rejected && account.auth_state == AccountAuthState::DegradedAccessOnly {
        account.auth_state = AccountAuthState::RequiresReauth(
            zenith_relay_core::accounts::ReauthReason::AccessTokenExpired,
        );
    }
    state.store.save_account(&account)?;
    Ok((account, transitions))
}

async fn refresh_data(
    state: &Arc<AppState>,
    account: &ServerAccountRecord,
    force_subscription_refresh: bool,
) -> Result<QuotaRefreshResult, QuotaRefreshFailure> {
    let secret = state
        .vault
        .load(&account.secret_ref)
        .map_err(|_| QuotaRefreshFailure::new("quota_secret_load", true))?
        .ok_or_else(|| QuotaRefreshFailure::new("quota_secret_missing", false))?;
    let credential: AccountCredential = serde_json::from_str(&secret)
        .map_err(|_| QuotaRefreshFailure::new("quota_secret_invalid", false))?;
    let (mut credential, mut authorization) =
        prepare_server_account_authorization(state, account, credential, None)
            .await
            .map_err(|_| QuotaRefreshFailure::new("quota_authorization_prepare", true))?;
    let proxy = account_proxy_config(state, account, &credential)
        .map_err(|_| QuotaRefreshFailure::new("quota_proxy_unavailable", false))?;
    let request_timeout_seconds = state
        .store
        .quota_request_timeout_seconds()
        .map_err(|_| QuotaRefreshFailure::new("quota_policy_invalid", false))?;
    let observed_at_ms = now_ms();
    let refresh_subscription = force_subscription_refresh
        || subscription_refresh_due(
            account.subscription.active_until_ms,
            account.subscription.updated_at_ms,
            observed_at_ms,
        );
    let client = CodexQuotaClient::new_with_proxy_and_timeout(
        proxy.as_ref(),
        Duration::from_secs(request_timeout_seconds),
    )?;
    let first = client
        .refresh_data_with_subscription_authorized(
            authorization.clone(),
            &credential.chatgpt_account_id,
            observed_at_ms,
            &account.subscription,
            refresh_subscription,
        )
        .await;
    let failure = match first {
        Ok(data) => return Ok(data),
        Err(failure)
            if credential.is_agent_identity()
                && is_agent_identity_task_invalid_failure(&failure) =>
        {
            let expected_task_id = credential.agent_task_id.clone().unwrap_or_default();
            (credential, authorization) = prepare_server_account_authorization(
                state,
                account,
                credential,
                Some(&expected_task_id),
            )
            .await
            .map_err(|_| QuotaRefreshFailure::new("quota_authorization_prepare", true))?;
            return client
                .refresh_data_with_subscription_authorized(
                    authorization,
                    &credential.chatgpt_account_id,
                    now_ms(),
                    &account.subscription,
                    refresh_subscription,
                )
                .await;
        }
        Err(failure) if credential.is_agent_identity() => return Err(failure),
        Err(failure) if failure.http_status() == Some(401) => failure,
        Err(failure) => return Err(failure),
    };
    let Ok(tokens) = state
        .recover_account_tokens_after_unauthorized(&account.id)
        .await
    else {
        return Err(failure);
    };
    authorization = bearer_authorization(tokens.access_token())
        .map_err(|_| QuotaRefreshFailure::new("quota_token_prepare", true))?;
    client
        .refresh_data_with_subscription_authorized(
            authorization,
            &credential.chatgpt_account_id,
            observed_at_ms,
            &account.subscription,
            refresh_subscription,
        )
        .await
}

fn apply_discovered_models(
    account: &mut ServerAccountRecord,
    result: Result<Vec<String>, (String, bool)>,
) {
    match result {
        Ok(models) if !models.is_empty() => {
            let models = zenith_relay_core::normalize_model_ids(models);
            if account.models.is_empty() {
                account.models = models.clone();
            }
            account.discovered_models = Some(models);
            let recovered = account
                .last_error_code
                .as_deref()
                .is_some_and(|code| code.starts_with("models_"));
            if recovered {
                account.last_error_code = None;
                if !matches!(account.auth_state, AccountAuthState::RequiresReauth(_)) {
                    if account.auth_state == AccountAuthState::Error {
                        account.auth_state = AccountAuthState::Active;
                    }
                    account.health = AccountHealthState::Healthy;
                }
            }
        }
        Ok(_) if account.effective_models().is_empty() => {
            apply_model_failure(account, "models_empty", false)
        }
        Err((code, retryable)) => {
            // Cached model slugs remain routable, but the failed refresh must
            // remain visible to management clients as stale availability.
            apply_model_failure(account, &code, retryable)
        }
        Ok(_) => {}
    }
}

fn model_discovery_was_unauthorized(result: &Result<Vec<String>, (String, bool)>) -> bool {
    matches!(result, Err((code, _)) if code == "models_unauthorized")
}

async fn discover_account_models(
    state: &Arc<AppState>,
    account: &ServerAccountRecord,
) -> Result<Vec<String>, (String, bool)> {
    let secret = state
        .vault
        .load(&account.secret_ref)
        .map_err(|_| ("models_secret_load".to_string(), true))?
        .ok_or_else(|| ("models_secret_missing".to_string(), false))?;
    let credential: AccountCredential =
        serde_json::from_str(&secret).map_err(|_| ("models_secret_invalid".to_string(), false))?;
    let (mut credential, mut authorization) =
        prepare_server_account_authorization(state, account, credential, None)
            .await
            .map_err(|_| ("models_authorization_prepare".to_string(), true))?;
    let proxy = account_proxy_config(state, account, &credential)
        .map_err(|_| ("models_proxy_unavailable".to_string(), false))?;
    let client = CodexModelsClient::new_with_proxy_and_timeout_and_user_agent(
        proxy.as_ref(),
        Duration::from_secs(20),
        "Zenith Relay Server",
    )
    .map_err(|_| ("models_client_init".to_string(), false))?;
    let mut result = client
        .discover_authorized(
            authorization,
            &credential.chatgpt_account_id,
            CODEX_MODELS_CLIENT_VERSION,
        )
        .await;
    if credential.is_agent_identity()
        && matches!(
            result.as_ref(),
            Err(ModelDiscoveryFailure {
                code: ModelDiscoveryFailureCode::AgentTaskInvalid,
                ..
            })
        )
    {
        let expected_task_id = credential.agent_task_id.clone().unwrap_or_default();
        (credential, authorization) = prepare_server_account_authorization(
            state,
            account,
            credential,
            Some(&expected_task_id),
        )
        .await
        .map_err(|_| ("models_authorization_prepare".to_string(), true))?;
        result = client
            .discover_authorized(
                authorization,
                &credential.chatgpt_account_id,
                CODEX_MODELS_CLIENT_VERSION,
            )
            .await;
    }
    result.map_err(model_discovery_error)
}

fn model_discovery_error(error: ModelDiscoveryFailure) -> (String, bool) {
    let code = match error.code {
        // The server retries agent task registration once. A second failed
        // attempt used to be handled as its 401 response category.
        ModelDiscoveryFailureCode::AgentTaskInvalid => "models_unauthorized",
        ModelDiscoveryFailureCode::Forbidden => "models_forbidden",
        ModelDiscoveryFailureCode::HttpStatus => "models_http_status",
        ModelDiscoveryFailureCode::InvalidAccessToken => "models_invalid_access_token",
        ModelDiscoveryFailureCode::InvalidAccountId => "models_invalid_account_id",
        ModelDiscoveryFailureCode::InvalidClientVersion => "models_invalid_client_version",
        ModelDiscoveryFailureCode::InvalidEndpoint => "models_client_init",
        ModelDiscoveryFailureCode::InvalidResponse => "models_invalid_response",
        ModelDiscoveryFailureCode::RateLimited => "models_rate_limited",
        ModelDiscoveryFailureCode::ResponseTooLarge => "models_response_too_large",
        ModelDiscoveryFailureCode::Transport => "models_transport",
        ModelDiscoveryFailureCode::Unauthorized => "models_unauthorized",
        ModelDiscoveryFailureCode::Upstream => "models_upstream",
    };
    (code.to_string(), error.retryable)
}

fn bearer_authorization(access_token: &str) -> Result<HeaderValue, ()> {
    let mut authorization =
        HeaderValue::from_str(&format!("Bearer {access_token}")).map_err(|_| ())?;
    authorization.set_sensitive(true);
    Ok(authorization)
}

fn apply_model_failure(account: &mut ServerAccountRecord, code: &str, retryable: bool) {
    account.last_error_code = Some(code.to_string());
    match code {
        "models_unauthorized" | "models_invalid_access_token" | "models_invalid_account_id" => {
            // Reauthentication is a terminal, user-actionable state. A
            // later failed model probe must not downgrade it to generic Error.
            if !matches!(account.auth_state, AccountAuthState::RequiresReauth(_)) {
                account.auth_state = AccountAuthState::Error;
            }
            account.health = AccountHealthState::Unhealthy;
        }
        "models_forbidden" => account.health = AccountHealthState::Blocked,
        _ if retryable => account.health = AccountHealthState::Degraded,
        _ => account.health = AccountHealthState::Unhealthy,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use zenith_relay_core::quota::QuotaSnapshot;

    #[test]
    fn reset_credit_count_parser_accepts_nested_provider_shapes() {
        for (payload, expected) in [
            (serde_json::json!({"available_count": 2}), Some(2)),
            (serde_json::json!({"data": {"availableCount": 3}}), Some(3)),
            (serde_json::json!({"items": [{"count": 1}]}), Some(1)),
            (serde_json::json!({"available_count": "bad"}), None),
        ] {
            assert_eq!(find_available_reset_credits(&payload), expected);
        }
    }

    fn account(models: &[&str]) -> ServerAccountRecord {
        ServerAccountRecord {
            id: "account-test".into(),
            label: "Account".into(),
            identity_hint: "a***@example.test".into(),
            enabled: true,
            in_pool: true,
            draining: false,
            source_id: "codex".into(),
            secret_ref: "account:account-test".into(),
            auth_state: AccountAuthState::Active,
            health: AccountHealthState::Healthy,
            models: models.iter().map(|model| (*model).to_string()).collect(),
            discovered_models: None,
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
            priority: 0,
            weight: 1,
            subscription: Default::default(),
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

    #[test]
    fn model_refresh_keeps_baseline_and_last_good_effective_list() {
        let mut record = account(&["gpt-old"]);
        apply_discovered_models(&mut record, Ok(vec!["gpt-future-codex".into()]));
        assert_eq!(record.models, ["gpt-old"]);
        assert!(record
            .discovered_models
            .as_ref()
            .is_some_and(|models| models.len() == 1 && models[0] == "gpt-future-codex"));
        assert_eq!(record.effective_models(), ["gpt-future-codex"]);

        apply_discovered_models(&mut record, Err(("models_transport".into(), true)));
        assert_eq!(record.models, ["gpt-old"]);
        assert_eq!(record.effective_models(), ["gpt-future-codex"]);
        assert_eq!(record.last_error_code.as_deref(), Some("models_transport"));

        let mut empty = account(&[]);
        apply_discovered_models(&mut empty, Err(("models_transport".into(), true)));
        assert_eq!(empty.health, AccountHealthState::Degraded);
        assert_eq!(empty.last_error_code.as_deref(), Some("models_transport"));

        apply_discovered_models(&mut empty, Ok(vec!["gpt-recovered".into()]));
        assert_eq!(empty.models, ["gpt-recovered"]);
        assert!(empty
            .discovered_models
            .as_ref()
            .is_some_and(|models| models.len() == 1 && models[0] == "gpt-recovered"));
        assert_eq!(empty.health, AccountHealthState::Healthy);
        assert!(empty.last_error_code.is_none());
    }

    #[test]
    fn successful_model_refresh_recovers_a_transient_auth_error() {
        let mut record = account(&["gpt-live"]);
        record.auth_state = AccountAuthState::Error;
        record.health = AccountHealthState::Unhealthy;
        record.last_error_code = Some("models_unauthorized".into());

        apply_discovered_models(&mut record, Ok(vec!["gpt-recovered".into()]));

        assert_eq!(record.auth_state, AccountAuthState::Active);
        assert_eq!(record.health, AccountHealthState::Healthy);
        assert_eq!(record.last_error_code, None);
    }

    #[test]
    fn model_unauthorized_removes_a_cached_server_account_from_routing() {
        let mut record = account(&["gpt-live"]);
        let failure = Err(("models_unauthorized".to_string(), false));
        assert!(model_discovery_was_unauthorized(&failure));

        apply_discovered_models(&mut record, failure);

        assert_eq!(record.models, ["gpt-live"]);
        assert_eq!(record.auth_state, AccountAuthState::Error);
        assert_eq!(record.health, AccountHealthState::Unhealthy);
        assert_eq!(
            record.last_error_code.as_deref(),
            Some("models_unauthorized")
        );
    }

    #[test]
    fn model_unauthorized_does_not_downgrade_server_reauthentication() {
        let mut record = account(&["gpt-live"]);
        record.auth_state = AccountAuthState::RequiresReauth(
            zenith_relay_core::accounts::ReauthReason::InvalidGrant,
        );
        apply_discovered_models(&mut record, Err(("models_unauthorized".to_string(), false)));

        assert!(matches!(
            record.auth_state,
            AccountAuthState::RequiresReauth(_)
        ));
        assert_eq!(record.health, AccountHealthState::Unhealthy);
    }

    #[test]
    fn shared_model_discovery_failures_keep_server_error_categories() {
        let agent_task = model_discovery_error(ModelDiscoveryFailure {
            code: ModelDiscoveryFailureCode::AgentTaskInvalid,
            retryable: false,
            http_status: Some(401),
        });
        assert_eq!(agent_task, ("models_unauthorized".to_string(), false));

        let rate_limit = model_discovery_error(ModelDiscoveryFailure {
            code: ModelDiscoveryFailureCode::RateLimited,
            retryable: true,
            http_status: Some(429),
        });
        assert_eq!(rate_limit, ("models_rate_limited".to_string(), true));
    }
}
