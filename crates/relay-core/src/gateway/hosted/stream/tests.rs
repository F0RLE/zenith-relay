use super::*;
use serde_json::json;

const CREATED: &[u8] = b"data: {\"type\":\"response.created\",\"sequence_number\":0,\
    \"response\":{\"id\":\"resp_fixture\",\"model\":\"fixture\",\"status\":\"in_progress\"}}\r\n\r\n";

#[test]
fn frames_preserve_bytes_across_every_split_including_utf8_and_crlf() {
    let input = [
        b": fixture\r\nid: sample\r\nretry: 500\r\n\r\n".as_slice(),
        CREATED,
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"✓\"}\r\n\r\n".as_bytes(),
    ]
    .concat();
    for size in 1..=input.len() {
        let mut frames = Frames::default();
        let mut bytes = Vec::new();
        let mut events = Events {
            model: "fixture".into(),
            id: None,
            text_delta: false,
            sequence: 0,
        };
        for chunk in input.chunks(size) {
            frames.chunk = Bytes::copy_from_slice(chunk);
            while !frames.chunk.is_empty() {
                if let Some(frame) = frames.take().unwrap() {
                    events.observe(&frame).unwrap();
                    bytes.extend_from_slice(&frame);
                }
            }
        }
        assert_eq!(bytes, input, "fragment size {size}");
        assert!(events.text_delta);
        assert_eq!(events.sequence, 2);
    }
}

#[test]
fn limits_apply_to_unterminated_events_and_the_cumulative_wire_bytes() {
    let mut frame = Frames {
        chunk: Bytes::from(vec![b'a'; MAX_EVENT + 1]),
        ..Frames::default()
    };
    assert_eq!(
        frame.take().unwrap_err(),
        HostedExecutionError::OutcomeUnknown(HostedUnknown::ResponseLimit)
    );
    assert_eq!(frame.pending.len(), MAX_EVENT);

    let mut stream = Frames {
        chunk: Bytes::from_static(b": ok\r\r: extra\r\r"),
        used: MAX_STREAM - 6,
        ..Frames::default()
    };
    assert_eq!(stream.take().unwrap().unwrap(), ": ok\r\r");
    assert_eq!(stream.used, MAX_STREAM);
    assert_eq!(
        stream.take().unwrap_err(),
        HostedExecutionError::OutcomeUnknown(HostedUnknown::ResponseLimit)
    );
}

#[test]
fn terminal_binds_to_the_created_response_and_its_own_status() {
    let mut events = Events {
        model: "fixture".into(),
        id: None,
        text_delta: false,
        sequence: 0,
    };
    events.observe(CREATED).unwrap();
    let end = json!({
        "type": "response.completed",
        "sequence_number": 42,
        "response": {"id":"resp_fixture", "model":"fixture", "status":"completed", "output":[]}
    });
    let observe = |events: &mut Events, value: &Value| {
        events.observe(format!("data: {value}\n\n").as_bytes())
    };
    for (field, wrong) in [
        ("id", json!("resp_other")),
        ("model", json!("other-model")),
        ("status", json!("incomplete")),
        ("output", Value::Null),
        ("error", json!({"message":"synthetic"})),
    ] {
        let mut bad = end.clone();
        bad["response"][field] = wrong;
        assert_eq!(observe(&mut events, &bad), Err(INVALID_RESPONSE));
    }
    assert_eq!(
        observe(&mut events, &end),
        Ok(HostedStreamObservation::Terminal {
            kind: HostedTerminalKind::Completed,
            usage: [None; 3],
            text_delta: false,
        })
    );
    assert_eq!(events.sequence, 43);
}
