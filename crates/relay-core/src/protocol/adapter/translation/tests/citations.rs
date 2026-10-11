use super::*;

fn citation() -> Value {
    json!({"type":"url_citation","start_index":0,"end_index":2,
        "url":"https://example.test/source","title":"Synthetic source"})
}

fn cited_response() -> Value {
    json!({"id":"resp_citations","status":"completed","output":[
        {"type":"web_search_call","id":"ws_test","status":"completed"},
        {"type":"message","id":"msg_test","role":"assistant","content":[
            {"type":"output_text","text":"А🦆 ","annotations":[]},
            {"type":"output_text","text":"文献","annotations":[citation()]}
        ]}
    ],"usage":{"input_tokens":3,"output_tokens":2}})
}

#[test]
fn responses_search_citations_use_concatenated_character_offsets_in_chat() {
    let result = prepare(
        WireApi::ChatCompletions,
        WireApi::Responses,
        &input(WireApi::ChatCompletions),
        false,
    )
    .translate_response_bytes(&serde_json::to_vec(&cited_response()).unwrap())
    .unwrap()
    .unwrap();
    let message = &result.response_body()["choices"][0]["message"];
    assert_eq!(message["content"], "А🦆 文献");
    assert_eq!(
        message["annotations"],
        json!([{"type":"url_citation","url_citation":{
        "start_index":3,"end_index":5,"url":"https://example.test/source",
        "title":"Synthetic source"}}])
    );
}

fn cited_stream(terminal_annotations: bool) -> Vec<Value> {
    let mut terminal = cited_response();
    if !terminal_annotations {
        terminal["output"][1]["content"][1]["annotations"] = json!([]);
    }
    vec![
        json!({"type":"response.output_item.added","output_index":0,
            "item":{"type":"web_search_call","id":"ws_test","status":"in_progress"}}),
        json!({"type":"response.web_search_call.completed","output_index":0}),
        json!({"type":"response.output_item.added","output_index":1,
            "item":{"type":"message","id":"msg_test","role":"assistant","content":[]}}),
        json!({"type":"response.content_part.added","output_index":1,"content_index":0,
            "part":{"type":"output_text","text":"","annotations":[]}}),
        json!({"type":"response.output_text.delta","output_index":1,"content_index":0,"delta":"А🦆 "}),
        json!({"type":"response.content_part.added","output_index":1,"content_index":1,
            "part":{"type":"output_text","text":"","annotations":[]}}),
        json!({"type":"response.output_text.delta","output_index":1,"content_index":1,"delta":"文献"}),
        json!({"type":"response.output_text.annotation.added","output_index":1,
            "content_index":1,"annotation_index":0,"annotation":citation()}),
        json!({"type":"response.completed","response":terminal}),
    ]
}

#[test]
fn fragmented_search_stream_preserves_text_and_emits_citations_once_before_finish() {
    for terminal_annotations in [true, false] {
        let mut bridge = prepare(
            WireApi::ChatCompletions,
            WireApi::Responses,
            &input(WireApi::ChatCompletions),
            true,
        )
        .into_stream_bridge()
        .unwrap();
        let events = cited_stream(terminal_annotations);
        for event in &events[..events.len() - 1] {
            let bytes = format!("data: {event}\n\n").into_bytes();
            for fragment in bytes.chunks(3) {
                bridge.push(fragment);
            }
        }
        let mut frames = Vec::new();
        while let Some(bytes) = bridge.pop_output() {
            frames.push(bytes);
        }
        let partial = String::from_utf8(frames.concat()).unwrap();
        assert!(
            partial.contains("А🦆 "),
            "text must stream before completion"
        );
        assert!(bridge.completed().is_none());
        bridge.push(format!("data: {}\n\n", events.last().unwrap()).as_bytes());
        bridge.finish();
        while let Some(bytes) = bridge.pop_output() {
            frames.push(bytes);
        }
        let completed = bridge.completed().expect("cited stream should complete");
        assert_eq!(
            completed.response_body["choices"][0]["message"]["annotations"][0]["url_citation"]
                ["start_index"],
            3
        );
        let output = String::from_utf8(frames.concat()).unwrap();
        assert_eq!(output.matches("\"annotations\"").count(), 1);
        assert!(
            output.find("\"annotations\"").unwrap()
                < output.find("\"finish_reason\":\"stop\"").unwrap()
        );
        assert!(output.ends_with("data: [DONE]\n\n"));
    }
}

