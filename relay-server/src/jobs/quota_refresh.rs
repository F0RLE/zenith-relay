use crate::{
    jobs::wake_automation,
    state::{now_ms, AccountCredential, AppState, ServerAccountRecord},
};
use reqwest::{
    header::{HeaderValue, AUTHORIZATION},
    redirect::Policy,
};
use serde::Deserialize;
use std::{sync::Arc, time::Duration};
use zenith_relay_core::{
    accounts::{AccountAuthState, AccountHealthState},
    quota::{
        QuotaErrorState, QuotaRefreshData, QuotaTransition, QuotaWindowInput, QuotaWindowKind,
        ResetTime, SubscriptionInput,
    },
};

const INTERVAL: Duration = Duration::from_secs(300);
const CODEX_QUOTA_ENDPOINT: &str = "https://chatgpt.com/backend-api/wham/usage";
const MAX_RESPONSE_BYTES: usize = 256 * 1024;

pub fn start(state: Arc<AppState>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            let _ = run(&state).await;
        }
    });
}

async fn run(state: &Arc<AppState>) -> Result<(), String> {
    for account in state.store.accounts()? {
        if !account.enabled || account.draining {
            continue;
        }
        let (updated, transitions) = refresh_one(state, account).await?;
        if !transitions.is_empty() {
            wake_automation::schedule_transitions(state, &updated, &transitions).await?;
        }
    }
    state.rebuild_runtime().await
}

pub async fn refresh_one(
    state: &Arc<AppState>,
    mut account: ServerAccountRecord,
) -> Result<(ServerAccountRecord, Vec<QuotaTransition>), String> {
    let result = refresh_data(state, &account).await;
    let previous = account.quota.clone();
    let transitions = match result {
        Ok(data) => {
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
            account.health = AccountHealthState::Healthy;
            account.last_error_code = None;
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

async fn refresh_data(
    state: &Arc<AppState>,
    account: &ServerAccountRecord,
) -> Result<QuotaRefreshData, (String, bool)> {
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
    let client = reqwest::Client::builder()
        .redirect(Policy::none())
        .timeout(Duration::from_secs(20))
        .user_agent("Zenith Relay Server")
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
    parse_payload(&bytes, now_ms()).map_err(|code| (code, false))
}

#[derive(Deserialize)]
struct UsagePayload {
    plan_type: Option<String>,
    rate_limit: Option<RateLimit>,
    rate_limit_reset_credits: Option<ResetCredits>,
}

#[derive(Deserialize)]
struct RateLimit {
    primary_window: Option<RateLimitWindow>,
    secondary_window: Option<RateLimitWindow>,
}

#[derive(Deserialize)]
struct RateLimitWindow {
    used_percent: f64,
    limit_window_seconds: Option<i64>,
    reset_after_seconds: Option<i64>,
    reset_at: Option<i64>,
}

#[derive(Deserialize)]
struct ResetCredits {
    available_count: i64,
}

fn parse_payload(body: &[u8], observed_at_ms: u64) -> Result<QuotaRefreshData, String> {
    let payload: UsagePayload =
        serde_json::from_slice(body).map_err(|_| "quota_invalid_response".to_string())?;
    let (primary, secondary) = match payload.rate_limit {
        Some(rate_limit) => (
            rate_limit
                .primary_window
                .map(|window| map_window(window, QuotaWindowKind::Primary, observed_at_ms))
                .transpose()?,
            rate_limit
                .secondary_window
                .map(|window| map_window(window, QuotaWindowKind::Secondary, observed_at_ms))
                .transpose()?,
        ),
        None => (None, None),
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
    Ok(QuotaRefreshData {
        primary,
        secondary,
        subscription,
        reset_credits_available: payload
            .rate_limit_reset_credits
            .and_then(|value| u32::try_from(value.available_count).ok()),
        observed_at_ms,
    })
}

fn map_window(
    window: RateLimitWindow,
    kind: QuotaWindowKind,
    observed_at_ms: u64,
) -> Result<QuotaWindowInput, String> {
    if !window.used_percent.is_finite() || !(0.0..=100.0).contains(&window.used_percent) {
        return Err("quota_invalid_percentage".to_string());
    }
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
        available_percent: Some(100.0 - window.used_percent),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quota_payload_maps_primary_and_secondary_windows() {
        let data = parse_payload(
            br#"{
                "plan_type":"plus",
                "rate_limit":{
                    "primary_window":{"used_percent":25,"limit_window_seconds":18000,"reset_after_seconds":60},
                    "secondary_window":{"used_percent":0,"limit_window_seconds":604800,"reset_after_seconds":604800}
                },
                "rate_limit_reset_credits":{"available_count":2}
            }"#,
            1_000,
        )
        .unwrap();
        let (quota, subscription) = data.normalize(&Default::default()).unwrap();
        assert_eq!(quota.primary.unwrap().available_basis_points, Some(7_500));
        assert_eq!(
            quota.secondary.unwrap().available_basis_points,
            Some(10_000)
        );
        assert_eq!(subscription.unwrap().plan_type.as_deref(), Some("plus"));
    }
}
