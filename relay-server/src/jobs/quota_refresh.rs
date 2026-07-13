use crate::{
    app::account_proxy_config,
    jobs::wake_automation,
    state::{now_ms, AccountCredential, AppState, ServerAccountRecord},
};
use futures_util::StreamExt;
use reqwest::{
    header::{HeaderValue, AUTHORIZATION},
    redirect::Policy,
};
use serde::Deserialize;
use std::{collections::HashSet, sync::Arc, time::Duration};
use zenith_relay_core::{
    accounts::{AccountAuthState, AccountHealthState},
    quota::{
        merge_subscription_metadata, subscription_refresh_due, CodexSubscriptionClient,
        QuotaErrorState, QuotaRefreshData, QuotaTransition, QuotaWindowInput, QuotaWindowKind,
        ResetTime, SubscriptionInput, SupplementalQuotaWindowInput,
    },
};

const CODEX_QUOTA_ENDPOINT: &str = "https://chatgpt.com/backend-api/wham/usage";
const CODEX_MODELS_ENDPOINT: &str = "https://chatgpt.com/backend-api/codex/models";
const MAX_RESPONSE_BYTES: usize = 256 * 1024;
const MAX_MODELS_RESPONSE_BYTES: usize = 512 * 1024;
const MAX_MODELS: usize = 4_096;
const MAX_MODEL_SLUG_BYTES: usize = 256;

pub fn start(state: Arc<AppState>) {
    tokio::spawn(async move {
        loop {
            let _ = run(&state).await;
            let refresh_interval_seconds = state
                .store
                .quota_policy()
                .map(|policy| policy.0)
                .unwrap_or(crate::store::DEFAULT_QUOTA_REFRESH_INTERVAL_SECONDS);
            tokio::time::sleep(Duration::from_secs(refresh_interval_seconds)).await;
        }
    });
}

async fn run(state: &Arc<AppState>) -> Result<(), String> {
    for account in state.store.accounts()? {
        if !account.enabled || account.draining {
            continue;
        }
        let (updated, transitions) = refresh_account_metadata(state, account, false).await?;
        if !transitions.is_empty() {
            wake_automation::schedule_transitions(state, &updated, &transitions).await?;
        }
    }
    state.rebuild_runtime().await
}

pub async fn refresh_account_metadata(
    state: &Arc<AppState>,
    account: ServerAccountRecord,
    force_subscription_refresh: bool,
) -> Result<(ServerAccountRecord, Vec<QuotaTransition>), String> {
    let model_account = account.clone();
    let (quota_result, model_result) = tokio::join!(
        refresh_one(state, account, force_subscription_refresh),
        discover_account_models(state, &model_account),
    );
    let (mut account, transitions) = quota_result?;
    apply_discovered_models(&mut account, model_result);
    state.store.save_account(&account)?;
    Ok((account, transitions))
}

pub async fn refresh_one(
    state: &Arc<AppState>,
    mut account: ServerAccountRecord,
    force_subscription_refresh: bool,
) -> Result<(ServerAccountRecord, Vec<QuotaTransition>), String> {
    let result = refresh_data(state, &account, force_subscription_refresh).await;
    let previous = account.quota.clone();
    let transitions = match result {
        Ok((mut data, subscription_error, allowed, limit_reached)) => {
            data.preserve_subscription_metadata(&account.subscription);
            let (quota, subscription) = data
                .normalize(&previous)
                .map_err(|error| error.to_string())?;
            let transitions = [QuotaWindowKind::Primary, QuotaWindowKind::Secondary]
                .into_iter()
                .filter_map(|kind| {
                    quota
                        .window(kind)
                        .and_then(|window| window.full_transition_from(previous.window(kind)))
                })
                .collect();
            account.quota = quota;
            if let Some(subscription) = subscription {
                account.subscription = subscription;
            }
            account.health = successful_quota_health(allowed, limit_reached);
            account.last_error_code = subscription_error;
            transitions
        }
        Err((code, retryable)) => {
            account.quota.error = Some(QuotaErrorState::new(&code, now_ms()));
            account.last_error_code = Some(code.clone());
            match code.as_str() {
                "quota_unauthorized" => {
                    account.auth_state = AccountAuthState::Error;
                    account.health = AccountHealthState::Unhealthy;
                }
                "quota_forbidden" => account.health = AccountHealthState::Blocked,
                _ if retryable => account.health = AccountHealthState::Degraded,
                _ => account.health = AccountHealthState::Unhealthy,
            }
            Vec::new()
        }
    };
    state.store.save_account(&account)?;
    Ok((account, transitions))
}

