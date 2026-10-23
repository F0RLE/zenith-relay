use super::*;

#[test]
fn converted_chat_streams_honor_client_usage_options_without_losing_accounting() {
    for upstream in [WireApi::Responses, WireApi::Messages, WireApi::Gemini] {
        for options in [
            None,
            Some(Value::Null),
            Some(json!({})),
            Some(json!({"include_usage":null})),
            Some(json!({"include_usage":false})),
            Some(json!({"include_usage":true})),
        ] {
            let mut request = input(WireApi::ChatCompletions);
            if let Some(options) = options {
                request["stream_options"] = options;
            }
            let include_usage =
                request.pointer("/stream_options/include_usage") == Some(&Value::Bool(true));
            let mut bridge = prepare(WireApi::ChatCompletions, upstream, &request, true)
                .into_stream_bridge()
                .unwrap();
            for fragment in sse(upstream).chunks(3) {
                bridge.push(fragment);
            }
            bridge.finish();
            let completed = bridge.completed().expect("converted Chat stream completes");
            let usage = completed.response_body["usage"].clone();
            assert_eq!(usage["prompt_tokens"], 3);
            assert_eq!(usage["completion_tokens"], 2);
            let mut usage_chunks = 0;
            let mut done = 0;
            while let Some(frame) = bridge.pop_output() {
                let data = crate::protocol::sse_data(&frame);
                if data.as_ref() == b"[DONE]" {
                    done += 1;
                    continue;
                }
                let payload: Value = serde_json::from_slice(&data).unwrap();
                if payload["choices"].as_array().unwrap().is_empty() {
                    usage_chunks += 1;
                    assert!(include_usage);
                    assert_eq!(payload["usage"], usage);
                } else {
                    assert_eq!(payload.get("usage").is_some(), include_usage);
                    assert!(payload["usage"].is_null());
                }
            }
            assert_eq!(usage_chunks, usize::from(include_usage));
            assert_eq!(done, 1);
        }
    }
}

#[test]
fn converted_chat_usage_options_reject_non_boolean_values() {
    for value in [json!("true"), json!(1), json!([]), json!({})] {
        let mut request = input(WireApi::ChatCompletions);
        request["stream_options"] = json!({"include_usage":value});
        let error = decode::request(WireApi::ChatCompletions, &request).unwrap_err();
        assert_eq!(error.parameter(), Some("stream_options.include_usage"));
    }
}
