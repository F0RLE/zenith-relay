use super::auth::{client_api_forbidden, invalid_host, unauthorized};
use super::errors::{
    apply_cooldown, apply_failure_cooldown_with_hint, apply_failure_state,
    failure_requires_independent_source_endpoint, rate_limit_body_hint, rate_limit_body_hint_value,
    RateLimitBodyHint, TRANSIENT_COOLDOWN_MS,
};
use super::now_ms;
use super::request::{
    forwarded_codex_headers, tool_use_diagnostics, with_forwarded_tool_diagnostics,
    CODEX_RESPONSES_LITE_HEADER,
};
use super::response::{apply_usage, emit_usage, usage_event};
use super::streaming::has_output_delta;
use crate::protocol::ClientWireApi;
use crate::runtime::{AuthenticatedKey, CandidateLease, ExecutorPrepareError, ExecutorRoute};
use crate::{GatewayRuntime, ToolUseDiagnostics, UsageEvent, WireApi};
use axum::body::Body;
use axum::extract::ws::{close_code, CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::header::{AUTHORIZATION, RETRY_AFTER};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Response, StatusCode};
use futures_util::{SinkExt, StreamExt};
use reqwest_websocket::{
    CloseCode as UpstreamCloseCode, Message as UpstreamMessage, Upgrade,
    WebSocket as UpstreamWebSocket,
};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::time::{interval_at, sleep_until, timeout, Instant as TokioInstant, MissedTickBehavior};
const WEBSOCKET_SEMANTIC_TIMEOUT: Duration = Duration::from_secs(180);

const MAX_WEBSOCKET_MESSAGE_BYTES: usize = super::request::MAX_CLIENT_REQUEST_BODY_BYTES;
const MAX_WEBSOCKET_ERROR_BYTES: usize = 1024 * 1024;
const INITIAL_MESSAGE_TIMEOUT: Duration = Duration::from_secs(30);
const UPSTREAM_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const WEBSOCKET_IDLE_TIMEOUT: Duration = Duration::from_secs(300);
const WEBSOCKET_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);
const RESPONSES_WEBSOCKET_BETA: &str = "responses_websockets=2026-02-06";
const RESPONSES_LITE_METADATA_KEY: &str =
    "ws_request_header_x_openai_internal_codex_responses_lite";
const WEBSOCKET_PROTOCOLS: &[WireApi] = &[WireApi::Responses];

pub(super) async fn responses(
    State(runtime): State<Arc<GatewayRuntime>>,
    headers: HeaderMap,
    websocket: WebSocketUpgrade,
) -> Response<Body> {
    if !super::auth::valid_local_host(&headers) {
        return invalid_host();
    }
    let Some(key) = runtime.authenticate(headers.get(AUTHORIZATION)) else {
        return unauthorized();
    };
    if !runtime.allows_client_wire_api(&key, ClientWireApi::Responses) {
        return client_api_forbidden();
    }

    websocket
        .max_message_size(MAX_WEBSOCKET_MESSAGE_BYTES)
        .max_frame_size(MAX_WEBSOCKET_MESSAGE_BYTES)
        .write_buffer_size(64 * 1024)
        .max_write_buffer_size(MAX_WEBSOCKET_MESSAGE_BYTES.saturating_mul(2))
        .on_upgrade(move |socket| handle_connection(socket, runtime, key, headers))
}

async fn handle_connection(
    mut downstream: WebSocket,
    runtime: Arc<GatewayRuntime>,
    key: AuthenticatedKey,
    headers: HeaderMap,
) {
    let request = match read_initial_request(&mut downstream, &runtime, &key, &headers).await {
        Ok(request) => request,
        Err(failure) => {
            send_gateway_error(&mut downstream, &failure).await;
            return;
        }
    };

    let connected = match connect_upstream(&runtime, &key, &headers, request, true, 0).await {
        Ok(connected) => connected,
        Err(failure) => {
            send_gateway_error(&mut downstream, &failure).await;
            return;
        }
    };

    bridge(downstream, runtime, key, headers, connected).await;
}

async fn read_initial_request(
    downstream: &mut WebSocket,
    runtime: &GatewayRuntime,
    key: &AuthenticatedKey,
    headers: &HeaderMap,
) -> Result<ClientRequest, GatewayFailure> {
    let deadline = TokioInstant::now() + INITIAL_MESSAGE_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(TokioInstant::now());
        if remaining.is_zero() {
            return Err(GatewayFailure::request_timeout());
        }
        let message = timeout(remaining, downstream.recv())
            .await
            .map_err(|_| GatewayFailure::request_timeout())?
            .ok_or_else(GatewayFailure::client_closed)?
            .map_err(|_| GatewayFailure::invalid_request("invalid WebSocket frame"))?;
        match message {
            Message::Text(text) => {
                return ClientRequest::parse(runtime, key, headers, text.as_bytes())
            }
            Message::Binary(bytes) => return ClientRequest::parse(runtime, key, headers, &bytes),
            Message::Ping(payload) => {
                if downstream.send(Message::Pong(payload)).await.is_err() {
                    return Err(GatewayFailure::client_closed());
                }
            }
            Message::Pong(_) => {}
            Message::Close(_) => return Err(GatewayFailure::client_closed()),
        }
    }
}

#[derive(Clone)]
struct ClientRequest {
    request_id: String,
    value: Value,
    requested_model: String,
    resolved_model: String,
    responses_lite: bool,
    response_affinity_key: Option<String>,
    prompt_affinity_key: Option<String>,
}

