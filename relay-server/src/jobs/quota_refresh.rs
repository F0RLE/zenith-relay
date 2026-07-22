use crate::{
    app::account_proxy_config,
    state::{now_ms, AccountCredential, AppState, ServerAccountRecord},
};
use futures_util::StreamExt;
use reqwest::{
    header::{HeaderValue, AUTHORIZATION},
    redirect::Policy,
};
use serde::Deserialize;
use std::{collections::HashSet, sync::Arc, time::Duration};
use tokio::{sync::watch, task::JoinHandle};
use zenith_relay_core::{
    accounts::{
        reduce_account_quota, AccountAuthState, AccountHealthState, AccountQuotaOutcome,
        CodexIdentityEnvelope, CODEX_MODELS_CLIENT_VERSION,
    },
    quota::{
        subscription_refresh_due, CodexQuotaClient, CodexQuotaRefreshData, QuotaRefreshFailure,
        QuotaTransition,
    },
};

const CODEX_MODELS_ENDPOINT: &str = "https://chatgpt.com/backend-api/codex/models";
const MAX_MODELS_RESPONSE_BYTES: usize = 512 * 1024;
const MAX_MODELS: usize = 4_096;
const MAX_MODEL_SLUG_BYTES: usize = 256;

pub fn start(state: Arc<AppState>, mut shutdown: watch::Receiver<bool>) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        break;
                    }
                }
                _ = async {
                    let _ = run(&state).await;
                    let refresh_interval_seconds = state
                        .store
                        .quota_policy()
                        .map(|policy| policy.0)
                        .unwrap_or(crate::store::DEFAULT_QUOTA_REFRESH_INTERVAL_SECONDS);
                    tokio::time::sleep(Duration::from_secs(refresh_interval_seconds)).await;
                } => {}
            }
        }
    })
}

