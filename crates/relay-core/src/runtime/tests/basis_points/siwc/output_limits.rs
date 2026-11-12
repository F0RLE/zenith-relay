use super::*;

fn gemini_path(stream: bool) -> &'static str {
    if stream {
        "/v1beta/models/gpt-test:streamGenerateContent?alt=sse"
    } else {
        "/v1beta/models/gpt-test:generateContent"
    }
}

#[tokio::test]
async fn siwc_null_output_limit_accepts_common_pool_ingress() {
    for stream in [false, true] {
        for (path, mut request) in [
            (
                "/v1/responses",
                json!({"input":"Synthetic input","max_output_tokens":null}),
            ),
            (
                "/v1/chat/completions",
                json!({"messages":[{"role":"user","content":"Synthetic input"}],
                    "max_completion_tokens":null}),
            ),
            (
                "/v1/chat/completions",
                json!({"messages":[{"role":"user","content":"Synthetic input"}],
                    "max_tokens":null}),
            ),
            (
                gemini_path(stream),
                json!({"contents":[{"role":"user","parts":[{"text":"Synthetic input"}]}],
                    "generationConfig":{"maxOutputTokens":null}}),
            ),
        ] {
            let siwc = AccessServer::start(json!([]), false).await;
            let native = AccessServer::start(json!([]), false).await;
            siwc.state.stream_generations.store(true, Ordering::SeqCst);
            *crate::poison::mutex(&siwc.state.generation_body) = json::completed_response();
            let events = Arc::new(Mutex::new(Vec::<crate::UsageEvent>::new()));
            let captured = events.clone();
            let finished = Arc::new(Notify::new());
            let notified = finished.clone();
            let mut runtime = runtime(&siwc, &native).await;
            {
                let mut scheduler = runtime.lock_scheduler();
                let mut excluded = scheduler.candidate("synthetic-account-1").unwrap().clone();
                excluded.model_rules.excluded.insert("gpt-test".into());
                scheduler.upsert(excluded);
            }
            let settings = Arc::get_mut(&mut runtime).unwrap();
            settings.diagnostics_enabled = || true;
            settings.usage = Arc::new(move |event| {
                crate::poison::mutex(&captured).push(event);
                notified.notify_one();
            });
            let (base, task) = recovery::gateway(runtime).await;
            request["model"] = json!("gpt-test");
            if !path.starts_with("/v1beta/") {
                request["stream"] = json!(stream);
            }
            let response = retry_admission::bounded_client()
                .post(format!("{base}{path}"))
                .bearer_auth("local-secret")
                .json(&request)
                .send()
                .await
                .unwrap();
            let status = response.status();
            let text = response.text().await.unwrap();
            assert_eq!(status, StatusCode::OK, "{path} stream={stream}: {text}");
            assert!(text.contains("Synthetic JSON answer"), "{text}");
            tokio::time::timeout(Duration::from_secs(3), finished.notified())
                .await
                .unwrap();
            let bodies = crate::poison::mutex(&siwc.state.websocket_bodies);
            assert_eq!(bodies.len(), 1);
            assert!(bodies[0].get("max_output_tokens").is_none());
            assert_eq!(native.state.generation_requests.load(Ordering::SeqCst), 0);
            assert_eq!(
                siwc.state.http_generation_requests.load(Ordering::SeqCst),
                0
            );
            let events = crate::poison::mutex(&events);
            let event = events.iter().find(|event| event.success).unwrap();
            assert_eq!(event.account_id.as_deref(), Some("synthetic-account-0"));
            assert_eq!(
                event
                    .routing
                    .as_ref()
                    .and_then(|routing| routing.transport.as_ref())
                    .and_then(|transport| transport.request_preparation.as_ref())
                    .and_then(|preparation| preparation.siwc_output_limit_omitted),
                None
            );
            task.abort();
        }
    }
}

#[tokio::test]
async fn siwc_gemini_output_limit_is_omitted_by_default() {
    for stream in [false, true] {
        let siwc = AccessServer::start(json!([]), false).await;
        let native = AccessServer::start(json!([]), false).await;
        siwc.state.stream_generations.store(true, Ordering::SeqCst);
        *crate::poison::mutex(&siwc.state.generation_body) = json::completed_response();
        let runtime = runtime(&siwc, &native).await;
        {
            let mut scheduler = runtime.lock_scheduler();
            let mut excluded = scheduler.candidate("synthetic-account-1").unwrap().clone();
            excluded.model_rules.excluded.insert("gpt-test".into());
            scheduler.upsert(excluded);
        }
        let (base, task) = recovery::gateway(runtime).await;
        let response = retry_admission::bounded_client()
            .post(format!("{base}{}", gemini_path(stream)))
            .bearer_auth("local-secret")
            .json(
                &json!({"contents":[{"role":"user","parts":[{"text":"Synthetic input"}]}],
                "generationConfig":{"maxOutputTokens":1024}}),
            )
            .send()
            .await
            .unwrap();
        let status = response.status();
        let text = response.text().await.unwrap();
        assert_eq!(status, StatusCode::OK, "stream={stream}: {text}");
        assert!(text.contains("Synthetic JSON answer"), "{text}");
        assert_eq!(native.state.generation_requests.load(Ordering::SeqCst), 0);
        assert_eq!(
            siwc.state.http_generation_requests.load(Ordering::SeqCst),
            0
        );
        let bodies = crate::poison::mutex(&siwc.state.websocket_bodies);
        assert_eq!(bodies.len(), 1);
        assert!(bodies[0].get("max_output_tokens").is_none());
        task.abort();
    }
}