impl ClientRequest {
    fn parse(
        runtime: &GatewayRuntime,
        key: &AuthenticatedKey,
        headers: &HeaderMap,
        payload: &[u8],
    ) -> Result<Self, GatewayFailure> {
        if payload.len() > MAX_WEBSOCKET_MESSAGE_BYTES {
            return Err(GatewayFailure::invalid_request(
                "WebSocket request is too large",
            ));
        }
        let mut value: Value = serde_json::from_slice(payload)
            .map_err(|_| GatewayFailure::invalid_request("request must be valid JSON"))?;
        let object = value
            .as_object_mut()
            .ok_or_else(|| GatewayFailure::invalid_request("request must be a JSON object"))?;
        if object
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|kind| kind != "response.create")
        {
            return Err(GatewayFailure::invalid_request(
                "only response.create messages are supported",
            ));
        }
        let requested_model = object
            .get("model")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|model| !model.is_empty())
            .ok_or_else(|| GatewayFailure::invalid_request("model must be a non-empty string"))?
            .to_string();
        let resolved_model = runtime
            .resolve_visible_model(key, &requested_model, WEBSOCKET_PROTOCOLS, now_ms())
            .ok_or_else(GatewayFailure::model_not_found)?;
        let responses_lite = headers.contains_key(CODEX_RESPONSES_LITE_HEADER)
            || metadata_flag(&value, RESPONSES_LITE_METADATA_KEY)
            || runtime.codex_model_uses_responses_lite(&resolved_model);
        let response_affinity_key = runtime
            .response_affinity_key(value.get("previous_response_id").and_then(Value::as_str));
        let prompt_affinity_key = runtime.prompt_affinity_key(
            &key.id,
            &resolved_model,
            value.get("prompt_cache_key").and_then(Value::as_str),
        );
        Ok(Self {
            request_id: super::request::request_id(),
            value,
            requested_model,
            resolved_model,
            responses_lite,
            response_affinity_key,
            prompt_affinity_key,
        })
    }

    fn payload_for(&self, route: &ExecutorRoute) -> Result<String, GatewayFailure> {
        let mut value = self.value.clone();
        let object = value
            .as_object_mut()
            .expect("request object was validated before routing");
        object.insert(
            "type".to_string(),
            Value::String("response.create".to_string()),
        );
        object.insert(
            "model".to_string(),
            Value::String(route.source_model.clone()),
        );
        if route.account_id.is_some() {
            super::request::normalize_account_request(object, self.responses_lite);
        }
        serde_json::to_string(&value)
            .map_err(|_| GatewayFailure::invalid_request("request could not be serialized"))
    }

    fn tool_use_for(&self, route: &ExecutorRoute) -> ToolUseDiagnostics {
        let client = tool_use_diagnostics(&self.value);
        self.payload_for(route)
            .map(|payload| with_forwarded_tool_diagnostics(&client, payload.as_bytes()))
            .unwrap_or(client)
    }

    fn has_previous_response_id(&self) -> bool {
        self.previous_response_id().is_some()
    }

    fn previous_response_id(&self) -> Option<&str> {
        self.value
            .get("previous_response_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
    }

    fn has_tool_call_output(&self) -> bool {
        super::request::contains_tool_call_output(&self.value)
    }

    fn drop_previous_response_id(&mut self) -> bool {
        let Some(object) = self.value.as_object_mut() else {
            return false;
        };
        if object.remove("previous_response_id").is_some() {
            self.response_affinity_key = None;
            true
        } else {
            false
        }
    }

    fn recover_invalid_encrypted_content(&mut self) -> bool {
        let mut attempted = false;
        super::request::try_recover_encrypted_content(&mut self.value, &mut attempted)
    }
}

struct Connected {
    upstream: UpstreamWebSocket,
    initial_messages: Vec<UpstreamMessage>,
    route: ExecutorRoute,
    request: ClientRequest,
    lease: CandidateLease,
    attempt: u16,
    started: Instant,
}

