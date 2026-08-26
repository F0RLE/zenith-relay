use crate::{
    app::{account_proxy_config, prepare_server_account_authorization},
    jobs::quota_refresh,
    state::{now_ms, AccountCredential, AppState, ServerAccountRecord},
};
use futures_util::StreamExt;
use reqwest::{
    header::{HeaderValue, AUTHORIZATION},
    redirect::Policy,
};
use std::{
    collections::BTreeSet,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{sync::watch, task::JoinHandle};
use zenith_relay_core::{
    accounts::{AccountAuthMode, AccountIdentity, AccountRecord},
    automations::{
        model_lightness_rank, verify_wake_countdown, WakeAdapterPolicy, WakeCompletion,
        WakeCompletionOutcome, WakeCoordinator, WakeModel, WakePermit,
    },
    providers::chatgpt::CodexIdentityEnvelope,
    quota::{QuotaTransition, QuotaWindowKind},
};

const INTERVAL: Duration = Duration::from_secs(30);
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const WAKE_PROMPT: &str = "Reply with OK.";

pub fn start(state: Arc<AppState>, shutdown: watch::Receiver<bool>) -> JoinHandle<()> {
    super::start_periodic(state, shutdown, INTERVAL, |state| async move {
        let _ = run_due(&state).await;
    })
}

pub async fn schedule_transitions(
    state: &Arc<AppState>,
    account: &ServerAccountRecord,
    transitions: &[QuotaTransition],
) -> Result<(), String> {
    let _guard = state.wake_lock.lock().await;
    let tasks = state.store.wake_tasks()?;
    let mut coordinator =
        WakeCoordinator::from_state(state.store.wake_state()?).map_err(str::to_string)?;
    let account_record = core_account(account)?;
    let policy = policy(account);
    for task in tasks {
        for transition in transitions {
            let _ = coordinator.evaluate(
                &task,
                &account_record,
                transition,
                account.last_used_at_ms,
                &policy,
                now_ms(),
            );
        }
    }
    state.store.save_wake_state(coordinator.state())
}

async fn run_due(state: &Arc<AppState>) -> Result<(), String> {
    let permits = {
        let _guard = state.wake_lock.lock().await;
        let mut coordinator =
            WakeCoordinator::from_state(state.store.wake_state()?).map_err(str::to_string)?;
        let permits = coordinator.claim_due_automatic(now_ms(), 2);
        state.store.save_wake_state(coordinator.state())?;
        permits
    };
    for permit in permits {
        let completion = execute(state, &permit).await;
        let _guard = state.wake_lock.lock().await;
        let mut coordinator =
            WakeCoordinator::from_state(state.store.wake_state()?).map_err(str::to_string)?;
        coordinator.complete(permit, completion);
        state.store.save_wake_state(coordinator.state())?;
    }
    Ok(())
}

async fn execute(state: &Arc<AppState>, permit: &WakePermit) -> WakeCompletion {
    let started_at_ms = now_ms();
    let started = Instant::now();
    match execute_inner(state, permit).await {
        Ok((outcome, input_tokens, output_tokens)) => WakeCompletion {
            outcome,
            completed_at_ms: now_ms(),
            latency_ms: Some(started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64),
            input_tokens,
            output_tokens,
            error_code: None,
        },
        Err(code) => WakeCompletion {
            outcome: WakeCompletionOutcome::Failed,
            completed_at_ms: now_ms().max(started_at_ms),
            latency_ms: Some(started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64),
            input_tokens: None,
            output_tokens: None,
            error_code: Some(code),
        },
    }
}

async fn execute_inner(
    state: &Arc<AppState>,
    permit: &WakePermit,
) -> Result<(WakeCompletionOutcome, Option<u64>, Option<u64>), String> {
    let account = state
        .store
        .accounts()?
        .into_iter()
        .find(|record| record.id == permit.account_id)
        .ok_or_else(|| "wake_account_missing".to_string())?;
    if account.last_used_at_ms.is_some_and(|value| {
        value
            >= permit
                .verification
                .baseline_window
                .as_ref()
                .map_or(permit.due_at_ms, |window| window.observed_at_ms)
    }) {
        return Err("wake_natural_use_observed".to_string());
    }
    let secret = state
        .vault
        .load(&account.secret_ref)?
        .ok_or_else(|| "wake_secret_missing".to_string())?;
    let credential: AccountCredential =
        serde_json::from_str(&secret).map_err(|_| "wake_secret_invalid".to_string())?;
    let (mut credential, mut authorization) =
        prepare_server_account_authorization(state, &account, credential, None)
            .await
            .map_err(|_| "wake_authorization_prepare".to_string())?;
    let identity = CodexIdentityEnvelope::standard(&credential.chatgpt_account_id)
        .map_err(|_| "wake_account_id_invalid".to_string())?;
    let proxy = account_proxy_config(state, &account, &credential)
        .map_err(|_| "wake_proxy_unavailable".to_string())?;
    let builder = reqwest::Client::builder()
        .redirect(Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(60))
        .user_agent("Zenith Relay Server");
    let client = match proxy.as_ref() {
        Some(proxy) => proxy.apply(builder),
        None => builder,
    }
    .build()
    .map_err(|_| "wake_client_init".to_string())?;
    let (mut status, mut bytes) = send_wake_request(
        &client,
        &identity,
        &credential.responses_url,
        authorization,
        permit,
    )
    .await?;
    if credential.is_agent_identity()
        && zenith_relay_core::providers::chatgpt::is_agent_identity_task_invalid_response(
            status.as_u16(),
            &bytes,
        )
    {
        let expected_task_id = credential.agent_task_id.clone().unwrap_or_default();
        (credential, authorization) = prepare_server_account_authorization(
            state,
            &account,
            credential,
            Some(&expected_task_id),
        )
        .await
        .map_err(|_| "wake_authorization_prepare".to_string())?;
        (status, bytes) = send_wake_request(
            &client,
            &identity,
            &credential.responses_url,
            authorization,
            permit,
        )
        .await?;
    }
    if !status.is_success() {
        return Err(match status.as_u16() {
            401 => "wake_unauthorized",
            403 => "wake_forbidden",
            429 => "wake_rate_limited",
            _ => "wake_upstream_failed",
        }
        .to_string());
    }
    let usage = serde_json::from_slice::<serde_json::Value>(&bytes)
        .ok()
        .and_then(|value| value.get("usage").cloned());
    drop(bytes);
    tokio::time::sleep(Duration::from_millis(permit.verification_delay_ms)).await;
    let (updated, _) = quota_refresh::refresh_one(state, account, false).await?;
    let after = updated.quota.window(permit.window_kind);
    let outcome = match verify_wake_countdown(permit.verification.baseline_window.as_ref(), after) {
        zenith_relay_core::automations::WakeVerificationOutcome::ConfirmedQuotaConsumed
        | zenith_relay_core::automations::WakeVerificationOutcome::ConfirmedCountdownAdvanced => {
            WakeCompletionOutcome::Confirmed
        }
        zenith_relay_core::automations::WakeVerificationOutcome::Unconfirmed => {
            WakeCompletionOutcome::Unconfirmed
        }
    };
    let input_tokens = usage
        .as_ref()
        .and_then(|value| value.get("input_tokens"))
        .and_then(serde_json::Value::as_u64);
    let output_tokens = usage
        .as_ref()
        .and_then(|value| value.get("output_tokens"))
        .and_then(serde_json::Value::as_u64);
    Ok((outcome, input_tokens, output_tokens))
}

async fn send_wake_request(
    client: &reqwest::Client,
    identity: &CodexIdentityEnvelope,
    responses_url: &str,
    authorization: HeaderValue,
    permit: &WakePermit,
) -> Result<(reqwest::StatusCode, Vec<u8>), String> {
    let response = identity
        .apply(
            client
                .post(responses_url)
                .header(AUTHORIZATION, authorization)
                .json(&serde_json::json!({
                    "model": permit.model_id,
                    "input": WAKE_PROMPT,
                    "stream": false,
                    "max_output_tokens": permit.output_token_cap,
                    "reasoning": { "effort": "minimal" },
                    "tools": []
                })),
        )
        .send()
        .await
        .map_err(|_| "wake_transport".to_string())?;
    let status = response.status();
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| "wake_transport".to_string())?;
        if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            return Err("wake_response_too_large".to_string());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok((status, bytes))
}

