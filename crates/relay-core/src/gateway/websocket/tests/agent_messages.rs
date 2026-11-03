use super::*;
use serde_json::json;

#[test]
fn websocket_restores_agent_messages_before_native_payload_preparation() {
    let runtime = runtime();
    let key = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
        .unwrap();
    let sealed = runtime
        .agent_message_context(&key.id)
        .seal_argument("collaboration.spawn_agent", "message", "synthetic task")
        .unwrap();
    let mut body = json!({"type":"response.create","model":"relay/upstream-model","input":[{
        "type":"agent_message","author":"/root","recipient":"/root/worker","encrypted_content":sealed
    }]});
    let request = ClientRequest::parse(
        &runtime,
        &key,
        &HeaderMap::new(),
        &serde_json::to_vec(&body).unwrap(),
    )
    .unwrap_or_else(|failure| panic!("{}", failure.message));
    let forwarded: Value = serde_json::from_slice(
        &request
            .http_payload()
            .unwrap_or_else(|failure| panic!("{}", failure.message)),
    )
    .unwrap();
    assert_eq!(forwarded["input"][0]["role"], "user");
    assert_eq!(
        forwarded["input"][0]["content"][1]["text"],
        "synthetic task"
    );
    assert_eq!(request.budget.dispatches(), 0);
    body["input"][0]["encrypted_content"] = runtime
        .agent_message_context("other-key")
        .seal_argument("collaboration.spawn_agent", "message", "synthetic task")
        .unwrap()
        .into();
    let failure = ClientRequest::parse(
        &runtime,
        &key,
        &HeaderMap::new(),
        &serde_json::to_vec(&body).unwrap(),
    )
    .err()
    .expect("different local key must not be accepted");
    assert_eq!(failure.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        failure.category,
        crate::AdapterError::agent_message_unavailable().code()
    );
    assert!(failure.message.contains("start a new chat"));
    assert!(!failure.message.contains("synthetic task"));
}
