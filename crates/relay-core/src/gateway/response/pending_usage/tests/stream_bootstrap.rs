use super::*;
use axum::{
    Router,
    body::{Body, Bytes},
    http::Request,
    routing::post,
};
use futures_util::{StreamExt, stream};
use std::convert::Infallible;

#[tokio::test]
async fn streaming_bootstrap_retains_usage_when_cancelled_before_handoff() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let router = Router::new().route(
        "/v1/responses",
        post(|| async {
            axum::http::Response::builder()
                .header("content-type", "text/event-stream")
                .body(Body::from_stream(
                    stream::once(async { Ok::<_, Infallible>(Bytes::from_static(PROGRESS)) })
                        .chain(stream::pending()),
                ))
                .unwrap()
        }),
    );
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    for debug in [false, true] {
        for client in [crate::WireApi::Responses, crate::WireApi::ChatCompletions] {
            let (runtime, events, _) = fixture_for_upstream(&format!("http://{address}/v1"), debug);
            let body = if client == crate::WireApi::Responses {
                serde_json::json!({"model":"synthetic-model","input":"Synthetic request","stream":true})
            } else {
                serde_json::json!({"model":"synthetic-model","messages":[{"role":"user","content":"Synthetic request"}],"stream":true})
            };
            let request_id = "synthetic-progress-request";
            let mut request = Request::builder()
                .method("POST")
                .uri(if client == crate::WireApi::Responses {
                    "/v1/responses"
                } else {
                    "/v1/chat/completions"
                })
                .header("host", "localhost")
                .header("authorization", "Bearer synthetic-local-key")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap();
            request
                .extensions_mut()
                .insert(crate::gateway::execution::RoutedRequestIdentity {
                    request_id: request_id.into(),
                    budget: SharedRequestBudget::for_incoming_request(3),
                    transport: crate::UsageTransport::Http,
                    transport_fallback: None,
                    service_tier_policy: crate::gateway::request::ServiceTierPolicy::client_owned(
                        &body,
                    ),
                    recovery: None,
                });
            let captured_runtime = runtime.clone();
            let task = tokio::spawn(async move {
                crate::gateway::execution::execute_client_request(captured_runtime, request, client)
                    .await
            });
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    if task.is_finished() {
                        panic!("stream committed before any output: {client:?}, debug={debug}");
                    }
                    let mut observed = false;
                    runtime.observe_pending_dispatch(request_id, 1, |event| {
                        observed = event.input_tokens == Some(10);
                    });
                    if observed {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("stream bootstrap discarded reported usage before handoff");
            assert!(events.lock().unwrap().is_empty());
            task.abort();
            assert!(matches!(task.await, Err(error) if error.is_cancelled()));
            let events = events.lock().unwrap();
            assert_eq!(events.len(), 1, "{client:?}, debug={debug}");
            let event = &events[0];
            assert_eq!(event.http_status, 499);
            assert_eq!(event.error_origin(), Some(crate::ErrorOrigin::Relay));
            assert_eq!(event.attempt, 1);
            assert_eq!(event.input_tokens, Some(10));
            assert_eq!(event.cached_input_tokens, Some(4));
            assert_eq!(event.output_tokens, Some(2));
            assert_eq!(event.reasoning_tokens, Some(1));
            assert_eq!(event.total_tokens, Some(12));
            assert_eq!(event.cache_write_input_tokens, None);
            assert_eq!(event.ttft_ms, None);
            assert_eq!(runtime.candidate_runtime_order()[0].in_flight, 0);
        }
    }
    server.abort();
    let _ = server.await;
}
