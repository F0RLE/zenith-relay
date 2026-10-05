use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

async fn response_from_sse_event(
    event: String,
) -> (reqwest::Response, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let address = listener.local_addr().unwrap();
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        event.len(), event
    );
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = [0_u8; 1024];
        let _ = socket.read(&mut request).await;
        socket.write_all(response.as_bytes()).await.unwrap();
    });
    let upstream = reqwest::get(format!("http://{address}/stream"))
        .await
        .unwrap();
    (upstream, server)
}

#[test]
fn streaming_terminal_errors_keep_the_canonical_category() {
    let terminal = parse_sse_event(
        br#"data: {"type":"response.failed","response":{"error":{"type":"usage_limit_reached","resets_in_seconds":7}}}

"#,
    );
    assert_eq!(terminal.error_category, Some("upstream_quota_exhausted"));
    assert_eq!(terminal.error_status, Some(StatusCode::TOO_MANY_REQUESTS));
    assert_eq!(terminal.cooldown_hint.retry_after_ms, Some(7_000));
    assert!(terminal.cooldown_hint.global);
}

#[test]
fn generic_gateway_rejection_sse_keeps_candidate_category_and_provider_details() {
    let terminal = parse_sse_event(
        br#"event: error
data: {"type":"error","error":{"type":"invalid_request_error","code":"invalid_request","message":"Zenith AI request is invalid. Check the model, messages, tools, and parameters."}}

"#,
    );

    assert_eq!(terminal.error_category, Some("upstream_candidate_rejected"));
    assert_eq!(terminal.error_status, Some(StatusCode::SERVICE_UNAVAILABLE));
    let upstream = terminal.upstream_error.unwrap();
    assert_eq!(upstream.code.as_deref(), Some("invalid_request"));
    assert_eq!(
        upstream.error_type.as_deref(),
        Some("invalid_request_error")
    );
    assert_eq!(upstream.http_status, None);
}

#[tokio::test]
async fn bootstrap_retries_empty_zero_token_incomplete_without_committing_output() {
    let event = concat!(
        "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_test\"}}\n\n",
        "data: {\"type\":\"response.incomplete\",\"response\":{\"output\":[],\"usage\":{\"output_tokens\":0}}}\n\n"
    );
    let (upstream, server) = response_from_sse_event(event.into()).await;
    let failure = bootstrap_stream(upstream, None)
        .await
        .err()
        .expect("empty incomplete stream must not commit client output");
    server.await.unwrap();

    assert_eq!(failure.failure.category, "stream_incomplete");
    assert_eq!(failure.failure.status, StatusCode::BAD_GATEWAY);
}

#[tokio::test]
async fn bootstrap_does_not_commit_an_opaque_compaction_before_disconnect() {
    let event = concat!(
        "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_test\"}}\n\n",
        "event: response.output_item.done\n",
        "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"compaction\",\"encrypted_content\":\"opaque\"}}\n\n"
    );
    let (upstream, server) = response_from_sse_event(event.into()).await;
    let failure = bootstrap_stream(upstream, None)
        .await
        .err()
        .expect("compaction alone must remain retryable");
    server.await.unwrap();

    assert_eq!(failure.failure.category, "stream_incomplete");
    assert_eq!(failure.failure.status, StatusCode::BAD_GATEWAY);
}

#[tokio::test]
async fn large_valid_bootstrap_event_is_not_rejected_at_the_old_limit() {
    let delta = "a".repeat(300 * 1024);
    let event =
        format!("data: {{\"type\":\"response.output_text.delta\",\"delta\":\"{delta}\"}}\n\n");
    let (upstream, server) = response_from_sse_event(event).await;
    let result = bootstrap_stream(upstream, None).await;
    server.await.unwrap();
    assert!(
        result.is_ok(),
        "valid large Responses event should bootstrap"
    );
    let (_, buffered, _) = if let Ok(value) = result {
        value
    } else {
        return;
    };
    assert!(buffered.len() > 256 * 1024);
    assert!(buffered.starts_with(b"data: {\"type\":\"response.output_text.delta\""));
}

