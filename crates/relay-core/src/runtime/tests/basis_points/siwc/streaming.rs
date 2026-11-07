use super::*;

#[tokio::test]
async fn siwc_streams_native_and_converted_output_before_upstream_completion() {
    incremental_stream(Some("text/event-stream")).await;
}

#[tokio::test]
async fn siwc_streams_with_missing_or_mislabeled_sse_content_type() {
    for content_type in [None, Some("application/json"), Some("text/plain")] {
        incremental_stream(content_type).await;
    }
}

async fn incremental_stream(content_type: Option<&str>) {
    for (chat, debug) in [(false, false), (true, false), (false, true), (true, true)] {
        let siwc = AccessServer::start(json!([]), false).await;
        let native = AccessServer::start(json!([]), false).await;
        siwc.state.stream_generations.store(true, Ordering::SeqCst);
        *crate::poison::mutex(&siwc.state.stream_content_type) = content_type.map(str::to_owned);
        *crate::poison::mutex(&siwc.state.generation_body) = json!({
            "id":"resp_synthetic_stream","model":"gpt-test","status":"completed",
            "output":[{"id":"rs_synthetic_stream","type":"reasoning","status":"completed","summary":[]},
                {"id":"msg_synthetic_stream","type":"message","role":"assistant",
                "status":"completed","content":[{"type":"output_text",
                    "text":"Synthetic incremental answer","annotations":[]}]}],
            "usage":{"input_tokens":41,"output_tokens":7,"total_tokens":48}
        });
        let gate = Arc::new(Semaphore::new(0));
        crate::poison::mutex(&siwc.state.stream_gates).push_back(gate.clone());
        let (closed, on_close) = tokio::sync::oneshot::channel();
        crate::poison::mutex(&siwc.state.stream_closures).push_back(closed);
        let events = Arc::new(Mutex::new(Vec::<crate::UsageEvent>::new()));
        let captured = events.clone();
        let finished = Arc::new(Notify::new());
        let notified = finished.clone();
        let mut runtime = runtime(&siwc, &native).await;
        let settings = Arc::get_mut(&mut runtime).unwrap();
        settings.diagnostics_enabled = if debug { || true } else { || false };
        settings.usage = Arc::new(move |event| {
            crate::poison::mutex(&captured).push(event);
            notified.notify_one();
        });
        let (base, task) = recovery::gateway(runtime).await;
        let (path, request) = if chat {
            (
                "chat/completions",
                json!({"model":"gpt-test","stream":true,
                "messages":[{"role":"user","content":"Synthetic input"}]}),
            )
        } else {
            (
                "responses",
                json!({"model":"gpt-test","stream":true,"input":"Synthetic input"}),
            )
        };
        let mut response = retry_admission::bounded_client()
            .post(format!("{base}/v1/{path}"))
            .bearer_auth("local-secret")
            .json(&request)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["content-type"], "text/event-stream");
        let prefix = tokio::time::timeout(Duration::from_secs(3), async {
            let mut text = String::new();
            while !text.contains("Synthetic incremental answer") {
                let chunk = response
                    .chunk()
                    .await
                    .unwrap()
                    .expect("output before terminal");
                text.push_str(std::str::from_utf8(&chunk).unwrap());
            }
            text
        })
        .await
        .expect("client output must arrive while upstream completion is blocked");
        assert_eq!(gate.available_permits(), 0);
        assert!(!prefix.contains("response.completed"));
        assert!(!prefix.contains("\"finish_reason\":\"stop\""));
        assert!(crate::poison::mutex(&events).is_empty());

        gate.add_permits(1);
        let text = prefix
            + &tokio::time::timeout(Duration::from_secs(3), response.text())
                .await
                .expect("terminal event must finish without upstream EOF")
                .unwrap();
        assert!(text.contains(if chat { "[DONE]" } else { "response.completed" }));
        tokio::time::timeout(Duration::from_secs(3), finished.notified())
            .await
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_secs(3), on_close)
                .await
                .unwrap()
                .is_err()
        );
        let events = crate::poison::mutex(&events);
        let attempts: Vec<_> = events
            .iter()
            .filter(|event| !event.is_request_completion())
            .collect();
        assert_eq!(attempts.len(), 1);
        let event = attempts[0];
        assert!(event.success);
        let first = event.ttft_ms.expect("observed upstream output has TTFT");
        assert!(first <= event.latency_ms);
        assert_eq!(event.generation_ms, Some(event.latency_ms - first));
        let transport = event
            .routing
            .as_ref()
            .and_then(|routing| routing.transport.as_ref());
        if debug {
            let reasoning = transport
                .unwrap()
                .stream_response
                .as_ref()
                .unwrap()
                .upstream_reasoning_first_ms
                .expect("typed empty reasoning establishes activity");
            assert!(reasoning <= first);
        } else {
            assert!(
                transport.is_none(),
                "disabled diagnostics must not collect reasoning timing"
            );
        }
        assert_eq!(event.input_tokens, Some(41));
        assert_eq!(event.output_tokens, Some(7));
        assert_eq!(siwc.state.generation_requests.load(Ordering::SeqCst), 1);
        assert_eq!(native.state.generation_requests.load(Ordering::SeqCst), 0);
        task.abort();
    }
}
