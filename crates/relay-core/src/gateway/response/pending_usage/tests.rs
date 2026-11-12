use super::*;
use crate::scheduler::rotation::SharedRequestBudget;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const PROGRESS: &[u8] = b"data: {\"type\":\"response.in_progress\",\"response\":{\"usage\":{\"input_tokens\":10,\"input_tokens_details\":{\"cached_tokens\":4},\"output_tokens\":2,\"output_tokens_details\":{\"reasoning_tokens\":1},\"total_tokens\":12,\"ignored\":\"synthetic-private-counter-note\"}}}\n\n";

mod stream_bootstrap;

#[test]
fn buffered_usage_state_stays_small_inside_nested_request_futures() {
    assert!(std::mem::size_of::<PendingUsageObservation<'_>>() <= 64);
}

fn fixture() -> (Arc<GatewayRuntime>, Arc<Mutex<Vec<UsageEvent>>>, UsageEvent) {
    fixture_for_upstream("http://127.0.0.1:1/v1", false)
}

fn fixture_for_upstream(
    base_url: &str,
    debug: bool,
) -> (Arc<GatewayRuntime>, Arc<Mutex<Vec<UsageEvent>>>, UsageEvent) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let captured = events.clone();
    let runtime = GatewayRuntime::from_pool(
        vec![crate::RuntimeSource::unrestricted(crate::ProviderSource {
            id: "synthetic-source".into(),
            name: "Synthetic source".into(),
            base_url: base_url.into(),
            api_key: "synthetic-key".into(),
            proxy_url: None,
            wire_api: crate::WireApi::Responses,
            models: vec!["synthetic-model".into()],
        })],
        vec![crate::RuntimeLocalKey::unrestricted(
            crate::LocalGatewayKey {
                id: "synthetic-local".into(),
                secret: "synthetic-local-key".into(),
            },
        )],
        crate::GatewayRuntimeOptions {
            diagnostics_enabled: if debug { || true } else { || false },
            ..Default::default()
        },
        Arc::new(move |event| captured.lock().unwrap().push(event)),
    )
    .unwrap();
    let mut event = crate::gateway::test_support::test_usage_event();
    event.success = false;
    event.client_context_id = Some("synthetic-context".into());
    event.routing = Some(
        serde_json::from_value(serde_json::json!({
            "reason":"only_eligible", "eligibleCandidates":1,
            "quotaRemainingBasisPoints":null, "inFlightBefore":0, "dispatchesBefore":0
        }))
        .unwrap(),
    );
    (Arc::new(runtime), events, event)
}

#[test]
fn reported_progress_survives_cancellation_without_extra_emissions_or_private_payloads() {
    let (runtime, events, event) = fixture();
    let budget = SharedRequestBudget::for_incoming_request(2);
    let scope = runtime.track_request_usage(&event.request_id, &budget);
    runtime.track_dispatch_usage(event.clone(), Instant::now() - Duration::from_millis(200));
    let mut observation = PendingUsageObservation::new(&runtime, event.clone());
    let split = PROGRESS.len() / 2;
    observation.observe(&PROGRESS[..split]);
    assert_eq!(observation.revision, 0);
    observation.observe(&PROGRESS[split..]);
    let revision = observation.revision;
    assert!(revision > 0);
    observation
        .observe(b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"synthetic\"}\n\n");
    assert_eq!(observation.revision, revision);
    observation.observe(b"data: {\"type\":\"response.in_progress\",\"response\":{\"usage\":{\"output_tokens\":3,\"total_tokens\":13}}}\n\n");
    assert!(events.lock().unwrap().is_empty());
    drop(scope);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    let event = &events[0];
    assert_eq!(event.http_status, 499);
    assert_eq!(event.input_tokens, Some(10));
    assert_eq!(event.cached_input_tokens, Some(4));
    assert_eq!(event.output_tokens, Some(3));
    assert_eq!(event.reasoning_tokens, Some(1));
    assert_eq!(event.total_tokens, Some(13));
    assert_eq!(event.cache_write_input_tokens, None);
    assert_eq!(event.ttft_ms, None);
    assert!(event.latency_ms >= 200);
    assert_eq!(
        event.client_context_id.as_deref(),
        Some("synthetic-context")
    );
    assert!(
        !serde_json::to_string(event)
            .unwrap()
            .contains("synthetic-private-counter-note")
    );
}

