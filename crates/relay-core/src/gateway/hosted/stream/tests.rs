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
            generation: Generation::default(),
        };
        for chunk in input.chunks(size) {
            frames.chunk = Bytes::copy_from_slice(chunk);
            while !frames.chunk.is_empty() {
                if let Some(frame) = frames.take().unwrap() {
                    events.observe(&frame, Instant::now()).unwrap();
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
        generation: Generation::default(),
    };
    let at = Instant::now();
    events.observe(CREATED, at).unwrap();
    let end = json!({
        "type": "response.completed",
        "sequence_number": 42,
        "response": {"id":"resp_fixture", "model":"fixture", "status":"completed", "output":[]}
    });
    let observe = |events: &mut Events, value: &Value| {
        events.observe(format!("data: {value}\n\n").as_bytes(), at)
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
            generation: None,
        })
    );
    assert_eq!(events.sequence, 43);
}

#[test]
fn generation_uses_paced_text_and_matching_final_usage_only() {
    let at = Instant::now();
    let end = json!({
        "type": "response.completed",
        "response": {
            "id": "resp_fixture", "model": "fixture", "status": "completed",
            "usage": {"input_tokens": 9, "output_tokens": 14, "output_tokens_details": {"reasoning_tokens": 3}},
            "output": [
                {"type": "reasoning"},
                {"id": "msg_fixture", "type": "message", "role": "assistant", "status": "completed",
                    "content": [{"type": "output_text", "text": "First second."}]},
            ]
        }
    });
    let delta = |text: &str| {
        json!({"type":"response.output_text.delta","item_id":"msg_fixture",
        "output_index":1,"content_index":0,"delta":text})
    };
    for (case, spread) in [
        ("paced", 250),
        ("same chunk", 0),
        ("burst", 1),
        ("late text", 250),
        ("missing reasoning", 250),
        ("tool", 250),
        ("bad binding", 250),
        ("incomplete", 250),
        ("relay pause", 250),
        ("coalesced metadata pause", 250),
    ] {
        let mut events = Events {
            model: "fixture".into(),
            id: None,
            text_delta: false,
            sequence: 0,
            generation: Generation::default(),
        };
        let mut end = end.clone();
        let mut second = delta("second.");
        match case {
            "late text" => {
                end["response"]["output"][1]["content"][0]["text"] = json!("First second. Extra")
            }
            "missing reasoning" => end["response"]["usage"]["output_tokens_details"] = Value::Null,
            "tool" => end["response"]["output"]
                .as_array_mut()
                .unwrap()
                .push(json!({"type":"function_call"})),
            "bad binding" => second["item_id"] = json!("other"),
            "incomplete" => {
                end["type"] = json!("response.incomplete");
                end["response"]["status"] = json!("incomplete");
                end["response"]["incomplete_details"] = json!({"reason":"max_output_tokens"});
            }
            _ => {}
        }
        events.observe(CREATED, at).unwrap();
        let first_chunk = at + Duration::from_millis(100);
        events
            .generation
            .mark_read_pause(if case == "coalesced metadata pause" {
                first_chunk
            } else {
                at
            });
        for (value, offset) in [(delta("First "), 100), (second, 100 + spread)] {
            let arrived = at + Duration::from_millis(offset);
            if case == "relay pause" && offset > 100 {
                events.generation.mark_read_pause(arrived);
            }
            events
                .observe(format!("data: {value}\n\n").as_bytes(), arrived)
                .unwrap();
            if case == "relay pause" && offset == 100 {
                events.generation.mark_read_pause(first_chunk);
            }
        }
        events
            .generation
            .mark_read_pause(first_chunk + Duration::from_millis(spread)); // progress after the last text
        let observed = events
            .observe(
                format!("data: {end}\n\n").as_bytes(),
                at + Duration::from_secs(5),
            )
            .unwrap();
        let HostedStreamObservation::Terminal {
            generation,
            text_delta: true,
            ..
        } = observed
        else {
            panic!("not a text terminal: {case}");
        };
        assert_eq!(
            generation,
            (case == "paced").then_some(HostedGeneration {
                duration_ms: 250,
                output_tokens: 10,
            }),
            "{case}"
        );
    }
}