fn successful_quota_health(
    allowed: Option<bool>,
    limit_reached: Option<bool>,
) -> AccountHealthState {
    if allowed == Some(false) && limit_reached != Some(true) {
        AccountHealthState::Blocked
    } else {
        AccountHealthState::Healthy
    }
}

async fn refresh_data(
    state: &Arc<AppState>,
    account: &ServerAccountRecord,
    force_subscription_refresh: bool,
) -> Result<(QuotaRefreshData, Option<String>, Option<bool>, Option<bool>), (String, bool)> {
    let tokens = state
        .prepare_account_tokens(&account.id)
        .await
        .map_err(|_| ("quota_token_prepare".to_string(), true))?;
    let secret = state
        .vault
        .load(&account.secret_ref)
        .map_err(|_| ("quota_secret_load".to_string(), true))?
        .ok_or_else(|| ("quota_secret_missing".to_string(), false))?;
    let credential: AccountCredential =
        serde_json::from_str(&secret).map_err(|_| ("quota_secret_invalid".to_string(), false))?;
    let account_id = HeaderValue::from_str(&credential.chatgpt_account_id)
        .map_err(|_| ("quota_account_id_invalid".to_string(), false))?;
    let authorization = HeaderValue::from_str(&format!("Bearer {}", tokens.access_token()))
        .map_err(|_| ("quota_access_token_invalid".to_string(), false))?;
    let proxy = account_proxy_config(state, &credential)
        .map_err(|_| ("quota_proxy_unavailable".to_string(), false))?;
    let request_timeout_seconds = state
        .store
        .quota_policy()
        .map_err(|_| ("quota_policy_invalid".to_string(), false))?
        .1;
    let builder = reqwest::Client::builder()
        .redirect(Policy::none())
        .timeout(Duration::from_secs(request_timeout_seconds))
        .user_agent("Zenith Relay Server");
    let client = match proxy.as_ref() {
        Some(proxy) => proxy.apply(builder),
        None => builder,
    }
    .build()
    .map_err(|_| ("quota_client_init".to_string(), false))?;
    let response = client
        .get(CODEX_QUOTA_ENDPOINT)
        .header(AUTHORIZATION, authorization)
        .header("chatgpt-account-id", account_id)
        .send()
        .await
        .map_err(|_| ("quota_transport".to_string(), true))?;
    let status = response.status();
    let bytes = response
        .bytes()
        .await
        .map_err(|_| ("quota_transport".to_string(), true))?;
    if bytes.len() > MAX_RESPONSE_BYTES {
        return Err(("quota_response_too_large".to_string(), false));
    }
    if !status.is_success() {
        return Err(match status.as_u16() {
            401 => ("quota_unauthorized".to_string(), false),
            403 => ("quota_forbidden".to_string(), false),
            429 => ("quota_rate_limited".to_string(), true),
            _ if status.is_server_error() => ("quota_upstream".to_string(), true),
            _ => ("quota_http_status".to_string(), false),
        });
    }
    let observed_at_ms = now_ms();
    let mut parsed = parse_payload(&bytes, observed_at_ms).map_err(|code| (code, false))?;
    let refresh_subscription = force_subscription_refresh
        || subscription_refresh_due(
            account.subscription.active_until_ms,
            account.subscription.updated_at_ms,
            observed_at_ms,
        );
    let subscription_error = if refresh_subscription {
        let subscription = CodexSubscriptionClient::new(client.clone())
            .map_err(|failure| (failure.code, failure.retryable))?;
        let input = parsed
            .quota
            .subscription
            .get_or_insert_with(|| SubscriptionInput {
                plan_type: account.subscription.plan_type.clone(),
                active_until_ms: account.subscription.active_until_ms,
                forbidden: false,
                observed_at_ms,
            });
        match subscription
            .fetch(
                tokens.access_token(),
                &credential.chatgpt_account_id,
                observed_at_ms,
            )
            .await
        {
            Ok(metadata) => {
                merge_subscription_metadata(
                    &mut input.plan_type,
                    &mut input.active_until_ms,
                    metadata,
                );
                None
            }
            Err(failure) => Some(failure.code),
        }
    } else {
        if let Some(input) = parsed.quota.subscription.as_mut() {
            if input.plan_type == account.subscription.plan_type && input.active_until_ms.is_none()
            {
                input.observed_at_ms = account.subscription.updated_at_ms.unwrap_or(observed_at_ms);
            }
        }
        None
    };
    Ok((
        parsed.quota,
        subscription_error,
        parsed.allowed,
        parsed.limit_reached,
    ))
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
        Err((code, retryable)) if account.models.is_empty() => {
            apply_model_failure(account, &code, retryable)
        }
        Ok(_) | Err(_) => {}
    }
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
    let account_id = HeaderValue::from_str(&credential.chatgpt_account_id)
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
    let response = client
        .get(CODEX_MODELS_ENDPOINT)
        .query(&[(
            "client_version",
            zenith_relay_core::accounts::CODEX_MODELS_CLIENT_VERSION,
        )])
        .header(AUTHORIZATION, authorization)
        .header("chatgpt-account-id", account_id)
        .header("originator", "codex_cli_rs")
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
        "models_unauthorized" | "models_access_token_invalid" => {
            account.auth_state = AccountAuthState::Error;
            account.health = AccountHealthState::Unhealthy;
        }
        "models_forbidden" => account.health = AccountHealthState::Blocked,
        _ if retryable => account.health = AccountHealthState::Degraded,
        _ => account.health = AccountHealthState::Unhealthy,
    }
}

