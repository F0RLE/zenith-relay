use super::*;
use futures_util::{SinkExt, StreamExt};
use reqwest_websocket::{Message, Upgrade};

pub(super) fn reasoning_stream(response: &Value) -> Vec<u8> {
    let mut pending =
        crate::gateway::test_support::basis_points_stream(&serde_json::to_vec(response).unwrap());
    let mut wire = Vec::new();
    while let Some(frame) = crate::protocol::take_sse_event(&mut pending) {
        let Ok(mut event) = serde_json::from_slice::<Value>(&crate::protocol::sse_data(&frame))
        else {
            wire.extend(frame);
            continue;
        };
        if event["type"] == "response.output_item.added" && event["item"]["type"] == "reasoning" {
            event["item"]["encrypted_content"] = json!("synthetic-added-cipher");
        } else if event["type"] == "response.completed" {
            event["response"]["output"][0]["encrypted_content"] =
                json!("synthetic-terminal-cipher");
        }
        wire.extend(format!("data: {event}\n\n").as_bytes());
    }
    wire
}

fn first_response() -> Value {
    json!({"id":"resp_siwc_reasoning","status":"completed","model":"gpt-test","output":[
        {"type":"reasoning","id":"rs_siwc","status":"completed","summary":[],
            "encrypted_content":"synthetic-done-cipher"},
        {"type":"function_call","id":"fc_siwc","call_id":"call_siwc","name":"lookup",
            "arguments":"{}","status":"completed"}
    ]})
}

fn request() -> Value {
    json!({"model":"gpt-test","input":[{"role":"user","content":"Synthetic first turn"}],
        "tools":[{"type":"function","name":"lookup","parameters":{"type":"object"}}],
        "reasoning":{"effort":"high"}})
}

fn result() -> Value {
    json!({"type":"function_call_output","call_id":"call_siwc","output":"Synthetic tool result"})
}

fn answer() -> Value {
    json!({"id":"resp_siwc_answer","status":"completed","model":"gpt-test","output":[
        {"type":"message","role":"assistant","content":[{"type":"output_text","text":"Synthetic next answer"}]}
    ]})
}

fn assert_replayed(body: &Value) {
    assert_eq!(body["input"][1], request()["input"][0]);
    assert_eq!(body["input"][2], first_response()["output"][0]);
    assert_eq!(body["input"][3], first_response()["output"][1]);
    assert_eq!(body["input"][4], result());
    assert_eq!(body["reasoning"], request()["reasoning"]);
    assert!(body.get("previous_response_id").is_none());
    assert!(!body.to_string().contains("synthetic-terminal-cipher"));
    assert!(!body.to_string().contains("synthetic-added-cipher"));
}

#[tokio::test]
async fn siwc_reasoning_http_tool_roundtrip_retains_completed_state() {
    reasoning_http_tool_roundtrip(true).await;
}

#[tokio::test]
async fn siwc_reasoning_http_json_tool_roundtrip_retains_completed_state() {
    reasoning_http_tool_roundtrip(false).await;
}

async fn reasoning_http_tool_roundtrip(upstream_stream: bool) {
    for stream in [false, true] {
        for previous_id in [false, true] {
            let siwc = AccessServer::start(json!([]), false).await;
            let native = AccessServer::start(json!([]), false).await;
            siwc.state
                .stream_generations
                .store(upstream_stream, Ordering::SeqCst);
            if upstream_stream {
                crate::poison::mutex(&siwc.state.stream_bodies)
                    .push_back(reasoning_stream(&first_response()));
            } else {
                crate::poison::mutex(&siwc.state.generation_replies)
                    .push_back((StatusCode::OK, first_response()));
            }
            *crate::poison::mutex(&siwc.state.generation_body) = answer();
            let (base, task) = recovery::gateway(runtime(&siwc, &native).await).await;
            let client = retry_admission::bounded_client();
            let mut payload = request();
            payload["stream"] = json!(stream);
            let first = client
                .post(format!("{base}/v1/responses"))
                .bearer_auth("local-secret")
                .json(&payload)
                .send()
                .await
                .unwrap();
            let status = first.status();
            assert_eq!(
                first.headers()["content-type"],
                if stream {
                    "text/event-stream"
                } else {
                    "application/json"
                }
            );
            let text = first.text().await.unwrap();
            assert_eq!(status, StatusCode::OK, "{text}");
            assert!(text.contains("synthetic-done-cipher"), "{text}");
            if !stream {
                let response: Value = serde_json::from_str(&text).unwrap();
                assert_eq!(response["output"], first_response()["output"]);
            }
            if previous_id {
                payload["previous_response_id"] = first_response()["id"].clone();
                payload["input"] = json!([result()]);
            } else {
                payload["input"].as_array_mut().unwrap().extend(
                    first_response()["output"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .cloned(),
                );
                payload["input"].as_array_mut().unwrap().push(result());
            }
            let next = client
                .post(format!("{base}/v1/responses"))
                .bearer_auth("local-secret")
                .json(&payload)
                .send()
                .await
                .unwrap();
            let status = next.status();
            let text = next.text().await.unwrap();
            assert_eq!(status, StatusCode::OK, "{text}");
            assert!(text.contains("Synthetic next answer"), "{text}");
            let bodies = crate::poison::mutex(&siwc.state.generation_bodies);
            assert_eq!(bodies.len(), 2);
            assert_replayed(&bodies[1]);
            assert_eq!(native.state.generation_requests.load(Ordering::SeqCst), 0);
            task.abort();
        }
    }
}

#[tokio::test]
async fn siwc_reasoning_websocket_tool_roundtrip_retains_completed_state() {
    let siwc = AccessServer::start(json!([]), false).await;
    let native = AccessServer::start(json!([]), false).await;
    siwc.state.stream_generations.store(true, Ordering::SeqCst);
    crate::poison::mutex(&siwc.state.stream_bodies).push_back(reasoning_stream(&first_response()));
    *crate::poison::mutex(&siwc.state.generation_body) = answer();
    let (base, task) = recovery::gateway(runtime(&siwc, &native).await).await;
    let mut socket = retry_admission::bounded_client()
        .get(format!("{base}/v1/responses"))
        .bearer_auth("local-secret")
        .upgrade()
        .send()
        .await
        .unwrap()
        .into_websocket()
        .await
        .unwrap();
    let mut payload = request();
    payload["type"] = json!("response.create");
    for turn in 0..2 {
        if turn == 1 {
            payload["previous_response_id"] = first_response()["id"].clone();
            payload["input"] = json!([result()]);
        }
        socket
            .send(Message::Text(payload.to_string()))
            .await
            .unwrap();
        let terminal = tokio::time::timeout(Duration::from_secs(3), async {
            while let Some(message) = socket.next().await {
                if let Message::Text(text) = message.unwrap() {
                    let event: Value = serde_json::from_str(&text).unwrap();
                    if matches!(event["type"].as_str(), Some("response.completed" | "error")) {
                        return event;
                    }
                }
            }
            panic!("Synthetic SIWC socket closed before completion");
        })
        .await
        .unwrap();
        assert_eq!(terminal["type"], "response.completed", "{terminal}");
    }
    let bodies = crate::poison::mutex(&siwc.state.generation_bodies);
    assert_eq!(bodies.len(), 2);
    assert_replayed(&bodies[1]);
    assert_eq!(native.state.generation_requests.load(Ordering::SeqCst), 0);
    task.abort();
}
