use super::upstream::await_while_client_connected;
use super::*;

pub(super) async fn bridge_http_fallback(
    mut downstream: WebSocket,
    runtime: Arc<GatewayRuntime>,
    key: AuthenticatedKey,
    headers: HeaderMap,
    mut request: ClientRequest,
) {
    let mut stream_id = request.stream_id.clone();
    loop {
        let mut client_visible_output = false;
        if let Err(failure) = serve_http_fallback_request(
            &mut downstream,
            runtime.clone(),
            &key,
            &headers,
            &request,
            &mut client_visible_output,
        )
        .await
        {
            if !client_visible_output {
                let request_id = Some(request.request_id.as_str());
                send_gateway_error(
                    &mut downstream,
                    &failure,
                    request_id,
                    request.stream_id.as_deref(),
                )
                .await;
            } else if failure.category != "client_closed" {
                let _ = downstream
                    .send(Message::Close(Some(CloseFrame {
                        code: close_code::ERROR,
                        reason: "upstream stream ended after output".into(),
                    })))
                    .await;
            }
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
                send_gateway_error(&mut downstream, &failure, None, None).await;
                return;
            }
        };
        if let Some(next_stream_id) = next_request.stream_id.as_deref() {
            if let Some(expected) = stream_id.as_deref() {
                if expected != next_stream_id {
                    let failure = GatewayFailure::invalid_request(
                        "only one WebSocket stream_id is supported per connection",
                    );
                    send_gateway_error(
                        &mut downstream,
                        &failure,
                        Some(&next_request.request_id),
                        next_request.stream_id.as_deref(),
                    )
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
    client_visible_output: &mut bool,
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
    let mut http_request = http_request;
    http_request
        .extensions_mut()
        .insert(request.tool_policy.clone());
    http_request
        .extensions_mut()
        .insert(super::super::execution::RoutedRequestIdentity {
            request_id: request.request_id.clone(),
            budget: request.budget.clone(),
        });
    let response = await_while_client_connected(
        downstream,
        execute_client_request(Arc::clone(&runtime), http_request, WireApi::Responses),
    )
    .await?;
    // The HTTP executor already knows whether the selected route belongs to an
    // account or an API provider. Preserve that attribution across the
    // WebSocket bridge instead of turning every fallback response into a Relay
    // error merely because the bridge is the component reading it.
    let response_origin = fallback_response_origin(&response);
    if !response.status().is_success() {
        let status = response.status();
        let body = await_while_client_connected(
            downstream,
            axum::body::to_bytes(response.into_body(), MAX_WEBSOCKET_ERROR_BYTES),
        )
        .await?
        .ok();
        return Err(
            GatewayFailure::upstream_status(status, body.as_deref(), response_origin)
                .apply_degraded_route_policy(&runtime),
        );
    }

    let stream_origin = response_origin;

    let mut body = response.into_body().into_data_stream();
    let mut pending = Vec::new();
    while let Some(chunk) = await_while_client_connected(downstream, body.next()).await? {
        let chunk = chunk.map_err(|_| GatewayFailure::transport(stream_origin))?;
        pending.extend_from_slice(&chunk);
        while let Some(event) = crate::protocol::take_sse_event(&mut pending) {
            if event.len() > MAX_SSE_EVENT_BYTES {
                return Err(GatewayFailure::message_too_large(ErrorOrigin::Relay));
            }
            let terminal = parse_sse_event(&event);
            if terminal.has_data && !terminal.valid {
                return Err(GatewayFailure::transport(stream_origin));
            }
            // An unframed [DONE] carries no Responses terminal payload for a
            // WebSocket client, even if an HTTP adapter treated it as complete.
            if terminal.outcome.is_some() && terminal.payload.is_none() {
                return Err(GatewayFailure::closed(stream_origin));
            }
            if let Some(payload) =
                fallback_event_message(&terminal, request.stream_id.as_deref(), stream_origin)?
            {
                *client_visible_output |= payload.semantic_output;
                downstream
                    .send(payload.message)
                    .await
                    .map_err(|_| GatewayFailure::client_closed())?;
            }
            if terminal.outcome.is_some() {
                return Ok(());
            }
        }
        // Complete events have already been drained above. Only an incomplete
        // tail counts against the per-event budget; one transport chunk may
        // contain several complete events whose combined size is larger.
        if pending.len() > MAX_SSE_EVENT_BYTES {
            return Err(GatewayFailure::message_too_large(ErrorOrigin::Relay));
        }
    }
    Err(GatewayFailure::closed(stream_origin))
}

pub(super) struct FallbackEventMessage {
    pub(super) message: Message,
    pub(super) semantic_output: bool,
}

pub(super) fn fallback_event_message(
    terminal: &super::super::streaming::TerminalEvent,
    stream_id: Option<&str>,
    origin: ErrorOrigin,
) -> Result<Option<FallbackEventMessage>, GatewayFailure> {
    if let Some(stream_id) = stream_id {
        let Some(mut payload) = terminal.payload.clone() else {
            // A named lane requires JSON events so the lane can be identified.
            // An opaque, non-JSON compaction event cannot be routed safely.
            return if terminal.raw_data.is_some() {
                Err(GatewayFailure::transport(origin))
            } else {
                Ok(None)
            };
        };
        let Some(object) = payload.as_object_mut() else {
            return Err(GatewayFailure::transport(origin));
        };
        object.insert(
            "stream_id".to_string(),
            Value::String(stream_id.to_string()),
        );
        let payload =
            serde_json::to_vec(&payload).map_err(|_| GatewayFailure::transport(origin))?;
        if payload.len() > MAX_WEBSOCKET_MESSAGE_BYTES {
            return Err(GatewayFailure::message_too_large(origin));
        }
        let semantic_output = !terminal.is_compaction && semantic_output_payload(&payload);
        let text = String::from_utf8(payload).map_err(|_| GatewayFailure::transport(origin))?;
        return Ok(Some(FallbackEventMessage {
            message: Message::Text(text.into()),
            semantic_output,
        }));
    }
    if let Some(raw_data) = terminal.raw_data.as_deref() {
        if raw_data.len() > MAX_WEBSOCKET_MESSAGE_BYTES {
            return Err(GatewayFailure::message_too_large(origin));
        }
        let semantic_output = !terminal.is_compaction && semantic_output_payload(raw_data);
        let message = match String::from_utf8(raw_data.to_vec()) {
            Ok(text) => Message::Text(text.into()),
            Err(error) => Message::Binary(error.into_bytes().into()),
        };
        return Ok(Some(FallbackEventMessage {
            message,
            semantic_output,
        }));
    }

    let Some(payload) = terminal.payload.as_ref() else {
        return Ok(None);
    };
    let payload = serde_json::to_vec(payload).map_err(|_| GatewayFailure::transport(origin))?;
    if payload.len() > MAX_WEBSOCKET_MESSAGE_BYTES {
        return Err(GatewayFailure::message_too_large(origin));
    }
    let semantic_output = semantic_output_payload(&payload);
    let text = String::from_utf8(payload).map_err(|_| GatewayFailure::transport(origin))?;
    Ok(Some(FallbackEventMessage {
        message: Message::Text(text.into()),
        semantic_output,
    }))
}

pub(super) const RELAY_ERROR_ORIGIN_HEADER: &str = "x-zenith-relay-error-origin";
pub(super) const RELAY_UPSTREAM_ORIGIN_HEADER: &str = "x-zenith-relay-upstream-origin";

pub(super) fn fallback_response_origin(response: &Response<Body>) -> ErrorOrigin {
    response
        .headers()
        .get(RELAY_ERROR_ORIGIN_HEADER)
        .or_else(|| response.headers().get(RELAY_UPSTREAM_ORIGIN_HEADER))
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(ErrorOrigin::Relay)
}

pub(super) fn websocket_transport_fallback_status(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::BAD_REQUEST
            | StatusCode::NOT_FOUND
            | StatusCode::METHOD_NOT_ALLOWED
            | StatusCode::UPGRADE_REQUIRED
            | StatusCode::NOT_IMPLEMENTED
    )
}