#[test]
fn progress_cannot_create_a_send_or_change_another_attempt() {
    for attempt in [None, Some(2)] {
        let (runtime, events, event) = fixture();
        let scope = runtime.track_request_usage(
            &event.request_id,
            &SharedRequestBudget::for_incoming_request(2),
        );
        if let Some(attempt) = attempt {
            let mut current = event.clone();
            current.attempt = attempt;
            runtime.track_dispatch_usage(current, Instant::now());
        }
        PendingUsageObservation::new(&runtime, event).observe(PROGRESS);
        drop(scope);
        let events = events.lock().unwrap();
        if let Some(attempt) = attempt {
            assert_eq!(events.len(), 1);
            assert_eq!(events[0].attempt, attempt);
            assert_eq!(events[0].input_tokens, None);
            assert_eq!(events[0].output_tokens, None);
        } else {
            assert!(events.is_empty());
        }
    }
}

#[test]
fn completed_attempt_ignores_late_progress_and_scope_drop() {
    let (runtime, events, mut event) = fixture();
    let scope = runtime.track_request_usage(
        &event.request_id,
        &SharedRequestBudget::for_incoming_request(2),
    );
    runtime.track_dispatch_usage(event.clone(), Instant::now());
    let mut observation = PendingUsageObservation::new(&runtime, event.clone());
    observation.observe(PROGRESS);
    event.success = true;
    event.input_tokens = Some(12);
    event.output_tokens = Some(4);
    event.total_tokens = Some(16);
    crate::gateway::response::emit_usage(&runtime, event);
    observation.observe(b"data: {\"type\":\"response.in_progress\",\"response\":{\"usage\":{\"input_tokens\":999}}}\n\n");
    drop(scope);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(events[0].success);
    assert_eq!(events[0].input_tokens, Some(12));
    assert_eq!(events[0].total_tokens, Some(16));
}

#[tokio::test]
async fn buffered_collectors_keep_usage_before_first_output_when_cancelled() {
    use axum::{
        Router,
        body::{Body, Bytes},
        routing::get,
    };
    use futures_util::{StreamExt, stream};
    use std::convert::Infallible;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let router = Router::new().route(
        "/",
        get(|| async {
            axum::http::Response::builder()
                .header("content-type", "text/event-stream")
                .body(Body::from_stream(
                    stream::once(async { Ok::<_, Infallible>(Bytes::from_static(PROGRESS)) })
                        .chain(stream::pending()),
                ))
                .unwrap()
        }),
    );
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    for basis_points in [false, true] {
        let (runtime, events, event) = fixture();
        let observed_runtime = runtime.clone();
        let request_id = event.request_id.clone();
        let task = tokio::spawn(async move {
            let budget = SharedRequestBudget::for_incoming_request(2);
            let _scope = runtime.track_request_usage(&event.request_id, &budget);
            let started = Instant::now();
            runtime.track_dispatch_usage(event.clone(), started);
            let upstream = reqwest::get(format!("http://{address}/")).await.unwrap();
            let observation = Some(PendingUsageObservation::new(&runtime, event));
            let mut timing = crate::gateway::response::BufferedResponseTiming::default();
            if basis_points {
                crate::gateway::response::collect_basis_points_response(
                    upstream,
                    None,
                    started,
                    &mut timing,
                    observation,
                )
                .await
            } else {
                crate::gateway::response::collect_upstream_response(
                    upstream,
                    true,
                    None,
                    false,
                    started,
                    &mut timing,
                    observation,
                )
                .await
            }
        });
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let mut observed = false;
                observed_runtime.observe_pending_dispatch(&request_id, 1, |event| {
                    observed = event.input_tokens == Some(10);
                });
                if observed {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("buffered reader did not retain the reported progress");
        assert!(!task.is_finished());
        assert!(events.lock().unwrap().is_empty());
        task.abort();
        assert!(matches!(task.await, Err(error) if error.is_cancelled()));
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].http_status, 499);
        assert_eq!(events[0].input_tokens, Some(10));
        assert_eq!(events[0].output_tokens, Some(2));
        assert_eq!(events[0].total_tokens, Some(12));
    }
    server.abort();
    let _ = server.await;
}
