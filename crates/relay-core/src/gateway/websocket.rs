use super::auth::{client_api_forbidden, invalid_host, unauthorized};
use super::errors::{
    apply_cooldown, apply_failure_cooldown_with_hint, apply_failure_state, rate_limit_body_hint,
    CooldownContext, RateLimitBodyHint, TRANSIENT_COOLDOWN_MS,
};
use super::execution::execute_client_request;
use super::now_ms;
use super::request::{
    client_context_fingerprint, forwarded_codex_headers, CODEX_RESPONSES_LITE_HEADER,
};
use super::response::{apply_usage, emit_usage, route_error_origin, usage_event};
use super::streaming::{has_output_delta, parse_sse_event};
use super::turn_state::{
    guard_account_request, note_account_response_header, CODEX_TURN_STATE_HEADER,
};
use crate::protocol::ClientWireApi;
use crate::runtime::{AuthenticatedKey, CandidateLease, ExecutorPrepareError, ExecutorRoute};
use crate::{ErrorOrigin, GatewayRuntime, UsageEvent, WireApi};
use axum::body::Body;
use axum::extract::ws::{close_code, CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::header::AUTHORIZATION;
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, Request, Response, StatusCode};
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

mod events;
mod failure;
mod request;

use events::{
    event_terminal, incomplete_requires_cooldown, incomplete_status, terminal_failure_status,
    EventTerminal, EventTerminalOutcome,
};
use failure::{send_gateway_error, GatewayFailure};
use request::ClientRequest;

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
            send_gateway_error(&mut downstream, &failure, None).await;
            return;
        }
    };

    let request_id = request.request_id.clone();
    if let Some(kind) = request.background_kind {
        if !runtime.codex_background_tasks_enabled() {
            runtime.blocked_codex_background_event(
                &request.request_id,
                &key.id,
                &request.requested_model,
                WireApi::Responses,
                kind,
            );
            let payload = serde_json::json!({
                "type": "response.completed",
                "response": {"id": format!("resp_relay_blocked_{}", request.request_id), "object": "response", "status": "completed", "output": [], "usage": {"input_tokens": 0, "output_tokens": 0, "total_tokens": 0}, "metadata": {"zenith_relay": {"blocked": true, "request_type": kind}}}
            });
            let _ = downstream
                .send(Message::Text(payload.to_string().into()))
                .await;
            let _ = downstream.send(Message::Close(None)).await;
            return;
        }
    }
    let fallback_request = request.clone();
    if !runtime.codex_websockets_enabled() {
        bridge_http_fallback(downstream, runtime, key, headers, fallback_request).await;
        return;
    }
    let connected = match connect_upstream(&runtime, &key, &headers, request, true, 0).await {
        Ok(connected) => connected,
        Err(failure) if failure.category == "upstream_websocket_unsupported" => {
            bridge_http_fallback(downstream, runtime, key, headers, fallback_request).await;
            return;
        }
        Err(failure) => {
            send_gateway_error(&mut downstream, &failure, Some(&request_id)).await;
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

/// Keeps the client-facing Responses WebSocket usable when every selected
/// provider only exposes HTTP/SSE. The normal HTTP executor remains the single
/// source of routing, adapters, retries, usage, and quota accounting.
async fn bridge_http_fallback(
    mut downstream: WebSocket,
    runtime: Arc<GatewayRuntime>,
    key: AuthenticatedKey,
    headers: HeaderMap,
    mut request: ClientRequest,
) {
    let mut stream_id = request.stream_id.clone();
    loop {
        if let Err(failure) =
            serve_http_fallback_request(&mut downstream, runtime.clone(), &key, &headers, &request)
                .await
        {
            let request_id = Some(request.request_id.as_str());
            send_gateway_error(&mut downstream, &failure, request_id).await;
            return;
        }

        let next = timeout(WEBSOCKET_IDLE_TIMEOUT, async {
            loop {
                let message = downstream.recv().await?;
                let Ok(message) = message else {
                    return None;
                };
                match message {
                    Message::Text(text) => return Some(Some(text.to_string().into_bytes())),
                    Message::Binary(bytes) => return Some(Some(bytes.to_vec())),
                    Message::Ping(payload) => {
                        if downstream.send(Message::Pong(payload)).await.is_err() {
                            return None;
                        }
                    }
                    Message::Pong(_) => {}
                    Message::Close(frame) => {
                        let _ = downstream.send(Message::Close(frame)).await;
                        return None;
                    }
                }
            }
        })
        .await;
        let Ok(Some(Some(payload))) = next else {
            return;
        };
        let next_request = match ClientRequest::parse(&runtime, &key, &headers, &payload) {
            Ok(request) => request,
            Err(failure) => {
                send_gateway_error(&mut downstream, &failure, None).await;
                return;
            }
        };
        if let Some(next_stream_id) = next_request.stream_id.as_deref() {
            if let Some(expected) = stream_id.as_deref() {
                if expected != next_stream_id {
                    let failure = GatewayFailure::invalid_request(
                        "only one WebSocket stream_id is supported per connection",
                    );
                    send_gateway_error(&mut downstream, &failure, Some(&next_request.request_id))
                        .await;
                    return;
                }
            } else {
                stream_id = Some(next_stream_id.to_string());
            }
        }
        request = next_request;
    }
}

async fn serve_http_fallback_request(
    downstream: &mut WebSocket,
    runtime: Arc<GatewayRuntime>,
    _key: &AuthenticatedKey,
    client_headers: &HeaderMap,
    request: &ClientRequest,
) -> Result<(), GatewayFailure> {
    let mut headers = client_headers.clone();
    for name in [
        "connection",
        "upgrade",
        "sec-websocket-key",
        "sec-websocket-version",
        "sec-websocket-protocol",
        "content-length",
    ] {
        headers.remove(name);
    }
    let http_request = Request::builder()
        .method(Method::POST)
        .uri("/v1/responses")
        .header("host", "localhost")
        .body(Body::from(request.http_payload()?))
        .map_err(|_| GatewayFailure::invalid_request("request could not be serialized"))
        .map(|mut request| {
            *request.headers_mut() = headers;
            request
        })?;
    let response = execute_client_request(runtime, http_request, WireApi::Responses).await;
    if !response.status().is_success() {
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), MAX_WEBSOCKET_ERROR_BYTES)
            .await
            .ok();
        return Err(GatewayFailure::upstream_status(
            status,
            body.as_deref(),
            ErrorOrigin::Relay,
        ));
    }

    let mut body = response.into_body().into_data_stream();
    let mut pending = Vec::new();
    while let Some(chunk) = body.next().await {
        let chunk = chunk.map_err(|_| GatewayFailure::transport(ErrorOrigin::Relay))?;
        if pending.len().saturating_add(chunk.len()) > MAX_WEBSOCKET_ERROR_BYTES {
            return Err(GatewayFailure::message_too_large(ErrorOrigin::Relay));
        }
        pending.extend_from_slice(&chunk);
        while let Some(end) = crate::protocol::sse_event_end(&pending) {
            let event = pending.drain(..end).collect::<Vec<_>>();
            let terminal = parse_sse_event(&event);
            if terminal.has_data && !terminal.valid {
                return Err(GatewayFailure::transport(ErrorOrigin::Relay));
            }
            if let Some(payload) = terminal.payload {
                let payload = serde_json::to_string(&payload)
                    .map_err(|_| GatewayFailure::transport(ErrorOrigin::Relay))?;
                if payload.len() > MAX_WEBSOCKET_MESSAGE_BYTES {
                    return Err(GatewayFailure::message_too_large(ErrorOrigin::Relay));
                }
                downstream
                    .send(Message::Text(payload.into()))
                    .await
                    .map_err(|_| GatewayFailure::client_closed())?;
            }
            if terminal.outcome.is_some() {
                return Ok(());
            }
        }
    }
    Err(GatewayFailure::closed(ErrorOrigin::Relay))
}

