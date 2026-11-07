use super::auth::{client_api_forbidden, invalid_host, unauthorized};
use super::errors::{
    RateLimitBodyHint, apply_failure_state, current_failure_state, rate_limit_body_hint,
    settle_classified_failure,
};
use super::execution::{CandidateRetryContext, execute_reserved_client_request, wait_for_recovery};
use super::execution::{ResponsesItemPrefixRepairs, repair_responses_item_prefixes};
use super::now_ms;
use super::request::{
    CODEX_RESPONSES_LITE_HEADER, apply_codex_routing_hint, codex_client_version,
    forwarded_codex_headers,
};
use super::response::{UsageAttempt, apply_usage, emit_usage, route_error_origin, usage_event};
use super::streaming::{
    NativeReplayCapture, has_generation_evidence, has_output_delta, has_semantic_output,
    is_compaction_payload, is_empty_responses_terminal, is_known_non_output_event, parse_sse_event,
};
use super::turn_state::request_scope;
use crate::error_codes;
use crate::protocol::ClientWireApi;
use crate::runtime::{
    AuthenticatedKey, AuthorizationIncarnation, CandidateLease, ExecutorPrepareError, ExecutorRoute,
};
use crate::{ErrorOrigin, GatewayRuntime, UsageEvent, UsageTransport, WireApi};
use axum::body::Body;
use axum::extract::State;
use axum::extract::ws::{CloseFrame, Message, WebSocketUpgrade, close_code};
use axum::http::header::AUTHORIZATION;
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, Request, Response, StatusCode};
use futures_util::StreamExt;
use reqwest_websocket::{
    CloseCode as UpstreamCloseCode, Message as UpstreamMessage, Upgrade,
    WebSocket as UpstreamWebSocket,
};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::time::{Instant as TokioInstant, MissedTickBehavior, interval_at, sleep_until, timeout};

mod bridge;
mod client_socket;
mod connection_responses;
mod event_scope;
mod events;
mod failure;
mod fallback;
mod io;
mod multiplex;
mod request;
mod upstream;

use bridge::{bridge, semantic_output_payload, semantic_output_value};
use client_socket::WebSocket;
use fallback::bridge_http_fallback;
#[cfg(test)]
use fallback::{
    RELAY_ERROR_ORIGIN_HEADER, RELAY_UPSTREAM_ORIGIN_HEADER, fallback_event_message,
    fallback_response_origin,
};
#[cfg(test)]
use upstream::initial_payloads_are_empty_incomplete;
use upstream::{
    ConnectedTransport, connect_transport_while_client_connected,
    connect_upstream_while_client_connected,
};

use events::{
    EventTerminal, EventTerminalOutcome, event_terminal, incomplete_requires_cooldown,
    incomplete_status, resolved_terminal_failure, terminal_failure_status,
};
use failure::{GatewayFailure, send_gateway_error};
use io::send_message;
use request::{ClientRequest, PreparedPayload};

const MAX_WEBSOCKET_MESSAGE_BYTES: usize = 64 * 1024 * 1024;
const MAX_WEBSOCKET_ERROR_BYTES: usize = 1024 * 1024;
const MAX_NAMED_STREAMS: usize = 32;
const MAX_ACTIVE_RESPONSES: usize = 16;
const INITIAL_MESSAGE_TIMEOUT: Duration = Duration::from_secs(60);
const UPSTREAM_CONNECT_TIMEOUT: Duration = Duration::from_secs(60);
const WEBSOCKET_IDLE_TIMEOUT: Duration = Duration::from_secs(900);
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
        .on_upgrade(move |socket| async move {
            let socket = WebSocket::new(socket);
            if runtime.codex_websockets_enabled() {
                handle_connection(socket, runtime, key, headers).await;
            } else {
                multiplex::serve_http(socket, runtime, key, headers).await;
            }
        })
}

