use super::*;

pub(super) async fn execute(state: &Arc<AppState>, permit: &WakePermit) -> WakeCompletion {
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
        .find(|account_record| account_record.id == permit.account_id)
        .ok_or_else(|| error_codes::WAKE_ACCOUNT_MISSING.to_string())?;
    if account.last_used_at_ms.is_some_and(|last_used_at_ms| {
        last_used_at_ms
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
    let (mut credential, mut authorization, _) =
        prepare_server_account_authorization(state, &account, credential, None)
            .await
            .map_err(|_| "wake_authorization_prepare".to_string())?;
    let identity = CodexIdentityEnvelope::standard(&credential.chatgpt_account_id)
        .map_err(|_| "wake_account_id_invalid".to_string())?;
    let proxy = account_proxy_config(state, &account, &credential)
        .map_err(|_| error_codes::WAKE_PROXY_UNAVAILABLE.to_string())?;
    let builder = reqwest::Client::builder()
        .redirect(Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(60))
        .user_agent("Zenith Relay Server");
    let wake_client = match proxy.as_ref() {
        Some(proxy) => proxy.apply(builder),
        None => builder,
    }
    .build()
    .map_err(|_| "wake_client_init".to_string())?;
    let (mut upstream_status, mut response_body) = send_wake_request(
        &wake_client,
        &identity,
        &credential.responses_url,
        authorization,
        permit,
    )
    .await?;
    if credential.is_agent_identity()
        && zenith_relay_core::providers::chatgpt::is_agent_identity_task_invalid_response(
            upstream_status.as_u16(),
            &response_body,
        )
    {
        let expected_task_id = credential.agent_task_id.clone().unwrap_or_default();
        (credential, authorization, _) = prepare_server_account_authorization(
            state,
            &account,
            credential,
            Some(&expected_task_id),
        )
        .await
        .map_err(|_| "wake_authorization_prepare".to_string())?;
        (upstream_status, response_body) = send_wake_request(
            &wake_client,
            &identity,
            &credential.responses_url,
            authorization,
            permit,
        )
        .await?;
    }
    if !upstream_status.is_success() {
        return Err(match upstream_status.as_u16() {
            401 => error_codes::WAKE_UNAUTHORIZED,
            403 => error_codes::WAKE_FORBIDDEN,
            429 => error_codes::WAKE_RATE_LIMITED,
            _ => "wake_upstream_failed",
        }
        .to_string());
    }
    let usage = serde_json::from_slice::<serde_json::Value>(&response_body)
        .ok()
        .and_then(|response_payload| response_payload.get("usage").cloned());
    drop(response_body);
    tokio::time::sleep(Duration::from_millis(permit.verification_delay_ms)).await;
    let updated = crate::jobs::refresh::request(
        state,
        &account.id,
        zenith_relay_core::scheduler::refresh::RefreshKind::Quota,
    )
    .await?
    .account;
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
        .and_then(|usage_object| usage_object.get("input_tokens"))
        .and_then(serde_json::Value::as_u64);
    let output_tokens = usage
        .as_ref()
        .and_then(|usage_object| usage_object.get("output_tokens"))
        .and_then(serde_json::Value::as_u64);
    Ok((outcome, input_tokens, output_tokens))
}

async fn send_wake_request(
    wake_http_client: &reqwest::Client,
    identity: &CodexIdentityEnvelope,
    responses_url: &str,
    authorization: HeaderValue,
    permit: &WakePermit,
) -> Result<(reqwest::StatusCode, Vec<u8>), String> {
    let (response, http_permit) =
        zenith_relay_core::scheduler::refresh::http::management_http_gate()
            .send(
                wake_http_client,
                identity.apply(
                    wake_http_client
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
                ),
                zenith_relay_core::scheduler::refresh::http::HttpClass::Ordinary,
            )
            .await
            .map_err(|_| error_codes::WAKE_TRANSPORT.to_string())?;
    let response_status = response.status();
    let mut response_body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| error_codes::WAKE_TRANSPORT.to_string())?;
        if response_body.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            return Err(error_codes::WAKE_RESPONSE_TOO_LARGE.to_string());
        }
        response_body.extend_from_slice(&chunk);
    }
    drop(http_permit);
    Ok((response_status, response_body))
}