pub(crate) fn core_account(account: &ServerAccountRecord) -> Result<AccountRecord, String> {
    let identity = AccountIdentity::from_hashed_parts(
        "openai_codex",
        "chatgpt.com",
        &account.identity_hint,
        &account.identity_hint,
        "remote",
        None,
    )
    .map_err(str::to_string)?;
    Ok(AccountRecord {
        id: account.id.clone(),
        label: account.label.clone(),
        identity,
        auth_mode: AccountAuthMode::OAuth,
        auth_state: account.auth_state,
        health: account.health,
        source_id: account.source_id.clone(),
        secret_refs: vec![account.secret_ref.clone()],
        subscription: account.subscription.clone(),
        quota: account.quota.clone(),
        token_generation: 0,
        token_updated_at_ms: None,
        tags: BTreeSet::new(),
        enabled: account.enabled,
        in_pool: account.in_pool,
        draining: account.draining,
        created_at_ms: 0,
        last_used_at_ms: account.last_used_at_ms,
        last_error_code: account.last_error_code.clone(),
    })
}

fn policy(account: &ServerAccountRecord) -> WakeAdapterPolicy {
    WakeAdapterPolicy {
        windows_requiring_activity: BTreeSet::from([
            QuotaWindowKind::Primary,
            QuotaWindowKind::Secondary,
        ]),
        models: account
            .effective_models()
            .iter()
            .enumerate()
            .map(|(index, id)| WakeModel {
                id: id.clone(),
                lightness_rank: model_lightness_rank(id, index),
                wake_capable: true,
            })
            .collect(),
        verification_delay_ms: 5_000,
        output_token_cap: 8,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zenith_relay_core::{
        accounts::{AccountAuthState, AccountHealthState, ReauthReason},
        quota::Subscription,
    };

    #[test]
    fn core_account_keeps_secret_refs_out_of_debug_identity() {
        let account = ServerAccountRecord {
            id: "account_test".into(),
            label: "Test".into(),
            identity_hint: "abcdef123456".into(),
            enabled: true,
            in_pool: true,
            draining: false,
            source_id: "openai_codex".into(),
            secret_ref: "account:synthetic".into(),
            auth_state: AccountAuthState::RequiresReauth(ReauthReason::InvalidGrant),
            health: AccountHealthState::Unhealthy,
            models: vec!["gpt-test".into()],
            discovered_models: None,
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
            priority: 0,
            weight: 1,
            subscription: Subscription::default(),
            quota: Default::default(),
            purchase_cost_micro_usd: None,
            cooldowns: Default::default(),
            consecutive_failures: 0,
            created_at_ms: 1,
            last_used_at_ms: None,
            last_error_code: None,
            proxy_id: None,
            bypass_common_proxy: false,
        };
        let mapped = core_account(&account).unwrap();
        assert_eq!(mapped.id, "account_test");
        assert!(!format!("{:?}", mapped.identity).contains("account:synthetic"));
    }
}
