use crate::{
    app::{account_proxy_config, prepare_server_account_authorization},
    state::{AccountCredential, AppState, ServerAccountRecord},
};
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, REFERER, USER_AGENT};
use serde_json::Value;
use std::sync::Arc;
use zenith_relay_core::quota::{QuotaTransition, QuotaWindowKind};

const RESET_CREDITS_URL: &str = "https://chatgpt.com/backend-api/wham/rate-limit-reset-credits";
const RESET_CREDITS_CONSUME_URL: &str =
    "https://chatgpt.com/backend-api/wham/rate-limit-reset-credits/consume";
const MAX_RESET_RESPONSE_BYTES: usize = 256 * 1024;

/// Redeem one ChatGPT reset credit after a weekly window reaches zero. This is
/// intentionally independent from the desktop credential store; the
/// per-account lock and persisted fingerprint make retries idempotent across
/// concurrent refresh workers.
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
    let transition = transitions
        .iter()
        .find(|transition| transition.window_kind == QuotaWindowKind::Secondary);
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

#[cfg(test)]
mod tests {
    use super::find_available_reset_credits;

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
}
