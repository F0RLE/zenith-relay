use super::super::*;

pub(super) async fn initial_application_messages(
    upstream: &mut UpstreamWebSocket,
    origin: ErrorOrigin,
    block_degraded_routes: bool,
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
        if block_degraded_routes && message_serves_degraded_model(&message) {
            return Err(GatewayFailure::classified(
                StatusCode::NOT_FOUND,
                error_codes::UPSTREAM_ROUTE_DEGRADED,
                origin,
            ));
        }
        let (has_output, terminal) = initial_message_state(&message);
        let committed = has_output || terminal.outcome.is_some();
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
    let mut heartbeat = interval_at(
        TokioInstant::now() + WEBSOCKET_HEARTBEAT_INTERVAL,
        WEBSOCKET_HEARTBEAT_INTERVAL,
    );
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
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

pub(in crate::gateway::websocket) fn first_message_terminal(
    message: &UpstreamMessage,
) -> Option<EventTerminal> {
    Some(initial_message_state(message).1)
}

fn message_serves_degraded_model(message: &UpstreamMessage) -> bool {
    let payload = match message {
        UpstreamMessage::Text(text) => text.as_bytes(),
        UpstreamMessage::Binary(bytes) => bytes.as_ref(),
        _ => return false,
    };
    serde_json::from_slice::<Value>(payload)
        .is_ok_and(|value| super::super::super::streaming::served_model_is_degraded(&value))
}

fn initial_message_state(message: &UpstreamMessage) -> (bool, EventTerminal) {
    let payload = match message {
        UpstreamMessage::Text(text) => text.as_bytes(),
        UpstreamMessage::Binary(bytes) => bytes.as_ref(),
        // This function is only called for application messages, but retain
        // conservative behavior if that invariant changes.
        _ => return (true, EventTerminal::default()),
    };
    let Ok(value) = serde_json::from_slice::<Value>(payload) else {
        // A malformed frame has already reached the bridge and must never be
        // replayed to another account as if it were setup metadata.
        return (true, EventTerminal::default());
    };
    let event_type = value.get("type").and_then(Value::as_str);
    (
        has_semantic_output(&value, event_type),
        event_terminal(&value),
    )
}

pub(super) fn initial_messages_are_empty_incomplete(messages: &[UpstreamMessage]) -> bool {
    let payloads = messages
        .iter()
        .filter_map(|message| match message {
            UpstreamMessage::Text(text) => serde_json::from_slice(text.as_bytes()).ok(),
            UpstreamMessage::Binary(bytes) => serde_json::from_slice(bytes.as_ref()).ok(),
            _ => None,
        })
        .collect::<Vec<Value>>();
    initial_payloads_are_empty_incomplete(&payloads)
}

pub(in crate::gateway::websocket) fn initial_payloads_are_empty_incomplete(
    payloads: &[Value],
) -> bool {
    let Some(terminal) = payloads.last() else {
        return false;
    };
    if terminal.get("type").and_then(Value::as_str) != Some("response.incomplete") {
        return false;
    }
    let saw_output = payloads
        .iter()
        .any(|payload| has_semantic_output(payload, payload.get("type").and_then(Value::as_str)));
    let completed_output_items = payloads
        .iter()
        .filter(|payload| {
            let event_type = payload.get("type").and_then(Value::as_str);
            event_type == Some("response.output_item.done")
                && !is_compaction_payload(payload, event_type)
        })
        .count();
    is_empty_responses_incomplete(terminal, saw_output, completed_output_items)
}