async fn connect_upstream(
    runtime: &GatewayRuntime,
    key: &AuthenticatedKey,
    client_headers: &HeaderMap,
    mut request: ClientRequest,
    allow_previous_response_reset: bool,
    attempt_offset: u16,
) -> Result<Connected, GatewayFailure> {
    let mut tried = HashSet::new();
    let mut attempt = attempt_offset;
    let mut attempts_this_run = 0_usize;
    let mut owner_recovery_confirmed = false;
    let mut confirmed_response_missing = false;
    let mut encrypted_content_recovered = false;
    let mut last_failure = None;

    'candidates: while attempts_this_run
        < super::errors::retry_candidate_limit(
            runtime.max_retry_candidates(),
            owner_recovery_confirmed,
        ) + usize::from(encrypted_content_recovered)
    {
        let selected = runtime
            .select_and_reserve(
                key,
                &request.resolved_model,
                WEBSOCKET_PROTOCOLS,
                &tried,
                (
                    request.response_affinity_key.as_deref(),
                    request.prompt_affinity_key.as_deref(),
                ),
                now_ms(),
            )
            .await;
        let Some((selected, lease)) = selected else {
            break;
        };
        tried.insert(selected.candidate_id.clone());
        let response_affinity_hit = selected.response_affinity_hit;
        let Some(mut route) =
            runtime.executor_route(&selected.candidate_id, &request.resolved_model)
        else {
            continue;
        };
        route.half_open_probe = selected.half_open_probe;
        route.routing = Some(selected.diagnostics);
        if route.wire_api != WireApi::Responses {
            continue;
        }
        if !route.adapter.is_passthrough() {
            drop(lease);
            last_failure = Some(GatewayFailure::adapter_unsupported());
            continue;
        }
        attempt = attempt.saturating_add(1);
        attempts_this_run = attempts_this_run.saturating_add(1);
        let started = Instant::now();
        let prepared = match runtime
            .prepare_authorization(&route.candidate_id, now_ms())
            .await
        {
            Ok(prepared) => prepared,
            Err(error) => {
                let failure = GatewayFailure::prepare(error);
                record_connect_failure(
                    runtime, key, &route, &request, attempt, started, &failure, None,
                );
                last_failure = Some(failure);
                continue;
            }
        };
        let payload = request.payload_for(&route)?;
        let mut prepared = prepared;
        let mut refresh_fence = None;
        let upgrade = loop {
            let headers = upstream_headers(
                client_headers,
                &prepared,
                request.responses_lite,
                &request.request_id,
            );
            let upgrade = runtime
                .websocket_client(&route.candidate_id)
                .get(route.upstream_url.clone())
                .headers(headers)
                .upgrade();
            let upgrade = match timeout(UPSTREAM_CONNECT_TIMEOUT, upgrade.send()).await {
                Ok(Ok(upgrade)) => upgrade,
                _ => {
                    let failure = GatewayFailure::transport();
                    record_connect_failure(
                        runtime, key, &route, &request, attempt, started, &failure, None,
                    );
                    exclude_correlated_source_endpoint(runtime, &route, &failure, &mut tried);
                    last_failure = Some(failure);
                    continue 'candidates;
                }
            };
            if upgrade.status() != StatusCode::UNAUTHORIZED
                || prepared.token_generation.is_none()
                || refresh_fence.is_some()
            {
                break upgrade;
            }
            drop(upgrade);
            refresh_fence = runtime.fence_execution(&route.candidate_id);
            prepared = match runtime
                .refresh_authorization_after_unauthorized(
                    &route.candidate_id,
                    prepared.token_generation,
                    now_ms(),
                )
                .await
            {
                Ok(prepared) => prepared,
                Err(error) => {
                    let failure = GatewayFailure::prepare(error);
                    record_connect_failure(
                        runtime, key, &route, &request, attempt, started, &failure, None,
                    );
                    last_failure = Some(failure);
                    continue 'candidates;
                }
            };
        };
        drop(refresh_fence);
        let status = upgrade.status();
        let response_headers = upgrade.headers().clone();
        runtime.observe_codex_quota_headers(
            &route.candidate_id,
            status,
            &response_headers,
            now_ms(),
        );
        if status != StatusCode::SWITCHING_PROTOCOLS {
            let response = upgrade.into_inner();
            let body = timeout(
                UPSTREAM_CONNECT_TIMEOUT,
                crate::runtime::collect_limited(response, MAX_WEBSOCKET_ERROR_BYTES),
            )
            .await
            .ok()
            .and_then(Result::ok);
            let failure = GatewayFailure::upstream_status(status, body.as_deref());
            if failure.category == "upstream_encrypted_content_invalid"
                && !encrypted_content_recovered
                && request.recover_invalid_encrypted_content()
            {
                encrypted_content_recovered = true;
                tried.remove(&route.candidate_id);
                record_connect_rejection(
                    runtime, key, &route, &request, attempt, started, &failure,
                );
                last_failure = Some(failure);
                continue;
            }
            let response_missing = body
                .as_deref()
                .is_some_and(super::errors::previous_response_not_found);
            let affinity_miss = super::errors::recoverable_response_affinity_miss(
                status,
                request.has_previous_response_id(),
                response_affinity_hit,
                response_missing,
            );
            if affinity_miss {
                confirmed_response_missing |= response_missing;
                owner_recovery_confirmed |= !response_affinity_hit;
                runtime.invalidate_response_affinity(request.response_affinity_key.as_deref());
                record_connect_affinity_miss(
                    runtime, key, &route, &request, attempt, started, status,
                );
                last_failure = Some(failure);
                if response_missing && response_affinity_hit {
                    break;
                }
                continue;
            }
            if super::errors::retryable_failure(
                status,
                failure.category,
                request.has_previous_response_id(),
            ) {
                record_connect_failure_with_hint(
                    runtime,
                    key,
                    &route,
                    &request,
                    attempt,
                    started,
                    &failure,
                    Some(&response_headers),
                    body.as_deref()
                        .map(rate_limit_body_hint)
                        .unwrap_or_default(),
                );
                exclude_correlated_source_endpoint(runtime, &route, &failure, &mut tried);
                last_failure = Some(failure);
                continue;
            }
            record_connect_rejection(runtime, key, &route, &request, attempt, started, &failure);
            return Err(failure);
        }
        let mut upstream = match timeout(UPSTREAM_CONNECT_TIMEOUT, upgrade.into_websocket()).await {
            Ok(Ok(upstream)) => upstream,
            _ => {
                let failure = GatewayFailure::transport();
                record_connect_failure(
                    runtime, key, &route, &request, attempt, started, &failure, None,
                );
                exclude_correlated_source_endpoint(runtime, &route, &failure, &mut tried);
                last_failure = Some(failure);
                continue;
            }
        };
        if send_request(&mut upstream, payload).await.is_err() {
            let failure = GatewayFailure::transport();
            record_connect_failure(
                runtime, key, &route, &request, attempt, started, &failure, None,
            );
            exclude_correlated_source_endpoint(runtime, &route, &failure, &mut tried);
            last_failure = Some(failure);
            continue;
        }
        let initial_messages = match initial_application_messages(&mut upstream).await {
            Ok(messages) => messages,
            Err(failure) => {
                let response_headers = HeaderMap::new();
                record_connect_failure(
                    runtime,
                    key,
                    &route,
                    &request,
                    attempt,
                    started,
                    &failure,
                    Some(&response_headers),
                );
                exclude_correlated_source_endpoint(runtime, &route, &failure, &mut tried);
                last_failure = Some(failure);
                continue;
            }
        };
        if let Some(terminal) = initial_messages.last().and_then(first_message_terminal) {
            if terminal.outcome == Some(false) {
                let category = terminal.error_category.unwrap_or_else(|| {
                    super::errors::classify_upstream_error(
                        terminal_failure_status(terminal.status),
                        None,
                    )
                    .category
                });
                let status = terminal
                    .status
                    .filter(|status| !status.is_success())
                    .unwrap_or_else(|| super::errors::upstream_failure_status(category));
                if category == "upstream_encrypted_content_invalid"
                    && !encrypted_content_recovered
                    && request.recover_invalid_encrypted_content()
                {
                    encrypted_content_recovered = true;
                    tried.remove(&route.candidate_id);
                    let failure = GatewayFailure::classified(status, category);
                    record_connect_rejection(
                        runtime, key, &route, &request, attempt, started, &failure,
                    );
                    last_failure = Some(failure);
                    continue;
                }
                let affinity_miss = super::errors::recoverable_response_affinity_miss(
                    status,
                    request.has_previous_response_id(),
                    response_affinity_hit,
                    terminal.previous_response_not_found,
                );
                if affinity_miss
                    || super::errors::retryable_failure(
                        status,
                        category,
                        request.has_previous_response_id(),
                    )
                {
                    let failure = GatewayFailure::classified(status, category);
                    if affinity_miss {
                        confirmed_response_missing |= terminal.previous_response_not_found;
                        owner_recovery_confirmed |= !response_affinity_hit;
                        runtime
                            .invalidate_response_affinity(request.response_affinity_key.as_deref());
                        record_connect_affinity_miss(
                            runtime, key, &route, &request, attempt, started, status,
                        );
                    } else {
                        record_connect_failure_with_hint(
                            runtime,
                            key,
                            &route,
                            &request,
                            attempt,
                            started,
                            &failure,
                            Some(&terminal.headers),
                            terminal.body_hint,
                        );
                        exclude_correlated_source_endpoint(runtime, &route, &failure, &mut tried);
                    }
                    last_failure = Some(failure);
                    if affinity_miss
                        && terminal.previous_response_not_found
                        && response_affinity_hit
                    {
                        break;
                    }
                    continue;
                }
            }
        }
        return Ok(Connected {
            upstream,
            initial_messages,
            route,
            request,
            lease,
            attempt,
            started,
        });
    }

    if allow_previous_response_reset
        && request.has_previous_response_id()
        && confirmed_response_missing
        && !request.has_tool_call_output()
    {
        let mut reset_request = request.clone();
        if reset_request.drop_previous_response_id() {
            return Box::pin(connect_upstream(
                runtime,
                key,
                client_headers,
                reset_request,
                false,
                attempt,
            ))
            .await;
        }
    }

    if let Some(retry_at_ms) = runtime.earliest_retry_at(
        key,
        &request.resolved_model,
        WEBSOCKET_PROTOCOLS,
        &HashSet::new(),
        request.response_affinity_key.as_deref(),
        now_ms(),
    ) {
        return Err(GatewayFailure::cooldown(retry_at_ms));
    }
    Err(last_failure.unwrap_or_else(GatewayFailure::unavailable))
}

async fn initial_application_messages(
    upstream: &mut UpstreamWebSocket,
) -> Result<Vec<UpstreamMessage>, GatewayFailure> {
    let mut messages = Vec::new();
    let mut buffered_bytes = 0_usize;
    loop {
        let message = first_application_message(upstream).await?;
        let message_bytes = match &message {
            UpstreamMessage::Text(text) => text.len(),
            UpstreamMessage::Binary(bytes) => bytes.len(),
            _ => 0,
        };
        buffered_bytes = buffered_bytes.saturating_add(message_bytes);
        if buffered_bytes > MAX_WEBSOCKET_MESSAGE_BYTES.saturating_mul(2) {
            return Err(GatewayFailure::bootstrap_too_large());
        }
        let committed = initial_message_state(&message)
            .map(|(has_output, terminal)| has_output || terminal.outcome.is_some())
            .unwrap_or(true);
        messages.push(message);
        if committed {
            return Ok(messages);
        }
    }
}

