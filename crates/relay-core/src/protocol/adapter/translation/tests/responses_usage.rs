//! A Responses client (Codex) fails to parse `response.completed` unless
//! `usage` carries input, output and total together, so bridged routes must
//! deliver all three or `null`.

use super::*;
use serde_json::json;

const BRIDGED_UPSTREAMS: [WireApi; 3] =
    [WireApi::ChatCompletions, WireApi::Messages, WireApi::Gemini];

fn json_usage(client: WireApi, upstream: WireApi, body: &Value) -> Value {
    let completed = prepare(client, upstream, &input(client), false)
        .translate_response_bytes(&serde_json::to_vec(body).unwrap())
        .unwrap()
        .expect("converted response");
    completed.response_body()["usage"].clone()
}

fn stream_usage(upstream: WireApi, events: &[u8]) -> (Value, Value) {
    let mut bridge = prepare(
        WireApi::Responses,
        upstream,
        &input(WireApi::Responses),
        true,
    )
    .into_stream_bridge()
    .unwrap();
    bridge.push(events);
    bridge.finish();
    let body_usage = bridge.completed().expect("stream completes").response_body["usage"].clone();
    let frames = std::iter::from_fn(|| bridge.pop_output())
        .map(|frame| String::from_utf8(frame).unwrap())
        .collect::<Vec<_>>();
    let frame = frames
        .iter()
        .find(|frame| frame.contains("event: response.completed"))
        .expect("response.completed frame");
    let data = frame
        .lines()
        .find_map(|line| line.strip_prefix("data: "))
        .expect("frame data");
    let event: Value = serde_json::from_str(data).unwrap();
    (event["response"]["usage"].clone(), body_usage)
}

#[test]
fn responses_client_gets_a_derived_total_from_every_bridged_upstream() {
    for upstream in BRIDGED_UPSTREAMS {
        let usage = json_usage(WireApi::Responses, upstream, &output(upstream));
        assert_eq!(usage["output_tokens"], 2, "{upstream:?}");
        assert_eq!(usage["total_tokens"], 5, "{upstream:?}");
        assert!(usage["input_tokens"].is_u64(), "{upstream:?}");

        let (frame_usage, body_usage) = stream_usage(upstream, &sse(upstream));
        for usage in [frame_usage, body_usage] {
            assert_eq!(usage["output_tokens"], 2, "{upstream:?} stream");
            assert_eq!(usage["total_tokens"], 5, "{upstream:?} stream");
            assert!(usage["input_tokens"].is_u64(), "{upstream:?} stream");
        }
    }
}

#[test]
fn upstream_reported_total_is_kept_for_a_responses_client() {
    let chat = json!({"id":"chat_test","model":"test","choices":[{"index":0,"message":{"role":"assistant","content":"Hi"},"finish_reason":"stop"}],
        "usage":{"prompt_tokens":3,"completion_tokens":2,"total_tokens":9}});
    let usage = json_usage(WireApi::Responses, WireApi::ChatCompletions, &chat);
    assert_eq!(usage["total_tokens"], 9);

    let gemini = json!({"candidates":[{"index":0,"content":{"role":"model","parts":[{"text":"Hi"}]},"finishReason":"STOP"}],
        "usageMetadata":{"promptTokenCount":3,"candidatesTokenCount":2,"totalTokenCount":11}});
    let usage = json_usage(WireApi::Responses, WireApi::Gemini, &gemini);
    assert_eq!(usage["total_tokens"], 11);
}

#[test]
fn messages_total_counts_cache_reads_and_writes_like_the_gateway() {
    let messages = json!({"id":"msg_test","type":"message","role":"assistant","model":"test","content":[{"type":"text","text":"Hi"}],"stop_reason":"end_turn",
        "usage":{"input_tokens":3,"output_tokens":2,"cache_read_input_tokens":10,"cache_creation_input_tokens":5}});
    let usage = json_usage(WireApi::Responses, WireApi::Messages, &messages);
    assert_eq!(usage["total_tokens"], 20);
}

#[test]
fn gemini_total_includes_thought_tokens() {
    let gemini = json!({"candidates":[{"index":0,"content":{"role":"model","parts":[{"text":"Hi"}]},"finishReason":"STOP"}],
        "usageMetadata":{"promptTokenCount":6,"candidatesTokenCount":2,"thoughtsTokenCount":4}});
    let usage = json_usage(WireApi::Responses, WireApi::Gemini, &gemini);
    assert_eq!(usage["total_tokens"], 12);
}

#[test]
fn absent_or_partial_usage_is_null_for_a_responses_client() {
    let chat = |usage: Value| {
        let mut body = output(WireApi::ChatCompletions);
        match usage {
            Value::Null => {
                body.as_object_mut().unwrap().remove("usage");
            }
            usage => body["usage"] = usage,
        }
        body
    };
    for usage in [Value::Null, json!({"completion_tokens":7})] {
        let body = chat(usage);
        assert!(json_usage(WireApi::Responses, WireApi::ChatCompletions, &body).is_null());
    }

    let messages =
        json!({"id":"msg_test","stop_reason":"end_turn","content":[{"type":"text","text":"Hi"}]});
    assert!(json_usage(WireApi::Responses, WireApi::Messages, &messages).is_null());

    let gemini = json!({"candidates":[{"index":0,"content":{"role":"model","parts":[{"text":"Hi"}]},"finishReason":"STOP"}],
        "usageMetadata":{"promptTokenCount":3}});
    assert!(json_usage(WireApi::Responses, WireApi::Gemini, &gemini).is_null());
}

#[test]
fn other_clients_do_not_gain_a_total() {
    let usage = json_usage(
        WireApi::ChatCompletions,
        WireApi::Messages,
        &output(WireApi::Messages),
    );
    assert_eq!(usage["prompt_tokens"], 3);
    assert!(usage.get("total_tokens").is_none());

    let usage = json_usage(
        WireApi::Messages,
        WireApi::ChatCompletions,
        &output(WireApi::ChatCompletions),
    );
    assert_eq!(usage["output_tokens"], 2);
    assert!(usage.get("total_tokens").is_none());
}
