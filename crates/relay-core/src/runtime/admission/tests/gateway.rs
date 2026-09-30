//! Actual loopback drivers: a local queue rejection never becomes provider work.

use super::*;
use futures_util::{SinkExt, StreamExt};
use reqwest_websocket::{Message, Upgrade};
use serde_json::{json, Value};

#[tokio::test]
async fn http_sse_websocket_and_images_report_local_overload_without_upstream_activity() {
    let runtime = Arc::new(runtime());
    runtime.admission.lock().unwrap().limits.requests = 0;
    let (_, held) = runtime
        .admit(request(&runtime, 0, "model-a"), crate::unix_time_ms())
        .await
        .unwrap();
    let before = runtime.candidate_runtime_order();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let router = crate::gateway::router(runtime.clone());
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let client = reqwest::Client::new();
    for stream in [false, true] {
        let response = client
            .post(format!("{origin}/v1/responses"))
            .bearer_auth("synthetic-one")
            .json(&json!({"model":"model-a", "input":"synthetic", "stream":stream}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let value: Value = response.json().await.unwrap();
        assert_eq!(
            value["error"]["code"],
            crate::error_codes::ADMISSION_QUEUE_FULL
        );
    }
    let upgraded = client
        .get(format!("{origin}/v1/responses"))
        .bearer_auth("synthetic-one")
        .upgrade()
        .send()
        .await
        .unwrap();
    let mut socket = upgraded.into_websocket().await.unwrap();
    socket
        .send(Message::Text(
            json!({"type":"response.create","model":"model-a","input":"synthetic"}).to_string(),
        ))
        .await
        .unwrap();
    let response = tokio::time::timeout(Duration::from_secs(2), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let Message::Text(response) = response else {
        panic!("expected a local error frame")
    };
    let value: Value = serde_json::from_str(&response).unwrap();
    assert_eq!(
        value["error"]["code"],
        crate::error_codes::ADMISSION_QUEUE_FULL
    );
    drop(socket);

    let response = client
        .post(format!("{origin}/v1/images/generations"))
        .bearer_auth("synthetic-one")
        .json(&json!({"model":"gpt-image-2", "prompt":"synthetic"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let value: Value = response.json().await.unwrap();
    assert_eq!(
        value["error"]["code"],
        crate::error_codes::ADMISSION_QUEUE_FULL
    );
    assert_eq!(runtime.candidate_runtime_order(), before);
    assert_eq!(queued(&runtime), (0, 0));
    server.abort();
    drop(held);
}