#[derive(Deserialize)]
struct UsagePayload {
    plan_type: Option<String>,
    rate_limit: Option<RateLimit>,
    code_review_rate_limit: Option<SupplementalRateLimit>,
    additional_rate_limits: Option<Vec<AdditionalRateLimit>>,
    rate_limit_reset_credits: Option<ResetCredits>,
}

#[derive(Deserialize)]
struct RateLimit {
    #[serde(default)]
    allowed: Option<bool>,
    #[serde(default)]
    limit_reached: Option<bool>,
    primary_window: Option<RateLimitWindow>,
    secondary_window: Option<RateLimitWindow>,
}

#[derive(Clone, Deserialize)]
struct RateLimitWindow {
    #[serde(default)]
    used_percent: Option<f64>,
    limit_window_seconds: Option<i64>,
    reset_after_seconds: Option<i64>,
    reset_at: Option<i64>,
}

#[derive(Deserialize)]
struct SupplementalRateLimit {
    primary_window: Option<RateLimitWindow>,
    secondary_window: Option<RateLimitWindow>,
}

#[derive(Deserialize)]
struct AdditionalRateLimit {
    limit_name: Option<String>,
    metered_feature: Option<String>,
    rate_limit: Option<SupplementalRateLimit>,
}

#[derive(Deserialize)]
struct ResetCredits {
    available_count: i64,
}

#[derive(Debug)]
struct ParsedQuotaData {
    quota: QuotaRefreshData,
    allowed: Option<bool>,
    limit_reached: Option<bool>,
}

fn parse_payload(body: &[u8], observed_at_ms: u64) -> Result<ParsedQuotaData, String> {
    let payload: UsagePayload =
        serde_json::from_slice(body).map_err(|_| "quota_invalid_response".to_string())?;
    let supplemental = collect_supplemental_windows(&payload, observed_at_ms);
    let (primary, secondary, allowed, limit_reached) = match payload.rate_limit {
        Some(rate_limit) => (
            rate_limit
                .primary_window
                .map(|window| map_window(window, QuotaWindowKind::Primary, observed_at_ms))
                .transpose()?,
            rate_limit
                .secondary_window
                .map(|window| map_window(window, QuotaWindowKind::Secondary, observed_at_ms))
                .transpose()?,
            rate_limit.allowed,
            rate_limit.limit_reached,
        ),
        None => (None, None, None, None),
    };
    let subscription = payload
        .plan_type
        .and_then(safe_label)
        .map(|plan_type| SubscriptionInput {
            plan_type: Some(plan_type),
            active_until_ms: None,
            forbidden: false,
            observed_at_ms,
        });
    Ok(ParsedQuotaData {
        quota: QuotaRefreshData {
            primary,
            secondary,
            supplemental,
            subscription,
            reset_credits_available: payload
                .rate_limit_reset_credits
                .and_then(|value| u32::try_from(value.available_count).ok()),
            observed_at_ms,
        },
        allowed,
        limit_reached,
    })
}