async fn run(state: &Arc<AppState>) -> Result<(), String> {
    super::refresh_automatic_accounts_now(state)
        .await
        .map(|_| ())
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
    let (mut account, transitions) =
        refresh_one(state, account, force_subscription_refresh).await?;
    let mut model_result = discover_account_models(state, &account).await;
    let reauth_state = if model_discovery_was_unauthorized(&model_result) {
        match state
            .recover_account_tokens_after_unauthorized(&account.id)
            .await
        {
            Ok(_) => {
                model_result = discover_account_models(state, &account).await;
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
    apply_discovered_models(&mut account, model_result);
    if let Some(auth_state) = reauth_state {
        account.auth_state = auth_state;
    }
    state.store.save_account(&account)?;
    Ok((account, transitions))
}

pub async fn refresh_one(
    state: &Arc<AppState>,
    mut account: ServerAccountRecord,
    force_subscription_refresh: bool,
) -> Result<(ServerAccountRecord, Vec<QuotaTransition>), String> {
    let result = refresh_data(state, &account, force_subscription_refresh).await;
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
    state.store.save_account(&account)?;
    Ok((account, transitions))
}

async fn refresh_data(
    state: &Arc<AppState>,
    account: &ServerAccountRecord,
    force_subscription_refresh: bool,
) -> Result<CodexQuotaRefreshData, QuotaRefreshFailure> {
    let mut tokens = state
        .prepare_account_tokens(&account.id)
        .await
        .map_err(|_| QuotaRefreshFailure::new("quota_token_prepare", true))?;
    let secret = state
        .vault
        .load(&account.secret_ref)
        .map_err(|_| QuotaRefreshFailure::new("quota_secret_load", true))?
        .ok_or_else(|| QuotaRefreshFailure::new("quota_secret_missing", false))?;
    let credential: AccountCredential = serde_json::from_str(&secret)
        .map_err(|_| QuotaRefreshFailure::new("quota_secret_invalid", false))?;
    let proxy = account_proxy_config(state, &credential)
        .map_err(|_| QuotaRefreshFailure::new("quota_proxy_unavailable", false))?;
    let request_timeout_seconds = state
        .store
        .quota_policy()
        .map_err(|_| QuotaRefreshFailure::new("quota_policy_invalid", false))?
        .1;
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
        .refresh_data_with_subscription(
            tokens.access_token(),
            &credential.chatgpt_account_id,
            observed_at_ms,
            &account.subscription,
            refresh_subscription,
        )
        .await;
    let failure = match first {
        Ok(data) => return Ok(data),
        Err(failure) if failure.http_status() == Some(401) => failure,
        Err(failure) => return Err(failure),
    };
    tokens = match state
        .recover_account_tokens_after_unauthorized(&account.id)
        .await
    {
        Ok(tokens) => tokens,
        Err(_) => return Err(failure),
    };
    client
        .refresh_data_with_subscription(
            tokens.access_token(),
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
            account.models = models;
            if account
                .last_error_code
                .as_deref()
                .is_some_and(|code| code.starts_with("models_"))
            {
                account.last_error_code = None;
            }
        }
        Ok(_) if account.models.is_empty() => apply_model_failure(account, "models_empty", false),
        Err((code, retryable))
            if account.models.is_empty()
                || matches!(
                    code.as_str(),
                    "models_unauthorized"
                        | "models_access_token_invalid"
                        | "models_account_id_invalid"
                ) =>
        {
            apply_model_failure(account, &code, retryable)
        }
        Ok(_) | Err(_) => {}
    }
}

fn model_discovery_was_unauthorized(result: &Result<Vec<String>, (String, bool)>) -> bool {
    matches!(result, Err((code, _)) if code == "models_unauthorized")
}

async fn discover_account_models(
    state: &Arc<AppState>,
    account: &ServerAccountRecord,
) -> Result<Vec<String>, (String, bool)> {
    let tokens = state
        .prepare_account_tokens(&account.id)
        .await
        .map_err(|_| ("models_token_prepare".to_string(), true))?;
    let secret = state
        .vault
        .load(&account.secret_ref)
        .map_err(|_| ("models_secret_load".to_string(), true))?
        .ok_or_else(|| ("models_secret_missing".to_string(), false))?;
    let credential: AccountCredential =
        serde_json::from_str(&secret).map_err(|_| ("models_secret_invalid".to_string(), false))?;
    let identity =
        CodexIdentityEnvelope::new(&credential.chatgpt_account_id, CODEX_MODELS_CLIENT_VERSION)
            .map_err(|_| ("models_account_id_invalid".to_string(), false))?;
    let authorization = HeaderValue::from_str(&format!("Bearer {}", tokens.access_token()))
        .map_err(|_| ("models_access_token_invalid".to_string(), false))?;
    let proxy = account_proxy_config(state, &credential)
        .map_err(|_| ("models_proxy_unavailable".to_string(), false))?;
    let builder = reqwest::Client::builder()
        .redirect(Policy::none())
        .timeout(Duration::from_secs(20))
        .user_agent("Zenith Relay Server");
    let client = match proxy.as_ref() {
        Some(proxy) => proxy.apply(builder),
        None => builder,
    }
    .build()
    .map_err(|_| ("models_client_init".to_string(), false))?;
    let response = identity
        .apply(
            client
                .get(CODEX_MODELS_ENDPOINT)
                .query(&[("client_version", CODEX_MODELS_CLIENT_VERSION)])
                .header(AUTHORIZATION, authorization),
        )
        .send()
        .await
        .map_err(|_| ("models_transport".to_string(), true))?;
    let status = response.status();
    let body = collect_limited(response, MAX_MODELS_RESPONSE_BYTES).await?;
    if !status.is_success() {
        return Err(match status.as_u16() {
            401 => ("models_unauthorized".to_string(), false),
            403 => ("models_forbidden".to_string(), false),
            429 => ("models_rate_limited".to_string(), true),
            _ if status.is_server_error() => ("models_upstream".to_string(), true),
            _ => ("models_http_status".to_string(), false),
        });
    }
    parse_models(&body).map_err(|code| (code, false))
}

async fn collect_limited(
    response: reqwest::Response,
    max_bytes: usize,
) -> Result<Vec<u8>, (String, bool)> {
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| ("models_transport".to_string(), true))?;
        if body.len().saturating_add(chunk.len()) > max_bytes {
            return Err(("models_response_too_large".to_string(), false));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

#[derive(Deserialize)]
struct ModelsPayload {
    models: Vec<ModelPayload>,
}

#[derive(Deserialize)]
struct ModelPayload {
    slug: String,
    #[serde(default)]
    supported_in_api: Option<bool>,
    #[serde(default)]
    visibility: Option<String>,
    #[serde(default)]
    upgrade: Option<serde_json::Value>,
}

fn parse_models(body: &[u8]) -> Result<Vec<String>, String> {
    let response: ModelsPayload =
        serde_json::from_slice(body).map_err(|_| "models_invalid_response".to_string())?;
    if response.models.len() > MAX_MODELS {
        return Err("models_invalid_response".to_string());
    }
    let mut seen = HashSet::new();
    Ok(response
        .models
        .into_iter()
        .filter(|model| model.supported_in_api != Some(false))
        .filter(|model| {
            !model
                .visibility
                .as_deref()
                .is_some_and(|visibility| visibility.eq_ignore_ascii_case("hide"))
                || model.upgrade.is_some()
        })
        .filter_map(|model| {
            let slug = model.slug.trim();
            (!slug.is_empty()
                && slug.len() <= MAX_MODEL_SLUG_BYTES
                && !slug.chars().any(char::is_control)
                && seen.insert(slug.to_string()))
            .then(|| slug.to_string())
        })
        .collect())
}

fn apply_model_failure(account: &mut ServerAccountRecord, code: &str, retryable: bool) {
    account.last_error_code = Some(code.to_string());
    match code {
        "models_unauthorized" | "models_access_token_invalid" | "models_account_id_invalid" => {
            account.auth_state = AccountAuthState::Error;
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
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
            priority: 0,
            weight: 1,
            subscription: Default::default(),
            quota: QuotaSnapshot::default(),
            cooldowns: BTreeMap::new(),
            consecutive_failures: 0,
            created_at_ms: 1,
            last_used_at_ms: None,
            last_error_code: None,
        }
    }

    #[test]
    fn model_payload_keeps_only_safe_supported_unique_slugs() {
        let models = parse_models(
            br#"{"models":[
                {"slug":"gpt-test","supported_in_api":true},
                {"slug":" gpt-test "},
                {"slug":"hidden","supported_in_api":false},
                {"slug":"internal","visibility":"hide"},
                {"slug":"legacy","visibility":"hide","upgrade":{"model":"gpt-test"}},
                {"slug":"bad\nslug"},
                {"slug":"gpt-mini"}
            ]}"#,
        )
        .unwrap();
        assert_eq!(models, vec!["gpt-test", "legacy", "gpt-mini"]);
    }

    #[test]
    fn model_refresh_replaces_live_slugs_but_keeps_last_good_list_on_failure() {
        let mut record = account(&["gpt-old"]);
        apply_discovered_models(&mut record, Ok(vec!["gpt-future-codex".into()]));
        assert_eq!(record.models, ["gpt-future-codex"]);

        apply_discovered_models(&mut record, Err(("models_transport".into(), true)));
        assert_eq!(record.models, ["gpt-future-codex"]);
        assert!(record.last_error_code.is_none());

        let mut empty = account(&[]);
        apply_discovered_models(&mut empty, Err(("models_transport".into(), true)));
        assert_eq!(empty.health, AccountHealthState::Degraded);
        assert_eq!(empty.last_error_code.as_deref(), Some("models_transport"));
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
}