#[tokio::test]
async fn heartbeat_never_splits_an_unfinished_sse_frame() {
    let frame = b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"synthetic\"}\r\n\r\n";
    for native_gemini in [false, true] {
        for split in 1..frame.len() {
            let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
            let input = stream::unfold(receiver, |mut receiver| async move {
                receiver.recv().await.map(|chunk| (chunk, receiver))
            });
            let mut stream = usage_stream_with_events(input, Arc::default());
            stream.native_gemini = native_gemini;
            sender
                .send(Ok(Bytes::copy_from_slice(&frame[..split])))
                .unwrap();
            let first = stream.next().await.unwrap().unwrap();
            stream
                .heartbeat
                .as_mut()
                .reset(TokioInstant::now() - Duration::from_secs(1));
            let pending = futures_util::future::poll_fn(|context| {
                Poll::Ready(Pin::new(&mut stream).poll_next(context))
            })
            .await;
            if split == frame.len() - 1 {
                // CR already completes the blank line; its optional LF
                // can arrive after a heartbeat without changing the data.
                assert_eq!(
                    pending,
                    Poll::Ready(Some(Ok(Bytes::from_static(SSE_HEARTBEAT))))
                );
                assert!(parse_sse_event(&frame[..split]).valid);
            } else {
                assert!(pending.is_pending(), "heartbeat inserted at byte {split}");
            }
            sender
                .send(Ok(Bytes::copy_from_slice(&frame[split..])))
                .unwrap();
            let last = stream.next().await.unwrap().unwrap();
            assert_eq!([first.as_ref(), last.as_ref()].concat(), frame);
            stream
                .heartbeat
                .as_mut()
                .reset(TokioInstant::now() - Duration::from_secs(1));
            assert_eq!(stream.next().await.unwrap().unwrap(), SSE_HEARTBEAT);
        }
    }
}

#[tokio::test(start_paused = true)]
async fn quiet_stream_keeps_sending_heartbeats_until_provider_completion() {
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    let input = stream::unfold(receiver, |mut receiver| async move {
        receiver.recv().await.map(|chunk| (chunk, receiver))
    });
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut stream = usage_stream_with_events(input, events.clone());
    let first = Bytes::from_static(
        b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"synthetic\"}\n\n",
    );
    sender.send(Ok(first.clone())).unwrap();
    assert_eq!(stream.next().await.unwrap().unwrap(), first);

    for _ in 0..3 {
        tokio::time::advance(Duration::from_secs(20 * 60)).await;
        assert_eq!(stream.next().await.unwrap().unwrap(), SSE_HEARTBEAT);
        assert!(events.lock().unwrap().is_empty());
        assert!(!stream.terminated);
    }

    let completed = Bytes::from_static(
        b"data: {\"type\":\"response.completed\",\"response\":{\"id\":\"slow-response\",\"usage\":{\"input_tokens\":1,\"output_tokens\":2,\"total_tokens\":3}}}\n\n",
    );
    sender.send(Ok(completed.clone())).unwrap();
    assert_eq!(stream.next().await.unwrap().unwrap(), completed);
    assert!(stream.next().await.is_none());
    drop(stream);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(events[0].success);
    assert_eq!(events[0].total_tokens, Some(3));
    assert_eq!(events[0].cached_input_tokens, None);
}

#[tokio::test]
async fn bootstrap_rejects_a_degraded_served_model_before_output_is_committed() {
    let event = concat!(
        "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_test\",\"model\":\"gpt-6-astra-degrade2-luna\"}}\n\n",
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hidden\"}\n\n"
    );
    let (upstream, server) = response_from_sse_event(event.into()).await;
    let failure = bootstrap_stream(upstream, Some("gpt-6-astra"))
        .await
        .err()
        .expect("degraded model must not commit client output");
    server.await.unwrap();

    assert_eq!(failure.failure.category, "upstream_route_degraded");
    assert_eq!(failure.failure.status, StatusCode::NOT_FOUND);
    assert_eq!(
        failure.execution.certainty,
        crate::scheduler::rotation::ExecutionCertainty::NotSent
    );
}