async fn handle_connection(
    mut downstream: WebSocket,
    runtime: Arc<GatewayRuntime>,
    key: AuthenticatedKey,
    headers: HeaderMap,
) {
    let client_request = match read_initial_request(&mut downstream, &runtime, &key, &headers).await
    {
        Ok(client_request) => client_request,
        Err((failure, stream_id)) => {
            send_gateway_error(&mut downstream, &failure, None, stream_id.as_deref()).await;
            return;
        }
    };

    if downstream.note_request_prepared().await.is_err() {
        return;
    }
    let request_id = client_request.request_id.clone();
    if let Some(kind) = client_request.background_kind
        && !runtime.codex_background_request_enabled(kind)
    {
        runtime.blocked_codex_background_event(
            &client_request.request_id,
            &key.id,
            &client_request.requested_model,
            WireApi::Responses,
            UsageTransport::Websocket,
            kind,
        );
        if let Some(failure) = GatewayFailure::blocked_memory(kind) {
            send_gateway_error(
                &mut downstream,
                &failure,
                Some(&request_id),
                client_request.stream_id.as_deref(),
            )
            .await;
            return;
        }
        let mut blocked_response_payload = serde_json::json!({
            "type": "response.completed",
            "response": {"id": format!("resp_relay_blocked_{}", client_request.request_id), "object": "response", "status": "completed", "output": [], "usage": {"input_tokens": 0, "output_tokens": 0, "total_tokens": 0}, "metadata": {"zenith_relay": {"blocked": true, "request_type": kind}}}
        });
        if let Some(stream_id) = client_request.stream_id.as_deref() {
            blocked_response_payload["stream_id"] = json!(stream_id);
        }
        if send_message(
            &mut downstream,
            Message::Text(blocked_response_payload.to_string().into()),
        )
        .await
        .is_ok()
        {
            let _ = send_message(&mut downstream, Message::Close(None)).await;
        }
        return;
    }
    let stream_id = client_request.stream_id.clone();
    if downstream.is_http_lane() || !runtime.codex_websockets_enabled() {
        bridge_http_fallback(
            downstream,
            runtime,
            key,
            headers,
            upstream::HttpFallback::new(
                client_request,
                crate::usage::TransportFallbackReason::WebsocketDisabled,
            ),
        )
        .await;
        return;
    }
    let connected = match connect_transport_while_client_connected(
        &mut downstream,
        &runtime,
        &key,
        &headers,
        client_request,
        true,
        0,
        None,
    )
    .await
    {
        Ok(ConnectedTransport::Websocket(connected)) => *connected,
        Ok(ConnectedTransport::Http(fallback)) => {
            bridge_http_fallback(downstream, runtime, key, headers, *fallback).await;
            return;
        }
        Err(failure) => {
            send_gateway_error(
                &mut downstream,
                &failure,
                Some(&request_id),
                stream_id.as_deref(),
            )
            .await;
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
) -> Result<ClientRequest, (GatewayFailure, Option<String>)> {
    let deadline = TokioInstant::now() + INITIAL_MESSAGE_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(TokioInstant::now());
        if remaining.is_zero() {
            return Err((GatewayFailure::request_timeout(), None));
        }
        let message = timeout(remaining, downstream.recv())
            .await
            .map_err(|_| (GatewayFailure::request_timeout(), None))?
            .ok_or_else(|| (GatewayFailure::client_closed(), None))?
            .map_err(|_| {
                (
                    GatewayFailure::invalid_request("invalid WebSocket frame"),
                    None,
                )
            })?;
        match message {
            message @ (Message::Text(_) | Message::Binary(_)) => {
                let payload: &[u8] = match &message {
                    Message::Text(text) => text.as_bytes(),
                    Message::Binary(bytes) => bytes,
                    _ => unreachable!("matched a data frame"),
                };
                match ClientRequest::prepare(runtime, key, headers, payload, None).await {
                    Ok(request) => return Ok(request),
                    Err(failure) => {
                        if failure::send_request_error(
                            downstream,
                            &failure,
                            None,
                            ClientRequest::error_stream_id(payload).as_deref(),
                        )
                        .await
                        .is_err()
                        {
                            return Err((GatewayFailure::client_closed(), None));
                        }
                    }
                }
            }
            Message::Ping(payload) => {
                if send_message(downstream, Message::Pong(payload))
                    .await
                    .is_err()
                {
                    return Err((GatewayFailure::client_closed(), None));
                }
            }
            Message::Pong(_) => {}
            Message::Close(_) => return Err((GatewayFailure::client_closed(), None)),
        }
    }
}

/// Keeps the client-facing Responses WebSocket usable when every selected
/// provider only exposes HTTP/SSE. The normal HTTP executor remains the single
/// source of routing, adapters, retries, usage, and quota accounting.
#[cfg(test)]
mod tests;