async fn first_application_message(
    upstream: &mut UpstreamWebSocket,
) -> Result<UpstreamMessage, GatewayFailure> {
    let deadline = TokioInstant::now() + INITIAL_MESSAGE_TIMEOUT;
    let mut heartbeat = interval_at(
        TokioInstant::now() + WEBSOCKET_HEARTBEAT_INTERVAL,
        WEBSOCKET_HEARTBEAT_INTERVAL,
    );
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = sleep_until(deadline) => return Err(GatewayFailure::idle_timeout()),
            _ = heartbeat.tick() => {
                upstream
                    .send(UpstreamMessage::Ping(Default::default()))
                    .await
                    .map_err(|_| GatewayFailure::transport())?;
            }
            message = upstream.next() => {
                match message {
                    Some(Ok(message @ (UpstreamMessage::Text(_) | UpstreamMessage::Binary(_)))) => {
                        return Ok(message);
                    }
                    Some(Ok(UpstreamMessage::Ping(payload))) => {
                        upstream
                            .send(UpstreamMessage::Pong(payload))
                            .await
                            .map_err(|_| GatewayFailure::transport())?;
                    }
                    Some(Ok(UpstreamMessage::Pong(_))) => {}
                    Some(Ok(UpstreamMessage::Close { .. })) | None => {
                        return Err(GatewayFailure::closed());
                    }
                    Some(Err(_)) => return Err(GatewayFailure::transport()),
                }
            }
        }
    }
}

fn first_message_terminal(message: &UpstreamMessage) -> Option<EventTerminal> {
    initial_message_state(message).map(|(_, terminal)| terminal)
}

fn initial_message_state(message: &UpstreamMessage) -> Option<(bool, EventTerminal)> {
    let payload = match message {
        UpstreamMessage::Text(text) => text.as_bytes(),
        UpstreamMessage::Binary(bytes) => bytes.as_ref(),
        _ => return None,
    };
    let value = serde_json::from_slice::<Value>(payload).ok()?;
    let event_type = value.get("type").and_then(Value::as_str);
    Some((has_output_delta(&value, event_type), event_terminal(&value)))
}

fn exclude_correlated_source_endpoint(
    runtime: &GatewayRuntime,
    route: &ExecutorRoute,
    failure: &GatewayFailure,
    tried: &mut HashSet<String>,
) {
    if failure_requires_independent_source_endpoint(failure.status, failure.category) {
        runtime.exclude_same_source_endpoint(&route.candidate_id, tried);
    }
}

#[allow(clippy::too_many_arguments)]
fn record_connect_failure(
    runtime: &GatewayRuntime,
    key: &AuthenticatedKey,
    route: &ExecutorRoute,
    request: &ClientRequest,
    attempt: u16,
    started: Instant,
    failure: &GatewayFailure,
    headers: Option<&HeaderMap>,
) {
    record_connect_failure_with_hint(
        runtime,
        key,
        route,
        request,
        attempt,
        started,
        failure,
        headers,
        RateLimitBodyHint::default(),
    );
}

#[allow(clippy::too_many_arguments)]
fn record_connect_failure_with_hint(
    runtime: &GatewayRuntime,
    key: &AuthenticatedKey,
    route: &ExecutorRoute,
    request: &ClientRequest,
    attempt: u16,
    started: Instant,
    failure: &GatewayFailure,
    headers: Option<&HeaderMap>,
    hint: RateLimitBodyHint,
) {
    let state = match headers {
        Some(headers) => apply_failure_cooldown_with_hint(
            runtime,
            &route.candidate_id,
            &route.source_model,
            failure.status,
            failure.category,
            headers,
            hint,
            route.half_open_probe,
        ),
        None => apply_cooldown(
            runtime,
            &route.candidate_id,
            "*",
            failure.cooldown_ms,
            route.half_open_probe,
        ),
    };
    let mut event = usage_event(
        &request.request_id,
        attempt,
        &key.id,
        route,
        &request.requested_model,
        false,
        failure.status.as_u16(),
        Some(failure.category.to_string()),
        started.elapsed().as_millis() as u64,
        request.tool_use_for(route),
    );
    apply_failure_state(&mut event, state);
    emit_usage(runtime, event);
}

fn record_connect_affinity_miss(
    runtime: &GatewayRuntime,
    key: &AuthenticatedKey,
    route: &ExecutorRoute,
    request: &ClientRequest,
    attempt: u16,
    started: Instant,
    status: StatusCode,
) {
    emit_usage(
        runtime,
        usage_event(
            &request.request_id,
            attempt,
            &key.id,
            route,
            &request.requested_model,
            false,
            status.as_u16(),
            Some("response_affinity_miss".to_string()),
            started.elapsed().as_millis() as u64,
            request.tool_use_for(route),
        ),
    );
}

fn record_connect_rejection(
    runtime: &GatewayRuntime,
    key: &AuthenticatedKey,
    route: &ExecutorRoute,
    request: &ClientRequest,
    attempt: u16,
    started: Instant,
    failure: &GatewayFailure,
) {
    emit_usage(
        runtime,
        usage_event(
            &request.request_id,
            attempt,
            &key.id,
            route,
            &request.requested_model,
            false,
            failure.status.as_u16(),
            Some(failure.category.to_string()),
            started.elapsed().as_millis() as u64,
            request.tool_use_for(route),
        ),
    );
}

fn upstream_headers(
    client_headers: &HeaderMap,
    prepared: &crate::runtime::PreparedAuthorization,
    responses_lite: bool,
    request_id: &str,
) -> HeaderMap {
    let mut headers = forwarded_codex_headers(client_headers, request_id);
    headers.insert(AUTHORIZATION, prepared.authorization.clone());
    if let Some(identity) = prepared.identity.as_ref() {
        identity.insert(&mut headers);
    }
    if responses_lite {
        headers.insert(
            HeaderName::from_static(CODEX_RESPONSES_LITE_HEADER),
            HeaderValue::from_static("true"),
        );
    }
    ensure_websocket_beta(&mut headers);
    headers
}

fn ensure_websocket_beta(headers: &mut HeaderMap) {
    let name = HeaderName::from_static("openai-beta");
    let present = headers
        .get_all(&name)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .any(|value| value.contains("responses_websockets="));
    if !present {
        headers.append(name, HeaderValue::from_static(RESPONSES_WEBSOCKET_BETA));
    }
}

async fn send_request(
    upstream: &mut UpstreamWebSocket,
    payload: String,
) -> Result<(), GatewayFailure> {
    let send = async {
        upstream.send(UpstreamMessage::Text(payload)).await?;
        upstream.flush().await
    };
    match timeout(UPSTREAM_CONNECT_TIMEOUT, send).await {
        Ok(Ok(())) => Ok(()),
        _ => Err(GatewayFailure::transport()),
    }
}