fn websocket_transport_fallback_status(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::BAD_REQUEST
            | StatusCode::NOT_FOUND
            | StatusCode::METHOD_NOT_ALLOWED
            | StatusCode::UPGRADE_REQUIRED
            | StatusCode::NOT_IMPLEMENTED
    )
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
    let mut function_item_id_repair_attempted = false;
    let mut custom_tool_item_id_repair_attempted = false;
    let mut message_item_id_repair_attempted = false;
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
        let Some(mut route) = runtime.executor_route(
            &selected.candidate_id,
            &request.resolved_model,
            &key.scope_snapshot(),
            WEBSOCKET_PROTOCOLS,
            false,
        ) else {
            continue;
        };
        request.apply_service_tier_for_route(runtime, &route);
        route.service_tier = request.service_tier();
        route.half_open_probe = selected.half_open_probe;
        route.routing = Some(selected.diagnostics);
        route.client_context_id = client_context_fingerprint(client_headers);
        let source_error_origin = route_error_origin(&route);
        if route.wire_api != WireApi::Responses {
            continue;
        }
        if !route.adapter.is_passthrough() {
            runtime.mark_websocket_http_only(
                &route.candidate_id,
                &request.resolved_model,
                now_ms(),
            );
            drop(lease);
            last_failure = Some(GatewayFailure::websocket_http_fallback(source_error_origin));
            continue;
        }
        if runtime.websocket_is_http_only(&route.candidate_id, &request.resolved_model, now_ms()) {
            drop(lease);
            last_failure = Some(GatewayFailure::websocket_http_fallback(source_error_origin));
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
                let failure = GatewayFailure::prepare(error, source_error_origin);
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
            let mut headers = upstream_headers(
                client_headers,
                &prepared,
                request.responses_lite_for(&route),
                &request.request_id,
            );
            if let Some(account_id) = route.account_id.as_deref() {
                guard_account_request(runtime, &key.id, &mut headers, account_id, now_ms());
            } else {
                headers.remove(CODEX_TURN_STATE_HEADER);
            }
            let upgrade = runtime
                .websocket_client(&route.candidate_id)
                .get(route.upstream_url.clone())
                .headers(headers)
                .upgrade();
            let Ok(Ok(upgrade)) = timeout(UPSTREAM_CONNECT_TIMEOUT, upgrade.send()).await else {
                let failure = GatewayFailure::transport(source_error_origin);
                record_connect_failure(
                    runtime, key, &route, &request, attempt, started, &failure, None,
                );
                last_failure = Some(failure);
                continue 'candidates;
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
                    let failure = GatewayFailure::prepare(error, source_error_origin);
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
        if status == StatusCode::SWITCHING_PROTOCOLS {
            if let Some(account_id) = route.account_id.as_deref() {
                note_account_response_header(
                    runtime,
                    &key.id,
                    client_headers,
                    account_id,
                    &response_headers,
                    now_ms(),
                );
            }
        }
        if status != StatusCode::SWITCHING_PROTOCOLS {
            let response = upgrade.into_inner();
            let body = timeout(
                UPSTREAM_CONNECT_TIMEOUT,
                crate::transport::collect_limited(response, MAX_WEBSOCKET_ERROR_BYTES),
            )
            .await
            .ok()
            .and_then(Result::ok);
            let failure =
                GatewayFailure::upstream_status(status, body.as_deref(), source_error_origin);
            if websocket_transport_fallback_status(status) {
                runtime.mark_websocket_http_only(
                    &route.candidate_id,
                    &request.resolved_model,
                    now_ms(),
                );
                last_failure = Some(GatewayFailure::websocket_http_fallback(source_error_origin));
                continue 'candidates;
            }
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
                last_failure = Some(failure);
                continue;
            }
            record_connect_rejection(runtime, key, &route, &request, attempt, started, &failure);
            return Err(failure);
        }
        let Ok(Ok(mut upstream)) =
            timeout(UPSTREAM_CONNECT_TIMEOUT, upgrade.into_websocket()).await
        else {
            let failure = GatewayFailure::transport(source_error_origin);
            record_connect_failure(
                runtime, key, &route, &request, attempt, started, &failure, None,
            );
            last_failure = Some(failure);
            continue;
        };
        runtime.mark_websocket_supported(&route.candidate_id, &request.resolved_model);
        if send_request(&mut upstream, payload, source_error_origin)
            .await
            .is_err()
        {
            let failure = GatewayFailure::transport(source_error_origin);
            record_connect_failure(
                runtime, key, &route, &request, attempt, started, &failure, None,
            );
            last_failure = Some(failure);
            continue;
        }
        let initial_messages =
            match initial_application_messages(&mut upstream, source_error_origin).await {
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
                    last_failure = Some(failure);
                    continue;
                }
            };
        if let Some(terminal) = initial_messages.last().and_then(first_message_terminal) {
            if terminal.outcome == Some(EventTerminalOutcome::Failure) {
                let terminal_body = initial_messages.last().and_then(|message| match message {
                    UpstreamMessage::Text(text) => Some(text.as_bytes()),
                    UpstreamMessage::Binary(bytes) => Some(bytes.as_ref()),
                    _ => None,
                });
                if !function_item_id_repair_attempted
                    && terminal_body
                        .is_some_and(super::errors::responses_function_item_id_requires_fc_prefix)
                    && request.repair_function_item_ids()
                {
                    function_item_id_repair_attempted = true;
                    tried.remove(&route.candidate_id);
                    last_failure = None;
                    continue;
                }
                if !custom_tool_item_id_repair_attempted
                    && terminal_body.is_some_and(
                        super::errors::responses_custom_tool_item_id_requires_ctc_prefix,
                    )
                    && request.repair_custom_tool_item_ids()
                {
                    custom_tool_item_id_repair_attempted = true;
                    tried.remove(&route.candidate_id);
                    last_failure = None;
                    continue;
                }
                if !message_item_id_repair_attempted
                    && terminal_body
                        .is_some_and(super::errors::responses_message_item_id_requires_msg_prefix)
                    && request.repair_message_item_ids()
                {
                    message_item_id_repair_attempted = true;
                    tried.remove(&route.candidate_id);
                    last_failure = None;
                    continue;
                }
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
                    let failure = GatewayFailure::classified(status, category, source_error_origin);
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
                    let failure = GatewayFailure::classified(status, category, source_error_origin);
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
    origin: ErrorOrigin,
) -> Result<Vec<UpstreamMessage>, GatewayFailure> {
    let mut messages = Vec::new();
    let mut buffered_bytes = 0_usize;
    loop {
        let message = first_application_message(upstream, origin).await?;
        let message_bytes = match &message {
            UpstreamMessage::Text(text) => text.len(),
            UpstreamMessage::Binary(bytes) => bytes.len(),
            _ => 0,
        };
        buffered_bytes = buffered_bytes.saturating_add(message_bytes);
        if buffered_bytes > MAX_WEBSOCKET_MESSAGE_BYTES.saturating_mul(2) {
            return Err(GatewayFailure::message_too_large(origin));
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
    origin: ErrorOrigin,
) -> Result<UpstreamMessage, GatewayFailure> {
    let deadline = TokioInstant::now() + INITIAL_MESSAGE_TIMEOUT;
    let mut heartbeat = interval_at(
        TokioInstant::now() + WEBSOCKET_HEARTBEAT_INTERVAL,
        WEBSOCKET_HEARTBEAT_INTERVAL,
    );
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = sleep_until(deadline) => return Err(GatewayFailure::idle_timeout(ErrorOrigin::Relay)),
            _ = heartbeat.tick() => {
                upstream
                    .send(UpstreamMessage::Ping(Default::default()))
                    .await
                    .map_err(|_| GatewayFailure::transport(origin))?;
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
                            .map_err(|_| GatewayFailure::transport(origin))?;
                    }
                    Some(Ok(UpstreamMessage::Pong(_))) => {}
                    Some(Ok(UpstreamMessage::Close { .. })) | None => {
                        return Err(GatewayFailure::closed(origin));
                    }
                    Some(Err(_)) => return Err(GatewayFailure::transport(origin)),
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
    let cooldown_context = CooldownContext {
        scope: &route.scope,
        allowed_protocols: &route.allowed_protocols,
    };
    let state = match headers {
        Some(headers) => apply_failure_cooldown_with_hint(
            runtime,
            &route.candidate_id,
            &route.source_model,
            failure.status,
            failure.category,
            headers,
            hint,
            &cooldown_context,
            route.half_open_probe,
        ),
        None => apply_failure_cooldown_with_hint(
            runtime,
            &route.candidate_id,
            &route.source_model,
            failure.status,
            failure.category,
            &HeaderMap::new(),
            hint,
            &cooldown_context,
            route.half_open_probe,
        ),
    };
    let mut event = usage_event(
        &request.request_id,
        attempt,
        &key.id,
        route,
        Some(&request.reasoning_effort_for(route)),
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
            Some(&request.reasoning_effort_for(route)),
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
            Some(&request.reasoning_effort_for(route)),
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
    origin: ErrorOrigin,
) -> Result<(), GatewayFailure> {
    let send = async {
        upstream.send(UpstreamMessage::Text(payload)).await?;
        upstream.flush().await
    };
    match timeout(UPSTREAM_CONNECT_TIMEOUT, send).await {
        Ok(Ok(())) => Ok(()),
        _ => Err(GatewayFailure::transport(origin)),
    }
}

struct InFlight {
    request: ClientRequest,
    route: ExecutorRoute,
    event: UsageEvent,
    started: Instant,
    response_id: Option<String>,
    prompt_affinity_key: Option<String>,
}

struct BridgeState {
    lease: Option<CandidateLease>,
    in_flight: Option<InFlight>,
    stream_id: Option<String>,
    upstream_candidate_id: String,
    upstream_origin: ErrorOrigin,
    last_response_id: Option<String>,
}

impl BridgeState {
    fn request_id(&self) -> Option<&str> {
        self.in_flight
            .as_ref()
            .map(|in_flight| in_flight.event.request_id.as_str())
    }
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
        Some(&connected.request.reasoning_effort_for(&connected.route)),
        &connected.request.requested_model,
        true,
        StatusCode::OK.as_u16(),
        None,
        0,
        connected.request.tool_use_for(&connected.route),
    );
    let upstream_candidate_id = connected.route.candidate_id.clone();
    let upstream_origin = route_error_origin(&connected.route);
    let prompt_affinity_key = connected.request.prompt_affinity_key.clone();
    let mut state = BridgeState {
        lease: Some(connected.lease),
        in_flight: Some(InFlight {
            request: connected.request.clone(),
            route: connected.route,
            event: initial_event,
            started: connected.started,
            response_id: None,
            prompt_affinity_key,
        }),
        stream_id: connected.request.stream_id.clone(),
        upstream_candidate_id,
        upstream_origin,
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
                let request_id = state.request_id().map(str::to_owned);
                finish_incomplete(&runtime, &mut state, "stream_semantic_timeout");
                send_gateway_error(
                    &mut downstream,
                    &GatewayFailure::semantic_timeout(ErrorOrigin::Relay),
                    request_id.as_deref(),
                ).await;
                break;
            }
            _ = sleep_until(idle_deadline) => {
                let active_request = state.in_flight.is_some();
                let request_id = state.request_id().map(str::to_owned);
                finish_incomplete(&runtime, &mut state, "websocket_idle_timeout");
                if active_request {
                    send_gateway_error(
                        &mut downstream,
                        &GatewayFailure::idle_timeout(ErrorOrigin::Relay),
                        request_id.as_deref(),
                    ).await;
                } else {
                    let _ = downstream.send(Message::Close(Some(CloseFrame {
                        code: close_code::AWAY,
                        reason: "idle timeout".into(),
                    }))).await;
                }
                break;
            }
            _ = heartbeat.tick() => {
                if upstream.send(UpstreamMessage::Ping(Default::default())).await.is_err() {
                    let active_request = state.in_flight.is_some();
                    let request_id = state.request_id().map(str::to_owned);
                    finish_incomplete(&runtime, &mut state, "upstream_websocket");
                    if active_request {
                        send_gateway_error(
                            &mut downstream,
                            &GatewayFailure::transport(state.upstream_origin),
                            request_id.as_deref(),
                        ).await;
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
                        let request_id = state.request_id().map(str::to_owned);
                        finish_incomplete(&runtime, &mut state, failure.category);
                        send_gateway_error(&mut downstream, &failure, request_id.as_deref()).await;
                        break;
                    }
                }
            }
            message = upstream.next() => {
                last_activity = TokioInstant::now();
                let (message, failure) = match message {
                    Some(Ok(message @ (UpstreamMessage::Text(_)
                        | UpstreamMessage::Binary(_)
                        | UpstreamMessage::Ping(_)
                        | UpstreamMessage::Pong(_)))) => (Some(message), None),
                    Some(Ok(UpstreamMessage::Close { .. })) | None => {
                        (None, Some(GatewayFailure::closed(state.upstream_origin)))
                    }
                    Some(Err(_)) => {
                        (None, Some(GatewayFailure::transport(state.upstream_origin)))
                    }
                };
                let Some(message) = message else {
                    let category = failure
                        .as_ref()
                        .map(|failure| failure.category)
                        .unwrap_or("upstream_websocket_closed");
                    let request_id = state.request_id().map(str::to_owned);
                    if let Some(request) = retryable_disconnect_request(&state) {
                        let attempt_offset = state
                            .in_flight
                            .as_ref()
                            .map(|in_flight| in_flight.event.attempt)
                            .unwrap_or_default();
                        finish_incomplete(&runtime, &mut state, category);
                        match connect_upstream(
                            &runtime,
                            &key,
                            &headers,
                            request,
                            true,
                            attempt_offset,
                        )
                        .await
                        {
                            Ok(connected) => {
                                if !install_connected(
                                    &mut downstream,
                                    &mut upstream,
                                    &runtime,
                                    &key,
                                    &mut state,
                                    connected,
                                )
                                .await
                                {
                                    break;
                                }
                                continue;
                            }
                            Err(retry_failure) => {
                                send_gateway_error(
                                    &mut downstream,
                                    &retry_failure,
                                    request_id.as_deref(),
                                )
                                .await;
                                break;
                            }
                        }
                    }
                    let active_request = state.in_flight.is_some();
                    finish_incomplete(&runtime, &mut state, category);
                    if active_request {
                        if let Some(failure) = failure {
                            send_gateway_error(
                                &mut downstream,
                                &failure,
                                request_id.as_deref(),
                            )
                            .await;
                        }
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

fn retryable_disconnect_request(state: &BridgeState) -> Option<ClientRequest> {
    let in_flight = state.in_flight.as_ref()?;
    if in_flight.request.has_previous_response_id()
        || in_flight.request.has_tool_call_output()
        || in_flight.event.ttft_ms.is_some()
        || in_flight.event.output_tokens.is_some()
        || in_flight.event.tool_use.tool_call_count > 0
        || in_flight.event.tool_use.text_output
    {
        return None;
    }
    Some(in_flight.request.clone())
}

async fn install_connected(
    downstream: &mut WebSocket,
    upstream: &mut UpstreamWebSocket,
    runtime: &GatewayRuntime,
    key: &AuthenticatedKey,
    state: &mut BridgeState,
    connected: Connected,
) -> bool {
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
        Some(&request.reasoning_effort_for(&route)),
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
    state.upstream_origin = route_error_origin(&route);
    state.last_response_id = None;
    state.in_flight = Some(InFlight {
        request: request.clone(),
        route,
        event,
        started,
        response_id: None,
        prompt_affinity_key: request.prompt_affinity_key,
    });
    for message in initial_messages {
        if !handle_upstream_message(downstream, runtime, state, message).await {
            return false;
        }
    }
    true
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
                .map_err(|_| GatewayFailure::transport(state.upstream_origin))?;
        }
        Message::Pong(payload) => {
            upstream
                .send(UpstreamMessage::Pong(payload))
                .await
                .map_err(|_| GatewayFailure::transport(state.upstream_origin))?;
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
    let mut request = ClientRequest::parse(runtime, key, headers, payload)?;
    if let Some(stream_id) = request.stream_id.as_deref() {
        if let Some(active_stream_id) = state.stream_id.as_deref() {
            if active_stream_id != stream_id {
                return Err(GatewayFailure::invalid_request(
                    "only one WebSocket stream_id is supported per connection",
                ));
            }
        } else {
            state.stream_id = Some(stream_id.to_string());
        }
    }
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
            .executor_route(
                &selected.candidate_id,
                &request.resolved_model,
                &key.scope_snapshot(),
                WEBSOCKET_PROTOCOLS,
                false,
            )
            .ok_or_else(GatewayFailure::unavailable)?;
        route.half_open_probe = selected.half_open_probe;
        route.routing = Some(selected.diagnostics);
        route.client_context_id = client_context_fingerprint(headers);
        request.apply_service_tier_for_route(runtime, &route);
        route.service_tier = request.service_tier();
        let started = Instant::now();
        let upstream_origin = route_error_origin(&route);
        if let Err(failure) =
            send_request(upstream, request.payload_for(&route)?, upstream_origin).await
        {
            record_connect_failure(runtime, key, &route, &request, 1, started, &failure, None);
            return Err(failure);
        }
        let event = usage_event(
            &request.request_id,
            1,
            &key.id,
            &route,
            Some(&request.reasoning_effort_for(&route)),
            &request.requested_model,
            true,
            StatusCode::OK.as_u16(),
            None,
            0,
            request.tool_use_for(&route),
        );
        state.lease = Some(lease);
        state.upstream_origin = upstream_origin;
        state.in_flight = Some(InFlight {
            request: request.clone(),
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
        Some(&request.reasoning_effort_for(&route)),
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
    state.upstream_origin = route_error_origin(&route);
    state.last_response_id = None;
    state.in_flight = Some(InFlight {
        request: request.clone(),
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
                let request_id = state.request_id().map(str::to_owned);
                finish_incomplete(runtime, state, "stream_event_too_large");
                send_gateway_error(
                    downstream,
                    &GatewayFailure::message_too_large(state.upstream_origin),
                    request_id.as_deref(),
                )
                .await;
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
                let request_id = state.request_id().map(str::to_owned);
                finish_incomplete(runtime, state, "stream_event_too_large");
                send_gateway_error(
                    downstream,
                    &GatewayFailure::message_too_large(state.upstream_origin),
                    request_id.as_deref(),
                )
                .await;
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
            let request_id = state.request_id().map(str::to_owned);
            finish_incomplete(runtime, state, "upstream_websocket_closed");
            if active_request {
                send_gateway_error(
                    downstream,
                    &GatewayFailure::closed(state.upstream_origin),
                    request_id.as_deref(),
                )
                .await;
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

fn finish_terminal(
    runtime: &GatewayRuntime,
    state: &mut BridgeState,
    terminal: EventTerminal,
) -> bool {
    let Some(outcome) = terminal.outcome else {
        return true;
    };
    let Some(mut in_flight) = state.in_flight.take() else {
        return true;
    };
    let delivered = matches!(
        outcome,
        EventTerminalOutcome::Success | EventTerminalOutcome::Incomplete
    );
    let success = matches!(outcome, EventTerminalOutcome::Success);
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
    if matches!(outcome, EventTerminalOutcome::Incomplete) {
        in_flight.event.error_category = Some("response_incomplete".to_string());
    }
    if terminal.deactivated_workspace
        && terminal.status == Some(StatusCode::PAYMENT_REQUIRED)
        && in_flight.route.account_id.is_some()
    {
        runtime.trip_chatgpt_team_breaker(&in_flight.route.candidate_id, now_ms());
    }
    runtime.observe_codex_quota_headers(
        &in_flight.route.candidate_id,
        match outcome {
            EventTerminalOutcome::Success => terminal.status.unwrap_or(StatusCode::OK),
            EventTerminalOutcome::Incomplete => StatusCode::OK,
            EventTerminalOutcome::Failure => terminal.status.unwrap_or(StatusCode::BAD_GATEWAY),
        },
        &terminal.headers,
        now_ms(),
    );
    if delivered {
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
            let cooldown_context = CooldownContext {
                scope: &in_flight.route.scope,
                allowed_protocols: &in_flight.route.allowed_protocols,
            };
            let failure_state = apply_failure_cooldown_with_hint(
                runtime,
                &in_flight.route.candidate_id,
                &in_flight.route.source_model,
                status,
                category,
                &terminal.headers,
                terminal.body_hint,
                &cooldown_context,
                in_flight.route.half_open_probe,
            );
            apply_failure_state(&mut in_flight.event, failure_state);
        }
    }
    emit_usage(runtime, in_flight.event);
    state.lease.take();
    delivered
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
    // A direct API source can serve independent requests concurrently. A
    // failed WebSocket stream must not cool the whole source and make an
    // unrelated response-affinity continuation fail with 409.
    if incomplete_requires_cooldown(category) && in_flight.route.account_id.is_some() {
        let cooldown_context = CooldownContext {
            scope: &in_flight.route.scope,
            allowed_protocols: &in_flight.route.allowed_protocols,
        };
        let failure_state = apply_cooldown(
            runtime,
            &in_flight.route.candidate_id,
            &in_flight.route.source_model,
            TRANSIENT_COOLDOWN_MS,
            &cooldown_context,
            in_flight.route.half_open_probe,
        );
        apply_failure_state(&mut in_flight.event, failure_state);
    }
    emit_usage(runtime, in_flight.event);
    state.lease.take();
}

#[cfg(test)]
mod tests {
    use super::events::{websocket_reset_delay_seconds, websocket_retry_headers};
    use super::{
        event_terminal, incomplete_requires_cooldown, terminal_failure_status, ClientRequest,
        EventTerminalOutcome, GatewayFailure, WEBSOCKET_PROTOCOLS,
    };
    use crate::{
        ErrorOrigin, GatewayRuntime, GatewayRuntimeOptions, LocalGatewayKey, ProviderSource,
        RuntimeLocalKey, RuntimeSource, WireApi,
    };
    use axum::http::{HeaderMap, HeaderValue, StatusCode};
    use serde_json::json;
    use std::sync::Arc;

    fn runtime() -> GatewayRuntime {
        GatewayRuntime::from_pool(
            vec![RuntimeSource::unrestricted(ProviderSource {
                id: "source".into(),
                name: "source".into(),
                base_url: "https://example.test/v1".into(),
                api_key: "upstream-secret".into(),
                wire_api: WireApi::Responses,
                models: vec!["upstream-model".into()],
            })],
            vec![RuntimeLocalKey {
                key: LocalGatewayKey {
                    id: "key".into(),
                    secret: "local-secret".into(),
                },
                enabled: true,
                source_ids: None,
                allowed_models: Vec::new(),
                excluded_models: Vec::new(),
                model_prefix: Some("relay".into()),
            }],
            GatewayRuntimeOptions::default(),
            Arc::new(|_| {}),
        )
        .unwrap()
    }

    #[test]
    fn only_upstream_incomplete_failures_cool_the_candidate() {
        assert!(incomplete_requires_cooldown("upstream_websocket_closed"));
        assert!(incomplete_requires_cooldown("websocket_idle_timeout"));
        assert!(!incomplete_requires_cooldown("client_cancelled"));
        assert!(!incomplete_requires_cooldown("invalid_request"));
    }

    #[test]
    fn response_incomplete_is_delivered_without_failure_outcome() {
        let terminal = event_terminal(&json!({
            "type": "response.incomplete",
            "response": {
                "id": "resp_incomplete",
                "status": "incomplete",
                "incomplete_details": {"reason": "max_output_tokens"}
            }
        }));

        assert_eq!(terminal.outcome, Some(EventTerminalOutcome::Incomplete));
        assert_eq!(terminal.error_category, Some("response_incomplete"));
        assert!(!incomplete_requires_cooldown("response_incomplete"));
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

    #[test]
    fn websocket_retry_headers_preserve_nested_reset_and_quota_hints() {
        let value = json!({
            "body": {
                "error": {"resets_in_seconds": 45},
                "headers": {"x-codex-primary-used-percent": "99"}
            }
        });
        let headers = websocket_retry_headers(&value);

        assert_eq!(
            headers
                .get("retry-after")
                .and_then(|value| value.to_str().ok()),
            Some("45")
        );
        assert_eq!(
            headers
                .get("x-codex-primary-used-percent")
                .and_then(|value| value.to_str().ok()),
            Some("99")
        );
    }

    #[test]
    fn websocket_cooldown_failure_keeps_retry_metadata() {
        let failure = GatewayFailure::cooldown(1_700_000_120_000);

        assert_eq!(failure.status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(failure.category, "all_candidates_cooling_down");
        assert_eq!(failure.retry_at_ms, Some(1_700_000_120_000));
        assert_eq!(failure.origin, ErrorOrigin::Relay);
    }

    #[test]
    fn websocket_errors_keep_the_source_origin_and_unmapped_category() {
        let failure = GatewayFailure::classified(
            StatusCode::BAD_REQUEST,
            "upstream_invalid_request",
            ErrorOrigin::Account,
        );
        let event = super::failure::gateway_error_event(&failure, Some("relay-request-3"));

        assert_eq!(event["error"]["code"], "invalid_request");
        assert_eq!(
            event["error"]["zenith_relay"]["category"],
            "upstream_invalid_request"
        );
        assert_eq!(event["error"]["zenith_relay"]["origin"], "account");
        assert_eq!(
            event["error"]["zenith_relay"]["request_id"],
            "relay-request-3"
        );
    }

    #[test]
    fn client_request_resolves_the_visible_model_before_upstream_serialization() {
        let runtime = runtime();
        let key = runtime
            .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
            .unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            crate::gateway::request::CODEX_RESPONSES_LITE_HEADER,
            HeaderValue::from_static("true"),
        );
        let request = ClientRequest::parse(
            &runtime,
            &key,
            &headers,
            br#"{
                "model": "relay/upstream-model",
                "input": "hello",
                "previous_response_id": "resp_previous",
                "prompt_cache_key": "cache-key"
            }"#,
        )
        .unwrap_or_else(|_| panic!("request should be accepted"));
        let route = runtime
            .executor_route(
                "source",
                &request.resolved_model,
                &key.scope_snapshot(),
                WEBSOCKET_PROTOCOLS,
                false,
            )
            .unwrap();
        let payload: serde_json::Value = serde_json::from_str(
            &request
                .payload_for(&route)
                .unwrap_or_else(|_| panic!("request should be serializable")),
        )
        .unwrap();

        assert_eq!(request.requested_model, "relay/upstream-model");
        assert_eq!(request.resolved_model, "upstream-model");
        assert!(request.responses_lite);
        assert!(request.response_affinity_key.is_some());
        assert!(request.prompt_affinity_key.is_some());
        assert_eq!(payload["type"], "response.create");
        assert_eq!(payload["model"], "upstream-model");
        assert_eq!(payload["input"], "hello");
    }

    #[test]
    fn client_request_rejects_non_create_messages_before_candidate_selection() {
        let runtime = runtime();
        let key = runtime
            .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
            .unwrap();
        let error = match ClientRequest::parse(
            &runtime,
            &key,
            &HeaderMap::new(),
            br#"{"type":"response.cancel","model":"relay/upstream-model"}"#,
        ) {
            Ok(_) => panic!("non-create message should be rejected"),
            Err(error) => error,
        };

        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert_eq!(error.category, "invalid_request");
        assert_eq!(error.message, "only response.create messages are supported");
    }

    #[test]
    fn client_request_accepts_and_preserves_a_stream_id() {
        let runtime = runtime();
        let key = runtime
            .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
            .unwrap();
        let request = match ClientRequest::parse(
            &runtime,
            &key,
            &HeaderMap::new(),
            br#"{"type":"response.create","stream_id":"main","model":"relay/upstream-model","input":"hello"}"#,
        ) {
            Ok(request) => request,
            Err(error) => panic!("stream_id should be accepted: {}", error.message),
        };
        assert_eq!(request.stream_id.as_deref(), Some("main"));
        let route = runtime
            .executor_route(
                "source",
                &request.resolved_model,
                &key.scope_snapshot(),
                WEBSOCKET_PROTOCOLS,
                false,
            )
            .expect("test source should be routable");
        let payload = request
            .payload_for(&route)
            .unwrap_or_else(|error| panic!("payload should serialize: {}", error.message));
        assert!(payload.contains("\"stream_id\":\"main\""));
    }

    #[test]
    fn websocket_request_can_repair_foreign_message_item_ids() {
        let runtime = runtime();
        let key = runtime
            .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
            .unwrap();
        let mut request = ClientRequest::parse(
            &runtime,
            &key,
            &HeaderMap::new(),
            br#"{
                "type": "response.create",
                "model": "relay/upstream-model",
                "input": [
                    {"type":"message","id":"item_foreign","role":"assistant","content":"hello"},
                    {"type":"message","id":"msg_native","role":"user","content":"continue"},
                    {"type":"reasoning","id":"item_reasoning","summary":[]}
                ]
            }"#,
        )
        .unwrap_or_else(|error| panic!("request should be accepted: {}", error.message));

        assert!(request.repair_message_item_ids());
        let route = runtime
            .executor_route(
                "source",
                &request.resolved_model,
                &key.scope_snapshot(),
                WEBSOCKET_PROTOCOLS,
                false,
            )
            .expect("test source should be routable");
        let payload: serde_json::Value = serde_json::from_str(
            &request
                .payload_for(&route)
                .unwrap_or_else(|error| panic!("request should serialize: {}", error.message)),
        )
        .expect("payload should be valid JSON");

        assert!(payload.pointer("/input/0/id").is_none());
        assert_eq!(payload.pointer("/input/1/id"), Some(&json!("msg_native")));
        assert_eq!(
            payload.pointer("/input/2/id"),
            Some(&json!("item_reasoning"))
        );
        assert!(!request.repair_message_item_ids());
    }
}
