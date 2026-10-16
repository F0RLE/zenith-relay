use crate::{
    app::account_proxy_config,
    state::{AppState, ServerAccountRecord},
    store::AccountRefreshFence,
};
use reqwest::{
    header::{HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE, REFERER, USER_AGENT},
    redirect::Policy,
};
use serde_json::Value;
use std::{sync::Arc, time::Duration};
use zenith_relay_core::quota::{QuotaTransition, QuotaWindowKind};
use zenith_relay_core::scheduler::refresh::http::{management_http_gate, HttpClass};

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
    fence: &AccountRefreshFence,
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
    ensure_reset_scope(state, fence, account)?;
    let prepared = super::refresh::request_authorization(state, fence)
        .await
        .map_err(|_| "reset_credits_authorization_failed".to_string())?;
    ensure_reset_scope(state, fence, account)?;
    let credential = prepared.credential;
    let authorization = prepared.header;
    let proxy = account_proxy_config(state, account, &credential)?;
    let builder = reqwest::Client::builder()
        .redirect(Policy::none())
        .timeout(Duration::from_secs(20));
    let client = match proxy.as_ref() {
        Some(proxy) => proxy.apply(builder),
        None => builder,
    }
    .build()
    .map_err(|_| "reset_credits_client_init".to_string())?;
    let mut account_id = HeaderValue::from_str(&credential.chatgpt_account_id)
        .map_err(|_| "reset_credits_account_invalid".to_string())?;
    account_id.set_sensitive(true);
    let headers = |request: reqwest::RequestBuilder| {
        request
            .header(AUTHORIZATION, authorization.clone())
            .header("ChatGPT-Account-Id", account_id.clone())
            .header(ACCEPT, "application/json")
            .header(CONTENT_TYPE, "application/json")
            .header(REFERER, "https://chatgpt.com/")
            .header(USER_AGENT, "Zenith Relay Server")
    };
    ensure_reset_scope(state, fence, account)?;
    let (snapshot, permit) = management_http_gate()
        .send(
            &client,
            headers(client.get(RESET_CREDITS_URL)),
            HttpClass::Ordinary,
        )
        .await
        .map_err(|_| "reset_credits_fetch_failed".to_string())?;
    if !snapshot.status().is_success() {
        return Ok(false);
    }
    let reset_response_body = collect_reset_body(snapshot).await?;
    drop(permit);
    let available_reset_credits = serde_json::from_slice::<Value>(&reset_response_body)
        .ok()
        .and_then(|response_payload| find_available_reset_credits(&response_payload))
        .unwrap_or(0);
    if available_reset_credits == 0 {
        return Ok(false);
    }
    ensure_reset_scope(state, fence, account)?;
    let redeem_id = uuid::Uuid::new_v4().to_string();
    let (response, permit) = management_http_gate()
        .send(
            &client,
            headers(
                client
                    .post(RESET_CREDITS_CONSUME_URL)
                    .json(&serde_json::json!({"redeem_request_id": redeem_id})),
            ),
            HttpClass::Ordinary,
        )
        .await
        .map_err(|_| "reset_credits_consume_failed".to_string())?;
    if !response.status().is_success() {
        return Ok(false);
    }
    collect_reset_body(response).await?;
    drop(permit);
    ensure_reset_scope(state, fence, account)?;
    state
        .store
        .mark_weekly_reset_applied(&account.id, &transition.fingerprint)?;
    Ok(true)
}

fn ensure_reset_scope(
    state: &AppState,
    fence: &AccountRefreshFence,
    observed: &ServerAccountRecord,
) -> Result<(), String> {
    let (stored_account, stored_fence) = state.store.account_refresh_scope(&fence.account_id)?;
    if &stored_fence != fence || stored_account.quota != observed.quota {
        return Err("account changed during reset verification".into());
    }
    Ok(())
}

async fn collect_reset_body(mut response: reqwest::Response) -> Result<Vec<u8>, String> {
    let mut response_bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "reset_credits_fetch_failed".to_string())?
    {
        if chunk.len() > MAX_RESET_RESPONSE_BYTES.saturating_sub(response_bytes.len()) {
            return Err("reset_credits_response_too_large".into());
        }
        response_bytes.extend_from_slice(&chunk);
    }
    Ok(response_bytes)
}

fn find_available_reset_credits(response_payload: &Value) -> Option<u32> {
    match response_payload {
        Value::Object(response_object) => {
            for key in ["available_count", "availableCount", "count"] {
                if let Some(number) = response_object.get(key).and_then(Value::as_u64) {
                    return u32::try_from(number).ok();
                }
            }
            response_object
                .values()
                .find_map(find_available_reset_credits)
        }
        Value::Array(response_items) => {
            response_items.iter().find_map(find_available_reset_credits)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

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

    #[test]
    fn reset_fence_rejects_newer_account_and_newer_quota_before_a_send() {
        let root = TempDir::new().unwrap();
        let state = crate::test_fixtures::test_app_state(root.path());
        let observed: ServerAccountRecord = serde_json::from_value(serde_json::json!({
            "id": "synthetic", "label": "Synthetic", "identityHint": "synthetic",
            "enabled": false, "inPool": false, "draining": false, "sourceId": "openai_codex",
            "secretRef": "account:synthetic", "authState": zenith_relay_core::accounts::AccountAuthState::Active,
            "health": "healthy", "models": [], "allowedModels": [], "excludedModels": [],
            "priority": 0, "weight": 1, "subscription": zenith_relay_core::quota::Subscription::default(),
            "quota": zenith_relay_core::quota::QuotaSnapshot::default(),
            "cooldowns": {}, "consecutiveFailures": 0
        })).unwrap();
        state.store.save_account(&observed).unwrap();
        let (_, fence) = state.store.account_refresh_scope(&observed.id).unwrap();
        assert!(ensure_reset_scope(&state, &fence, &observed).is_ok());
        state
            .store
            .apply_account_refresh(&fence, |account_record| {
                account_record.quota.updated_at_ms = Some(123);
                Ok(())
            })
            .unwrap();
        assert!(ensure_reset_scope(&state, &fence, &observed).is_err());
        let mut changed = observed.clone();
        changed.enabled = true;
        state.store.save_account(&changed).unwrap();
        assert!(ensure_reset_scope(&state, &fence, &changed).is_err());
    }

    #[tokio::test]
    async fn reset_response_is_bounded_while_streaming_not_after_full_allocation() {
        use axum::{body::Body, routing::get, Router};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new().route(
                    "/reset",
                    get(|| async { Body::from(vec![b'a'; MAX_RESET_RESPONSE_BYTES + 1]) }),
                ),
            )
            .await
            .unwrap();
        });
        let response = reqwest::get(format!("http://{address}/reset"))
            .await
            .unwrap();
        assert_eq!(
            collect_reset_body(response).await.unwrap_err(),
            "reset_credits_response_too_large"
        );
        server.abort();
    }
}