#[tokio::test]
async fn bootstrap_forwards_a_degraded_model_when_blocking_is_off() {
    let event = concat!(
        "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_test\",\"model\":\"gpt-6-astra-degrade2-luna\"}}\n\n",
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"visible\"}\n\n"
    );
    let (upstream, server) = response_from_sse_event(event.into()).await;
    let opened = bootstrap_stream(upstream, None).await;
    server.await.unwrap();
    let (_, buffered, _) = match opened {
        Ok(opened) => opened,
        Err(failure) => panic!(
            "blocking off keeps the upstream stream, got {}",
            failure.failure.category
        ),
    };
    assert!(buffered.windows(7).any(|window| window == b"visible"));
}

#[tokio::test]
async fn bootstrap_accepts_the_requested_model_name() {
    let event = concat!(
        "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_test\",\"model\":\"gpt-6-astra\"}}\n\n",
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"ok\"}\n\n"
    );
    let (upstream, server) = response_from_sse_event(event.into()).await;
    let result = bootstrap_stream(upstream, Some("gpt-6-astra")).await;
    server.await.unwrap();
    assert!(result.is_ok(), "a normal served model is not a downgrade");
}

#[tokio::test]
async fn bootstrap_checks_model_identity_before_the_first_output() {
    for (served, expected, rejected) in [
        ("gpt-5.6-luna", Some("gpt-6-astra"), true),
        ("gpt-6-astra-2026-09-04", Some("gpt-6-astra"), false),
        ("", Some("gpt-6-astra"), false),
        ("gpt-5.6-luna", None, false),
    ] {
        let event = format!(
            "data: {{\"type\":\"response.created\",\"response\":{{\"model\":\"{served}\"}}}}\n\ndata: {{\"type\":\"response.output_text.delta\",\"delta\":\"synthetic\"}}\n\n"
        );
        let (upstream, server) = response_from_sse_event(event).await;
        let result = bootstrap_stream(upstream, expected).await;
        server.await.unwrap();
        if rejected {
            let failure = result.err().expect("model substitution must be rejected");
            assert_eq!(failure.failure.category, "upstream_route_degraded");
            assert_eq!(
                failure.execution.certainty,
                crate::scheduler::rotation::ExecutionCertainty::NotSent
            );
        } else {
            assert!(result.is_ok(), "served={served}, expected={expected:?}");
        }
    }
}

#[tokio::test]
async fn buffered_basis_points_rejects_created_without_waiting_for_the_body() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let address = listener.local_addr().unwrap();
    let (release, wait) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = [0_u8; 1024];
        assert!(socket.read(&mut request).await.unwrap() > 0);
        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\ndata: {\"type\":\"response.created\",\"response\":{\"model\":\"gpt-5.6-luna\"}}\n\n").await.unwrap();
        let _ = wait.await;
    });
    let upstream = reqwest::get(format!("http://{address}/stream"))
        .await
        .unwrap();
    let result = tokio::time::timeout(
        Duration::from_secs(3),
        crate::gateway::response::collect_upstream_response(upstream, true, Some("gpt-6-astra")),
    )
    .await;
    let _ = release.send(());
    server.await.unwrap();
    let failure = result
        .expect("must reject before upstream completes")
        .unwrap_err();
    assert_eq!(
        failure.failure.category,
        error_codes::UPSTREAM_ROUTE_DEGRADED
    );
    assert_eq!(
        failure.execution.certainty,
        crate::scheduler::rotation::ExecutionCertainty::NotSent
    );
}

#[tokio::test]
async fn bootstrap_does_not_replay_a_late_model_mismatch_after_generated_output() {
    let event = concat!(
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"synthetic\"}\n\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"model\":\"gpt-5.6-luna\"}}\n\n"
    );
    let (upstream, server) = response_from_sse_event(event.into()).await;
    let failure = bootstrap_stream(upstream, Some("gpt-6-astra"))
        .await
        .err()
        .unwrap();
    server.await.unwrap();
    assert_eq!(failure.failure.category, "upstream_route_degraded");
    assert_eq!(
        failure.execution.certainty,
        crate::scheduler::rotation::ExecutionCertainty::Accepted
    );
}