#[test]
fn malformed_or_unrepresentable_citations_fail_instead_of_being_dropped() {
    for annotation in [
        json!({"type":"file_citation","file_id":"file_test","index":0}),
        json!({"type":"url_citation","start_index":0,"end_index":3,
            "url":"https://example.test","title":"Out of range"}),
        json!({"type":"url_citation","start_index":2,"end_index":1,
            "url":"https://example.test","title":"Reversed"}),
        json!({"type":"url_citation","start_index":0,"end_index":1,
            "url":"javascript:alert(1)","title":"Invalid URL"}),
    ] {
        let mut response = cited_response();
        response["output"][1]["content"][1]["annotations"] = json!([annotation]);
        assert!(prepare(
            WireApi::ChatCompletions,
            WireApi::Responses,
            &input(WireApi::ChatCompletions),
            false
        )
        .translate_response_bytes(&serde_json::to_vec(&response).unwrap())
        .is_err());
    }
    for client in [WireApi::Messages, WireApi::Gemini] {
        assert!(prepare(client, WireApi::Responses, &input(client), false)
            .translate_response_bytes(&serde_json::to_vec(&cited_response()).unwrap())
            .is_err());
    }
}

#[test]
fn citation_conflicts_and_truncated_streams_never_finish_successfully() {
    for conflict in [false, true] {
        let mut bridge = prepare(
            WireApi::ChatCompletions,
            WireApi::Responses,
            &input(WireApi::ChatCompletions),
            true,
        )
        .into_stream_bridge()
        .unwrap();
        let mut events = cited_stream(true);
        if conflict {
            events.last_mut().unwrap()["response"]["output"][1]["content"][1]["annotations"][0]
                ["title"] = "Conflicting source".into();
        } else {
            events.pop();
        }
        for event in events {
            bridge.push(format!("data: {event}\n\n").as_bytes());
        }
        bridge.finish();
        assert!(bridge.completed().is_none());
        let mut output = Vec::new();
        while let Some(bytes) = bridge.pop_output() {
            output.extend(bytes);
        }
        let output = String::from_utf8(output).unwrap();
        assert!(!output.contains("data: [DONE]"));
        assert!(output.contains("adapter_upstream_stream_invalid"));
    }
}

#[test]
fn annotations_in_done_parts_survive_omission_from_done_items_and_terminal_output() {
    for source in ["part", "annotation"] {
        let mut events = cited_stream(false);
        let terminal = events.pop().unwrap();
        if source == "part" {
            events.pop();
            events.push(
                json!({"type":"response.content_part.done","output_index":1,"content_index":1,
                "part":cited_response()["output"][1]["content"][1]}),
            );
        }
        events.push(json!({"type":"response.output_item.done","output_index":1,"item":terminal["response"]["output"][1]}));
        events.push(terminal);
        let mut bridge = prepare(
            WireApi::ChatCompletions,
            WireApi::Responses,
            &input(WireApi::ChatCompletions),
            true,
        )
        .into_stream_bridge()
        .unwrap();
        for event in events {
            bridge.push(format!("data: {event}\n\n").as_bytes());
        }
        bridge.finish();
        let body = &bridge
            .completed()
            .expect("done citations remain available")
            .response_body;
        let annotations = body["choices"][0]["message"]["annotations"]
            .as_array()
            .unwrap();
        assert_eq!(annotations.len(), 1);
        assert_eq!(annotations[0]["url_citation"]["start_index"], 3);
    }
}