fn collect_supplemental_windows(
    payload: &UsagePayload,
    observed_at_ms: u64,
) -> Vec<SupplementalQuotaWindowInput> {
    let mut windows = Vec::new();
    if let Some(rate_limit) = payload.code_review_rate_limit.as_ref() {
        append_supplemental_windows(
            &mut windows,
            "code_review",
            "Code Review",
            rate_limit,
            observed_at_ms,
        );
    }
    for (index, entry) in payload
        .additional_rate_limits
        .as_deref()
        .unwrap_or_default()
        .iter()
        .take(15)
        .enumerate()
    {
        if is_spark_limit(entry) {
            continue;
        }
        let Some(rate_limit) = entry.rate_limit.as_ref() else {
            continue;
        };
        let label = entry
            .limit_name
            .as_deref()
            .and_then(safe_display_label)
            .or_else(|| {
                entry
                    .metered_feature
                    .as_deref()
                    .and_then(safe_display_label)
            })
            .unwrap_or_else(|| "Additional quota".to_string());
        append_supplemental_windows(
            &mut windows,
            &format!("additional:{index}"),
            &label,
            rate_limit,
            observed_at_ms,
        );
    }
    windows
}

fn is_spark_limit(entry: &AdditionalRateLimit) -> bool {
    entry
        .limit_name
        .as_deref()
        .into_iter()
        .chain(entry.metered_feature.as_deref())
        .any(|value| value.to_ascii_lowercase().contains("spark"))
}

fn append_supplemental_windows(
    output: &mut Vec<SupplementalQuotaWindowInput>,
    id_prefix: &str,
    label: &str,
    rate_limit: &SupplementalRateLimit,
    observed_at_ms: u64,
) {
    for (kind, window) in [
        (QuotaWindowKind::Primary, rate_limit.primary_window.as_ref()),
        (
            QuotaWindowKind::Secondary,
            rate_limit.secondary_window.as_ref(),
        ),
    ] {
        let Some(window) = window else { continue };
        let kind_label = match kind {
            QuotaWindowKind::Primary => "primary",
            QuotaWindowKind::Secondary => "secondary",
        };
        let Ok(window) = map_window(window.clone(), kind, observed_at_ms) else {
            continue;
        };
        output.push(SupplementalQuotaWindowInput {
            id: format!("{id_prefix}:{kind_label}"),
            label: label.to_string(),
            window,
        });
    }
}

fn map_window(
    window: RateLimitWindow,
    kind: QuotaWindowKind,
    observed_at_ms: u64,
) -> Result<QuotaWindowInput, String> {
    let used_percent = window
        .used_percent
        .filter(|value| value.is_finite() && (0.0..=100.0).contains(value))
        .ok_or_else(|| "quota_invalid_percentage".to_string())?;
    let reset = window
        .reset_at
        .filter(|value| *value > 0)
        .and_then(|value| u64::try_from(value).ok())
        .map(ResetTime::AbsoluteUnixSeconds)
        .or_else(|| {
            window
                .reset_after_seconds
                .filter(|value| *value >= 0)
                .and_then(|value| u64::try_from(value).ok())
                .map(ResetTime::RelativeSeconds)
        });
    Ok(QuotaWindowInput {
        kind,
        available_percent: Some(100.0 - used_percent),
        explicitly_full: None,
        reset,
        window_minutes: window
            .limit_window_seconds
            .filter(|value| *value > 0)
            .and_then(|value| u32::try_from((value.saturating_add(59)) / 60).ok()),
        provider_cycle_id: None,
        observed_at_ms,
    })
}

fn safe_label(value: String) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')))
    .then(|| value.to_ascii_lowercase())
}

