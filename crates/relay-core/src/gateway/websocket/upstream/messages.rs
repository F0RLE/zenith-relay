use super::super::*;

pub(in crate::gateway::websocket) fn first_message_terminal(
    message: &UpstreamMessage,
) -> Option<EventTerminal> {
    Some(initial_message_state(message).1)
}

pub(in crate::gateway::websocket) fn message_serves_rejected_model(
    message: &UpstreamMessage,
    expected: &str,
) -> bool {
    let upstream_message_bytes = match message {
        UpstreamMessage::Text(text) => text.as_bytes(),
        UpstreamMessage::Binary(bytes) => bytes.as_ref(),
        _ => return false,
    };
    if crate::gateway::streaming::fast_response_delta_json(upstream_message_bytes).is_some() {
        return false;
    }
    serde_json::from_slice::<Value>(upstream_message_bytes).is_ok_and(|upstream_payload| {
        super::super::super::streaming::served_model_is_rejected(&upstream_payload, expected)
    })
}

pub(in crate::gateway::websocket) fn initial_message_state(
    message: &UpstreamMessage,
) -> (bool, EventTerminal) {
    let upstream_message_bytes = match message {
        UpstreamMessage::Text(text) => text.as_bytes(),
        UpstreamMessage::Binary(bytes) => bytes.as_ref(),
        // This function is only called for application messages, but retain
        // conservative behavior if that invariant changes.
        _ => return (true, EventTerminal::default()),
    };
    let Ok(upstream_message_value) = serde_json::from_slice::<Value>(upstream_message_bytes) else {
        // A malformed frame has already reached the bridge and must never be
        // replayed to another account as if it were setup metadata.
        return (true, EventTerminal::default());
    };
    let event_type = upstream_message_value.get("type").and_then(Value::as_str);
    (
        has_generation_evidence(&upstream_message_value, event_type),
        event_terminal(&upstream_message_value),
    )
}

pub(in crate::gateway::websocket) fn initial_messages_are_empty_incomplete(
    messages: &[UpstreamMessage],
) -> bool {
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
    let saw_output = payloads.iter().any(|upstream_payload| {
        has_semantic_output(
            upstream_payload,
            upstream_payload.get("type").and_then(Value::as_str),
        )
    });
    let completed_output_items = payloads
        .iter()
        .filter(|upstream_payload| {
            let event_type = upstream_payload.get("type").and_then(Value::as_str);
            event_type == Some("response.output_item.done")
                && !is_compaction_payload(upstream_payload, event_type)
        })
        .count();
    is_empty_responses_terminal(terminal, saw_output, completed_output_items)
}
