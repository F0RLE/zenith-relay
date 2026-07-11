use crate::{
    app::account_proxy_config,
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
        ResetTime, SubscriptionInput, SupplementalQuotaWindowInput,
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
    let proxy = account_proxy_config(state, &credential)
        .map_err(|_| ("quota_proxy_unavailable".to_string(), false))?;
    let builder = reqwest::Client::builder()
        .redirect(Policy::none())
        .timeout(Duration::from_secs(20))
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
    parse_payload(&bytes, now_ms()).map_err(|code| (code, false))
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

fn parse_payload(body: &[u8], observed_at_ms: u64) -> Result<QuotaRefreshData, String> {
    let payload: UsagePayload =
        serde_json::from_slice(body).map_err(|_| "quota_invalid_response".to_string())?;
    let supplemental = collect_supplemental_windows(&payload, observed_at_ms);
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
        supplemental,
        subscription,
        reset_credits_available: payload
            .rate_limit_reset_credits
            .and_then(|value| u32::try_from(value.available_count).ok()),
        observed_at_ms,
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
                }],
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
        assert!(data.supplemental.is_empty());
        assert_eq!(
            data.normalize(&Default::default())
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
        assert!(without_optional.supplemental.is_empty());
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
}