fn safe_display_label(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty() && value.len() <= 128 && !value.chars().any(char::is_control))
        .then(|| value.split_whitespace().collect::<Vec<_>>().join(" "))
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
            last_used_at_ms: None,
            last_error_code: None,
        }
    }

    #[test]
    fn quota_payload_maps_primary_and_secondary_windows() {
        let data = parse_payload(
            br#"{
                "plan_type":"plus",
                "rate_limit":{
                    "primary_window":{"used_percent":25,"limit_window_seconds":18000,"reset_after_seconds":60},
                    "secondary_window":{"used_percent":0,"limit_window_seconds":604800,"reset_after_seconds":604800}
                },
                "code_review_rate_limit":{
                    "primary_window":{"used_percent":40,"limit_window_seconds":18000}
                },
                "additional_rate_limits":[{
                    "metered_feature":" GPT-5   Priority ",
                    "rate_limit":{"secondary_window":{"used_percent":10,"limit_window_seconds":604800}}
                },{
                    "limit_name":"GPT-5.3 Codex Spark",
                    "rate_limit":{"primary_window":{"used_percent":50,"limit_window_seconds":18000}}
                }],
                "rate_limit_reset_credits":{"available_count":2}
            }"#,
            1_000,
        )
        .unwrap();
        assert_eq!(data.allowed, None);
        assert_eq!(data.limit_reached, None);
        let (quota, subscription) = data.quota.normalize(&Default::default()).unwrap();
        assert_eq!(quota.primary.unwrap().available_basis_points, Some(7_500));
        assert_eq!(
            quota.secondary.unwrap().available_basis_points,
            Some(10_000)
        );
        assert_eq!(quota.supplemental.len(), 2);
        assert_eq!(quota.supplemental[0].id, "code_review:primary");
        assert_eq!(
            quota.supplemental[0].window.available_basis_points,
            Some(6_000)
        );
        assert_eq!(quota.supplemental[1].id, "additional:0:secondary");
        assert_eq!(quota.supplemental[1].label, "GPT-5 Priority");
        assert_eq!(subscription.unwrap().plan_type.as_deref(), Some("plus"));
    }

    #[test]
    fn free_quota_keeps_its_thirty_day_window_and_access_signal() {
        let data = parse_payload(
            br#"{
                "plan_type":"free",
                "rate_limit":{
                    "allowed":false,
                    "limit_reached":false,
                    "primary_window":{"used_percent":5,"limit_window_seconds":2592000}
                }
            }"#,
            1_000,
        )
        .unwrap();
        assert_eq!(data.allowed, Some(false));
        assert_eq!(data.limit_reached, Some(false));
        let (quota, subscription) = data.quota.normalize(&Default::default()).unwrap();
        let primary = quota.primary.unwrap();
        assert_eq!(primary.available_basis_points, Some(9_500));
        assert_eq!(primary.window_minutes, Some(43_200));
        assert_eq!(subscription.unwrap().plan_type.as_deref(), Some("free"));
        assert_eq!(
            successful_quota_health(data.allowed, data.limit_reached),
            AccountHealthState::Blocked
        );
        assert_eq!(
            successful_quota_health(Some(false), Some(true)),
            AccountHealthState::Healthy
        );
    }

    #[test]
    fn optional_quota_windows_are_absent_or_skipped_when_unusable() {
        let data = parse_payload(
            br#"{
                "rate_limit":{"primary_window":{"used_percent":25}},
                "code_review_rate_limit":{"primary_window":{}},
                "additional_rate_limits":[{
                    "limit_name":"Broken optional limit",
                    "rate_limit":{"primary_window":{"used_percent":101}}
                }]
            }"#,
            1_000,
        )
        .unwrap();
        assert!(data.quota.supplemental.is_empty());
        assert_eq!(
            data.quota
                .normalize(&Default::default())
                .unwrap()
                .0
                .primary
                .unwrap()
                .available_basis_points,
            Some(7_500)
        );

        let without_optional = parse_payload(
            br#"{"rate_limit":{"primary_window":{"used_percent":25}}}"#,
            1_000,
        )
        .unwrap();
        assert!(without_optional.quota.supplemental.is_empty());
    }

    #[test]
    fn primary_window_rejects_missing_or_invalid_usage_percentage() {
        for body in [
            br#"{"rate_limit":{"primary_window":{}}}"#.as_slice(),
            br#"{"rate_limit":{"primary_window":{"used_percent":101}}}"#.as_slice(),
        ] {
            assert_eq!(
                parse_payload(body, 1_000).unwrap_err(),
                "quota_invalid_percentage"
            );
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
                {"slug":"bad\nslug"},
                {"slug":"gpt-mini"}
            ]}"#,
        )
        .unwrap();
        assert_eq!(models, vec!["gpt-test", "gpt-mini"]);
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
}