struct InFlight {
    route: ExecutorRoute,
    event: UsageEvent,
    started: Instant,
    response_id: Option<String>,
    prompt_affinity_key: Option<String>,
}

struct BridgeState {
    lease: Option<CandidateLease>,
    in_flight: Option<InFlight>,
    upstream_candidate_id: String,
    last_response_id: Option<String>,
}

async fn bridge(
    mut downstream: WebSocket,
    runtime: Arc<GatewayRuntime>,
    key: AuthenticatedKey,
    headers: HeaderMap,
    connected: Connected,
) {
    let mut upstream = connected.upstream;
    let initial_event = usage_event(
        &connected.request.request_id,
        connected.attempt,
        &key.id,
        &connected.route,
        &connected.request.requested_model,
        true,
        StatusCode::OK.as_u16(),
        None,
        0,
        connected.request.tool_use_for(&connected.route),
    );
    let upstream_candidate_id = connected.route.candidate_id.clone();
    let prompt_affinity_key = connected.request.prompt_affinity_key.clone();
    let mut state = BridgeState {
        lease: Some(connected.lease),
        in_flight: Some(InFlight {
            route: connected.route,
            event: initial_event,
            started: connected.started,
            response_id: None,
            prompt_affinity_key,
        }),
        upstream_candidate_id,
        last_response_id: None,
    };
    for message in connected.initial_messages {
        if !handle_upstream_message(&mut downstream, &runtime, &mut state, message).await {
            return;
        }
    }
    let mut last_activity = TokioInstant::now();
    let mut heartbeat = interval_at(
        TokioInstant::now() + WEBSOCKET_HEARTBEAT_INTERVAL,
        WEBSOCKET_HEARTBEAT_INTERVAL,
    );
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        let idle_deadline = last_activity + WEBSOCKET_IDLE_TIMEOUT;
        let semantic_waiting = state
            .in_flight
            .as_ref()
            .is_some_and(|in_flight| in_flight.event.ttft_ms.is_none());
        let semantic_deadline = TokioInstant::now()
            + state
                .in_flight
                .as_ref()
                .map_or(WEBSOCKET_SEMANTIC_TIMEOUT, |in_flight| {
                    WEBSOCKET_SEMANTIC_TIMEOUT.saturating_sub(in_flight.started.elapsed())
                });
        tokio::select! {
            _ = sleep_until(semantic_deadline), if semantic_waiting => {
                finish_incomplete(&runtime, &mut state, "stream_semantic_timeout");
                send_gateway_error(&mut downstream, &GatewayFailure::semantic_timeout()).await;
                break;
            }
            _ = sleep_until(idle_deadline) => {
                finish_incomplete(&runtime, &mut state, "websocket_idle_timeout");
                let _ = downstream.send(Message::Close(Some(CloseFrame {
                    code: close_code::AWAY,
                    reason: "idle timeout".into(),
                }))).await;
                break;
            }
            _ = heartbeat.tick() => {
                if upstream.send(UpstreamMessage::Ping(Default::default())).await.is_err() {
                    let active_request = state.in_flight.is_some();
                    finish_incomplete(&runtime, &mut state, "upstream_websocket");
                    if active_request {
                        send_gateway_error(&mut downstream, &GatewayFailure::transport()).await;
                    }
                    break;
                }
            }
            message = downstream.recv() => {
                last_activity = TokioInstant::now();
                let Some(message) = message else {
                    finish_incomplete(&runtime, &mut state, "client_cancelled");
                    break;
                };
                let Ok(message) = message else {
                    finish_incomplete(&runtime, &mut state, "client_websocket");
                    break;
                };
                match handle_downstream_message(
                    &mut downstream,
                    &mut upstream,
                    &runtime,
                    &key,
                    &headers,
                    &mut state,
                    message,
                ).await {
                    Ok(true) => {}
                    Ok(false) => break,
                    Err(failure) => {
                        finish_incomplete(&runtime, &mut state, failure.category);
                        send_gateway_error(&mut downstream, &failure).await;
                        break;
                    }
                }
            }
            message = upstream.next() => {
                last_activity = TokioInstant::now();
                let Some(message) = message else {
                    let active_request = state.in_flight.is_some();
                    finish_incomplete(&runtime, &mut state, "upstream_websocket_closed");
                    if active_request {
                        send_gateway_error(&mut downstream, &GatewayFailure::closed()).await;
                    }
                    break;
                };
                let Ok(message) = message else {
                    let active_request = state.in_flight.is_some();
                    finish_incomplete(&runtime, &mut state, "upstream_websocket");
                    if active_request {
                        send_gateway_error(&mut downstream, &GatewayFailure::transport()).await;
                    }
                    break;
                };
                if !handle_upstream_message(&mut downstream, &runtime, &mut state, message).await {
                    break;
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_downstream_message(
    downstream: &mut WebSocket,
    upstream: &mut UpstreamWebSocket,
    runtime: &GatewayRuntime,
    key: &AuthenticatedKey,
    headers: &HeaderMap,
    state: &mut BridgeState,
    message: Message,
) -> Result<bool, GatewayFailure> {
    match message {
        Message::Text(text) => {
            return start_next_request(
                downstream,
                upstream,
                runtime,
                key,
                headers,
                state,
                text.as_bytes(),
            )
            .await;
        }
        Message::Binary(bytes) => {
            return start_next_request(downstream, upstream, runtime, key, headers, state, &bytes)
                .await;
        }
        Message::Ping(payload) => {
            upstream
                .send(UpstreamMessage::Ping(payload))
                .await
                .map_err(|_| GatewayFailure::transport())?;
        }
        Message::Pong(payload) => {
            upstream
                .send(UpstreamMessage::Pong(payload))
                .await
                .map_err(|_| GatewayFailure::transport())?;
        }
        Message::Close(frame) => {
            let (code, reason) = frame
                .map(|frame| {
                    (
                        UpstreamCloseCode::from(frame.code),
                        frame.reason.to_string(),
                    )
                })
                .unwrap_or((UpstreamCloseCode::Normal, String::new()));
            let _ = upstream.send(UpstreamMessage::Close { code, reason }).await;
            let _ = downstream.send(Message::Close(None)).await;
            finish_incomplete(runtime, state, "client_cancelled");
            return Ok(false);
        }
    }
    Ok(true)
}

async fn start_next_request(
    downstream: &mut WebSocket,
    upstream: &mut UpstreamWebSocket,
    runtime: &GatewayRuntime,
    key: &AuthenticatedKey,
    headers: &HeaderMap,
    state: &mut BridgeState,
    payload: &[u8],
) -> Result<bool, GatewayFailure> {
    if state.in_flight.is_some() {
        return Err(GatewayFailure::invalid_request(
            "a response is already in progress",
        ));
    }
    let request = ClientRequest::parse(runtime, key, headers, payload)?;
    if request
        .previous_response_id()
        .is_some_and(|response_id| Some(response_id) == state.last_response_id.as_deref())
    {
        let tried = HashSet::new();
        let selected = runtime
            .select_and_reserve(
                key,
                &request.resolved_model,
                WEBSOCKET_PROTOCOLS,
                &tried,
                (
                    request.response_affinity_key.as_deref(),
                    request.prompt_affinity_key.as_deref(),
                ),
                now_ms(),
            )
            .await;
        let Some((selected, lease)) = selected else {
            if let Some(retry_at_ms) = runtime.earliest_retry_at(
                key,
                &request.resolved_model,
                WEBSOCKET_PROTOCOLS,
                &tried,
                request.response_affinity_key.as_deref(),
                now_ms(),
            ) {
                return Err(GatewayFailure::cooldown(retry_at_ms));
            }
            return Err(GatewayFailure::unavailable());
        };
        if selected.candidate_id != state.upstream_candidate_id {
            return Err(GatewayFailure::unavailable());
        }
        let mut route = runtime
            .executor_route(&selected.candidate_id, &request.resolved_model)
            .ok_or_else(GatewayFailure::unavailable)?;
        route.half_open_probe = selected.half_open_probe;
        route.routing = Some(selected.diagnostics);
        let started = Instant::now();
        if let Err(failure) = send_request(upstream, request.payload_for(&route)?).await {
            record_connect_failure(runtime, key, &route, &request, 1, started, &failure, None);
            return Err(failure);
        }
        let event = usage_event(
            &request.request_id,
            1,
            &key.id,
            &route,
            &request.requested_model,
            true,
            StatusCode::OK.as_u16(),
            None,
            0,
            request.tool_use_for(&route),
        );
        state.lease = Some(lease);
        state.in_flight = Some(InFlight {
            route: route.clone(),
            event,
            started,
            response_id: None,
            prompt_affinity_key: request.prompt_affinity_key,
        });
        return Ok(true);
    }
    let connected = connect_upstream(runtime, key, headers, request, true, 0).await?;
    let Connected {
        upstream: next_upstream,
        initial_messages,
        route,
        request,
        lease,
        attempt,
        started,
    } = connected;
    let event = usage_event(
        &request.request_id,
        attempt,
        &key.id,
        &route,
        &request.requested_model,
        true,
        StatusCode::OK.as_u16(),
        None,
        0,
        request.tool_use_for(&route),
    );
    let _ = upstream
        .send(UpstreamMessage::Close {
            code: UpstreamCloseCode::Normal,
            reason: String::new(),
        })
        .await;
    *upstream = next_upstream;
    state.lease = Some(lease);
    state.upstream_candidate_id = route.candidate_id.clone();
    state.last_response_id = None;
    state.in_flight = Some(InFlight {
        route,
        event,
        started,
        response_id: None,
        prompt_affinity_key: request.prompt_affinity_key,
    });
    for message in initial_messages {
        if !handle_upstream_message(downstream, runtime, state, message).await {
            return Ok(false);
        }
    }
    Ok(true)
}

async fn handle_upstream_message(
    downstream: &mut WebSocket,
    runtime: &GatewayRuntime,
    state: &mut BridgeState,
    message: UpstreamMessage,
) -> bool {
    match message {
        UpstreamMessage::Text(text) => {
            if text.len() > MAX_WEBSOCKET_MESSAGE_BYTES {
                finish_incomplete(runtime, state, "stream_event_too_large");
                return false;
            }
            let terminal = inspect_upstream_event(text.as_bytes(), state);
            if downstream.send(Message::Text(text.into())).await.is_err() {
                finish_incomplete(runtime, state, "client_cancelled");
                return false;
            }
            finish_terminal(runtime, state, terminal)
        }
        UpstreamMessage::Binary(bytes) => {
            if bytes.len() > MAX_WEBSOCKET_MESSAGE_BYTES {
                finish_incomplete(runtime, state, "stream_event_too_large");
                return false;
            }
            let terminal = inspect_upstream_event(&bytes, state);
            if downstream.send(Message::Binary(bytes)).await.is_err() {
                finish_incomplete(runtime, state, "client_cancelled");
                return false;
            }
            finish_terminal(runtime, state, terminal)
        }
        UpstreamMessage::Ping(payload) => downstream.send(Message::Ping(payload)).await.is_ok(),
        UpstreamMessage::Pong(payload) => downstream.send(Message::Pong(payload)).await.is_ok(),
        UpstreamMessage::Close { code, reason } => {
            let active_request = state.in_flight.is_some();
            finish_incomplete(runtime, state, "upstream_websocket_closed");
            if active_request {
                send_gateway_error(downstream, &GatewayFailure::closed()).await;
            } else {
                let _ = downstream
                    .send(Message::Close(Some(CloseFrame {
                        code: u16::from(code),
                        reason: reason.into(),
                    })))
                    .await;
            }
            false
        }
    }
}

#[derive(Default)]
struct EventTerminal {
    outcome: Option<bool>,
    status: Option<StatusCode>,
    error_category: Option<&'static str>,
    headers: HeaderMap,
    body_hint: RateLimitBodyHint,
    previous_response_not_found: bool,
}

fn inspect_upstream_event(payload: &[u8], state: &mut BridgeState) -> EventTerminal {
    let Ok(value) = serde_json::from_slice::<Value>(payload) else {
        return EventTerminal::default();
    };
    let event_type = value.get("type").and_then(Value::as_str);
    if let Some(in_flight) = state.in_flight.as_mut() {
        in_flight.event.tool_use.observe_stream_payload(&value);
        if has_output_delta(&value, event_type) && in_flight.event.ttft_ms.is_none() {
            in_flight.event.ttft_ms = Some(in_flight.started.elapsed().as_millis() as u64);
        }
        if let Some(usage) = super::response::find_usage(&value) {
            apply_usage(&mut in_flight.event, usage);
        }
        if let Some(response_id) = super::response::response_id(&value) {
            in_flight.response_id = Some(response_id.to_string());
        }
    }
    event_terminal(&value)
}

fn event_terminal(value: &Value) -> EventTerminal {
    let outcome = match value.get("type").and_then(Value::as_str) {
        Some("response.completed" | "response.done") => Some(true),
        Some(
            "response.failed"
            | "response.incomplete"
            | "response.cancelled"
            | "response.canceled"
            | "error",
        ) => Some(false),
        _ => None,
    };
    let status = super::errors::upstream_status_from_value(value);
    EventTerminal {
        outcome,
        status,
        error_category: super::errors::upstream_event_failure_category(
            value.get("type").and_then(Value::as_str),
            value,
        ),
        headers: websocket_retry_headers(value),
        body_hint: rate_limit_body_hint_value(value, std::time::SystemTime::now()),
        previous_response_not_found: super::errors::previous_response_not_found_value(value),
    }
}

fn finish_terminal(
    runtime: &GatewayRuntime,
    state: &mut BridgeState,
    terminal: EventTerminal,
) -> bool {
    let Some(success) = terminal.outcome else {
        return true;
    };
    let Some(mut in_flight) = state.in_flight.take() else {
        return true;
    };
    in_flight.event.latency_ms = in_flight.started.elapsed().as_millis() as u64;
    in_flight.event.generation_ms = in_flight
        .event
        .ttft_ms
        .map(|ttft_ms| in_flight.event.latency_ms.saturating_sub(ttft_ms))
        .filter(|duration| *duration > 0);
    in_flight.event.success = success;
    if success {
        in_flight.event.tool_use.finish();
    }
    runtime.observe_codex_quota_headers(
        &in_flight.route.candidate_id,
        terminal.status.unwrap_or(if success {
            StatusCode::OK
        } else {
            StatusCode::BAD_GATEWAY
        }),
        &terminal.headers,
        now_ms(),
    );
    if success {
        state.last_response_id = in_flight.response_id.clone();
        let recovered = runtime.record_success_with_metrics(
            &in_flight.route.candidate_id,
            &in_flight.route.source_model,
            now_ms(),
            in_flight.event.output_tokens,
            in_flight
                .event
                .generation_ms
                .unwrap_or(in_flight.event.latency_ms),
        );
        runtime.bind_response_affinity(
            in_flight.response_id.as_deref(),
            &in_flight.route.candidate_id,
            now_ms(),
        );
        runtime.bind_prompt_affinity(
            in_flight.prompt_affinity_key.as_deref(),
            &in_flight.route.candidate_id,
            now_ms(),
        );
        in_flight.event.consecutive_failures = recovered.then_some(0);
    } else {
        let category = terminal.error_category.unwrap_or_else(|| {
            super::errors::classify_upstream_error(terminal_failure_status(terminal.status), None)
                .category
        });
        let status = terminal
            .status
            .filter(|status| !status.is_success())
            .unwrap_or_else(|| super::errors::upstream_failure_status(category));
        let status = super::errors::canonical_upstream_status(status, category);
        in_flight.event.http_status = status.as_u16();
        in_flight.event.error_category = Some(category.to_string());
        if super::errors::retryable_failure(status, category, false) {
            let failure_state = apply_failure_cooldown_with_hint(
                runtime,
                &in_flight.route.candidate_id,
                &in_flight.route.source_model,
                status,
                category,
                &terminal.headers,
                terminal.body_hint,
                in_flight.route.half_open_probe,
            );
            apply_failure_state(&mut in_flight.event, failure_state);
        }
    }
    emit_usage(runtime, in_flight.event);
    state.lease.take();
    success
}

fn finish_incomplete(runtime: &GatewayRuntime, state: &mut BridgeState, category: &str) {
    let Some(mut in_flight) = state.in_flight.take() else {
        state.lease.take();
        return;
    };
    in_flight.event.success = false;
    in_flight.event.error_category = Some(category.to_string());
    in_flight.event.latency_ms = in_flight.started.elapsed().as_millis() as u64;
    if let Some(status) = incomplete_status(category) {
        in_flight.event.http_status = status.as_u16();
    }
    in_flight.event.generation_ms = in_flight
        .event
        .ttft_ms
        .map(|ttft_ms| in_flight.event.latency_ms.saturating_sub(ttft_ms))
        .filter(|duration| *duration > 0);
    if incomplete_requires_cooldown(category) {
        let failure_state = apply_cooldown(
            runtime,
            &in_flight.route.candidate_id,
            &in_flight.route.source_model,
            TRANSIENT_COOLDOWN_MS,
            in_flight.route.half_open_probe,
        );
        apply_failure_state(&mut in_flight.event, failure_state);
    }
    emit_usage(runtime, in_flight.event);
    state.lease.take();
}

fn incomplete_status(category: &str) -> Option<StatusCode> {
    match category {
        "websocket_idle_timeout" => Some(StatusCode::GATEWAY_TIMEOUT),
        "stream_semantic_timeout" => Some(StatusCode::GATEWAY_TIMEOUT),
        "stream_event_too_large"
        | "upstream_transport"
        | "upstream_websocket"
        | "upstream_websocket_closed" => Some(StatusCode::BAD_GATEWAY),
        _ => None,
    }
}

fn incomplete_requires_cooldown(category: &str) -> bool {
    matches!(
        category,
        "stream_event_too_large"
            | "upstream_transport"
            | "upstream_websocket"
            | "upstream_websocket_closed"
            | "websocket_idle_timeout"
            | "stream_semantic_timeout"
    )
}

fn terminal_failure_status(status: Option<StatusCode>) -> StatusCode {
    status
        .filter(|status| !status.is_success())
        .unwrap_or(StatusCode::BAD_GATEWAY)
}

fn websocket_retry_headers(value: &Value) -> HeaderMap {
    let mut headers = HeaderMap::new();
    let retry_after = websocket_reset_delay_seconds(value, now_ms() / 1_000)
        .map(|seconds| seconds.to_string())
        .or_else(|| {
            value
                .pointer("/headers/retry-after")
                .or_else(|| value.pointer("/headers/retry_after"))
                .or_else(|| value.pointer("/body/headers/retry-after"))
                .or_else(|| value.pointer("/body/error/resets_in_seconds"))
                .or_else(|| value.pointer("/error/resets_in_seconds"))
                .and_then(|value| match value {
                    Value::String(value) => Some(value.clone()),
                    Value::Number(value) => Some(value.to_string()),
                    _ => None,
                })
        });
    if let Some(value) = retry_after
        .filter(|value| value.len() <= 128)
        .and_then(|value| HeaderValue::from_str(&value).ok())
    {
        headers.insert(RETRY_AFTER, value);
    }
    for name in [
        "x-codex-primary-used-percent",
        "x-codex-primary-reset-after-seconds",
        "x-codex-primary-window-minutes",
        "x-codex-secondary-used-percent",
        "x-codex-secondary-reset-after-seconds",
        "x-codex-secondary-window-minutes",
    ] {
        if let Some(value) = websocket_header_value(value, name)
            .filter(|value| value.len() <= 128)
            .and_then(|value| HeaderValue::from_str(&value).ok())
        {
            headers.insert(HeaderName::from_static(name), value);
        }
    }
    headers
}

fn websocket_header_value(value: &Value, name: &str) -> Option<String> {
    let alternate = name.replace('-', "_");
    [
        value.get("headers"),
        value.pointer("/body/headers"),
        value.pointer("/response/headers"),
    ]
    .into_iter()
    .flatten()
    .find_map(|headers| headers.get(name).or_else(|| headers.get(&alternate)))
    .and_then(|value| match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    })
}

fn websocket_reset_delay_seconds(value: &Value, now_seconds: u64) -> Option<u64> {
    let reset_at = value
        .pointer("/body/error/resets_at")
        .or_else(|| value.pointer("/response/error/resets_at"))
        .or_else(|| value.pointer("/error/resets_at"))?;
    let mut reset_at = reset_at
        .as_u64()
        .or_else(|| reset_at.as_str().and_then(|value| value.parse().ok()))?;
    if reset_at > 10_000_000_000 {
        reset_at /= 1_000;
    }
    reset_at
        .checked_sub(now_seconds)
        .filter(|seconds| *seconds > 0)
}

fn metadata_flag(value: &Value, key: &str) -> bool {
    value
        .get("client_metadata")
        .and_then(|metadata| metadata.get(key))
        .is_some_and(|value| match value {
            Value::Bool(value) => *value,
            Value::String(value) => value.eq_ignore_ascii_case("true"),
            _ => false,
        })
}

async fn send_gateway_error(downstream: &mut WebSocket, failure: &GatewayFailure) {
    let event = json!({
        "type": "error",
        "status": failure.status.as_u16(),
        "error": {
            "type": super::errors::api_error_type(
                failure.status,
                super::errors::api_error_code(failure.category),
            ),
            "code": super::errors::api_error_code(failure.category),
            "message": failure.message,
            "param": null,
        },
        "retry_at_ms": failure.retry_at_ms,
    });
    let _ = downstream
        .send(Message::Text(event.to_string().into()))
        .await;
    let _ = downstream
        .send(Message::Close(Some(CloseFrame {
            code: if failure.status.is_client_error() {
                close_code::POLICY
            } else {
                close_code::ERROR
            },
            reason: "request failed".into(),
        })))
        .await;
}

struct GatewayFailure {
    status: StatusCode,
    category: &'static str,
    message: &'static str,
    cooldown_ms: u64,
    retry_at_ms: Option<u64>,
}

impl GatewayFailure {
    fn invalid_request(message: &'static str) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            category: "invalid_request",
            message,
            cooldown_ms: 0,
            retry_at_ms: None,
        }
    }

    fn adapter_unsupported() -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            category: "adapter_websocket_not_supported",
            message: "the selected source adapter does not support Responses WebSocket transport",
            cooldown_ms: 0,
            retry_at_ms: None,
        }
    }

    fn model_not_found() -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            category: "model_not_found",
            message: "model is not available for this local key",
            cooldown_ms: 0,
            retry_at_ms: None,
        }
    }

    fn request_timeout() -> Self {
        Self {
            status: StatusCode::REQUEST_TIMEOUT,
            category: "request_timeout",
            message: "response.create was not received in time",
            cooldown_ms: 0,
            retry_at_ms: None,
        }
    }

    fn client_closed() -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            category: "client_cancelled",
            message: "client closed the WebSocket connection",
            cooldown_ms: 0,
            retry_at_ms: None,
        }
    }

    fn prepare(error: ExecutorPrepareError) -> Self {
        let failure = super::errors::AttemptFailure::prepare(error);
        Self {
            status: failure.status,
            category: failure.category,
            message: failure.message,
            cooldown_ms: failure.cooldown_ms,
            retry_at_ms: None,
        }
    }

    fn transport() -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            category: "upstream_transport",
            message: "upstream WebSocket connection failed",
            cooldown_ms: TRANSIENT_COOLDOWN_MS,
            retry_at_ms: None,
        }
    }

    fn closed() -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            category: "upstream_websocket_closed",
            message: "upstream WebSocket closed before the response completed",
            cooldown_ms: TRANSIENT_COOLDOWN_MS,
            retry_at_ms: None,
        }
    }

    fn idle_timeout() -> Self {
        Self {
            status: StatusCode::GATEWAY_TIMEOUT,
            category: "websocket_idle_timeout",
            message: "upstream WebSocket produced no event before the idle timeout",
            cooldown_ms: TRANSIENT_COOLDOWN_MS,
            retry_at_ms: None,
        }
    }

    fn semantic_timeout() -> Self {
        Self {
            status: StatusCode::GATEWAY_TIMEOUT,
            category: "stream_semantic_timeout",
            message: "upstream produced no semantic output before the watchdog timeout",
            cooldown_ms: TRANSIENT_COOLDOWN_MS,
            retry_at_ms: None,
        }
    }

    fn bootstrap_too_large() -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            category: "stream_event_too_large",
            message: "upstream WebSocket bootstrap is too large",
            cooldown_ms: TRANSIENT_COOLDOWN_MS,
            retry_at_ms: None,
        }
    }

    fn upstream_status(status: StatusCode, body: Option<&[u8]>) -> Self {
        let classification = super::errors::classify_upstream_error(status, body);
        Self::classified(status, classification.category)
    }

    fn classified(status: StatusCode, category: &'static str) -> Self {
        Self {
            status: super::errors::canonical_upstream_status(status, category),
            category,
            message: super::errors::upstream_failure_message(category),
            cooldown_ms: TRANSIENT_COOLDOWN_MS,
            retry_at_ms: None,
        }
    }

    fn unavailable() -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            category: "no_eligible_source",
            message: "no eligible WebSocket source is available",
            cooldown_ms: 0,
            retry_at_ms: None,
        }
    }

    fn cooldown(retry_at_ms: u64) -> Self {
        Self {
            status: StatusCode::TOO_MANY_REQUESTS,
            category: "all_candidates_cooling_down",
            message: "all eligible sources are cooling down",
            cooldown_ms: 0,
            retry_at_ms: Some(retry_at_ms),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        incomplete_requires_cooldown, terminal_failure_status, websocket_reset_delay_seconds,
    };
    use reqwest::StatusCode;
    use serde_json::json;

    #[test]
    fn only_upstream_incomplete_failures_cool_the_candidate() {
        assert!(incomplete_requires_cooldown("upstream_websocket_closed"));
        assert!(incomplete_requires_cooldown("websocket_idle_timeout"));
        assert!(!incomplete_requires_cooldown("client_cancelled"));
        assert!(!incomplete_requires_cooldown("invalid_request"));
    }

    #[test]
    fn absolute_usage_reset_becomes_retry_delay() {
        let value = json!({
            "type": "error",
            "body": {"error": {"type": "usage_limit_reached", "resets_at": 1_700_000_120}}
        });
        assert_eq!(
            websocket_reset_delay_seconds(&value, 1_700_000_000),
            Some(120)
        );
    }

    #[test]
    fn terminal_errors_never_keep_a_success_status() {
        assert_eq!(
            terminal_failure_status(Some(StatusCode::OK)),
            StatusCode::BAD_GATEWAY
        );
        assert_eq!(
            terminal_failure_status(Some(StatusCode::TOO_MANY_REQUESTS)),
            StatusCode::TOO_MANY_REQUESTS
        );
    }

    #[test]
    fn upstream_status_accepts_string_status_codes() {
        let value = serde_json::json!({"type": "error", "status": "429"});
        assert_eq!(
            crate::gateway::errors::upstream_status_from_value(&value),
            Some(StatusCode::TOO_MANY_REQUESTS)
        );
    }
}
