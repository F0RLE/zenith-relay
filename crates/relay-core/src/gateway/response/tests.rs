use super::*;
use crate::gateway::test_support::test_usage_event;

#[test]
fn response_cache_policy_is_observed_outside_usage_for_json_and_sse() {
    for ttl in ["30m", "12h"] {
        let response = serde_json::json!({
            "status":"completed", "prompt_cache_options":{"mode":"implicit","ttl":ttl},
            "usage":{"input_tokens":100, "input_tokens_details":{"cached_tokens":50,"cache_write_tokens":40},"output_tokens":10}
        });
        for streaming in [false, true] {
            let body = if streaming {
                let envelope = serde_json::json!({"type":"response.completed","response":response});
                format!("data: {envelope}\n\n").into_bytes()
            } else {
                serde_json::to_vec(&response).unwrap()
            };
            let mut event = test_usage_event();
            populate_tokens(&mut event, &body);
            assert_eq!(event.cache_write_input_tokens, Some(40));
            assert_eq!(event.cache_write_ttl.as_deref(), Some(ttl));
        }
    }
    let mut event = test_usage_event();
    populate_tokens(&mut event, br#"{"model":"gpt-6.1-sol","usage":{"input_tokens":100,"input_tokens_details":{"cache_write_tokens":40}}}"#);
    assert_eq!(
        event.cache_write_ttl, None,
        "a model name does not prove the applied cache window"
    );
}
use axum::body::Body;
use axum::http::HeaderValue;
use axum::http::header::CONTENT_TYPE;

#[test]
fn generation_evidence_uses_protocol_usage_without_inventing_a_first_token() {
    for payload in [
        serde_json::json!({"usage":{"output_tokens":7}}),
        serde_json::json!({"response":{"usage":{"output_tokens_details":{"reasoning_tokens":7}}}}),
        serde_json::json!({"message":{"usage":{"output_tokens":7}}}),
        serde_json::json!({"usage":{"completion_tokens":7}}),
        serde_json::json!({"usage":{"completion_tokens_details":{"reasoning_tokens":7}}}),
        serde_json::json!({"usageMetadata":{"candidatesTokenCount":7}}),
        serde_json::json!({"usageMetadata":{"candidatesTokenCount":0,"thoughtsTokenCount":7}}),
    ] {
        assert!(has_generated_tokens(&payload));
        assert!(!crate::gateway::streaming::has_output_delta(&payload, None));
    }
    for usage in [
        serde_json::json!({"input_tokens":50,"input_tokens_details":{"cached_tokens":20,"cache_write_tokens":30}}),
        serde_json::json!({"output_tokens":0,"output_tokens_details":{"reasoning_tokens":0}}),
        serde_json::json!({"output_tokens":null}),
        serde_json::json!({"output_tokens":-1}),
        serde_json::json!({"output_tokens":"7"}),
        serde_json::json!({}),
    ] {
        assert!(!has_generated_tokens(&serde_json::json!({"usage":usage})));
    }
}

#[test]
fn generated_usage_prevents_replaying_json_and_buffered_stream_failures() {
    for usage in [
        serde_json::json!({"output_tokens": 7}),
        serde_json::json!({"output_tokens": 0, "output_tokens_details": {"reasoning_tokens": 7}}),
        serde_json::json!({"completion_tokens": 7}),
        serde_json::json!({"reasoning_tokens": 7}),
    ] {
        let response = serde_json::json!({"status":"failed","output":[],"usage":usage,
            "error":{"code":"model_capacity_exhausted","message":"Synthetic capacity refusal"}
        });
        let failure = validate_upstream_response(&response, None).err().unwrap();
        assert_eq!(
            failure.execution.certainty,
            crate::scheduler::rotation::ExecutionCertainty::Accepted
        );
        let terminal = serde_json::json!({"type":"response.failed","response":response});
        let failure =
            completed_upstream_response(format!("data: {terminal}\n\n").as_bytes(), true, None)
                .err()
                .unwrap();
        assert_eq!(
            failure.execution.certainty,
            crate::scheduler::rotation::ExecutionCertainty::Accepted
        );
    }
}

#[test]
fn buffered_timing_ignores_lifecycle_empty_invalid_and_terminal_snapshots() {
    let mut timing = BufferedResponseTiming::default();
    for bytes in [
        b": heartbeat\n\n".as_slice(),
        b"data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_test\"}}\n\n",
        b"data: {\"type\":\"response.in_progress\"}\n\n",
        b"data: {\"type\":\"response.in_progress\",\"response\":{\"usage\":{\"output_tokens\":7}}}\n\n",
        b"data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"signature_delta\",\"signature\":\"opaque\"}}\n\n",
        b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"\"}\n\n",
        b"data: {\"type\":\"response.output_item.added\",\"item\":{\"type\":\"message\",\"id\":\"msg_test\"}}\n\n",
        b"data: {\"type\":\"response.compaction.delta\",\"delta\":\"opaque\"}\n\n",
        b"data: invalid\n\n",
        b"data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[{\"type\":\"message\",\"content\":[{\"type\":\"output_text\",\"text\":\"synthetic\"}]}]}}\n\n",
    ] {
        timing.observe(&crate::gateway::streaming::parse_sse_event(bytes), 50);
    }
    timing.finish(120);
    let mut event = test_usage_event();
    timing.apply(&mut event);
    assert_eq!(event.latency_ms, 120);
    assert_eq!(event.ttft_ms, None);
    assert_eq!(event.generation_ms, None);
}

#[test]
fn buffered_timing_keeps_first_output_through_completion_and_failure() {
    for output in [
        serde_json::json!({"type":"response.output_text.delta","delta":"synthetic"}),
        serde_json::json!({"type":"response.reasoning_text.delta","delta":"synthetic"}),
        serde_json::json!({"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"synthetic"}}),
        serde_json::json!({"type":"response.function_call_arguments.delta","delta":"{}"}),
        serde_json::json!({"type":"response.output_item.added","item":{"type":"function_call","call_id":"call_test","name":"test"}}),
    ] {
        for success in [false, true] {
            let mut timing = BufferedResponseTiming::default();
            let bytes = format!("data: {output}\n\n");
            let parsed = crate::gateway::streaming::parse_sse_event(bytes.as_bytes());
            timing.observe(&parsed, 50);
            timing.observe(&parsed, 80);
            timing.finish(120);
            let mut event = test_usage_event();
            event.success = success;
            timing.apply(&mut event);
            assert_eq!(event.ttft_ms, Some(50));
            assert_eq!(event.latency_ms, 120);
            assert_eq!(event.generation_ms, Some(70));
            assert_eq!(event.success, success);
        }
    }
}

#[test]
fn buffered_timing_preserves_unobserved_transports_and_known_zero() {
    let mut event = test_usage_event();
    event.routing = Some(
        serde_json::from_value(serde_json::json!({
            "reason":"only_eligible", "eligibleCandidates":1,
            "inFlightBefore":0, "dispatchesBefore":0,
            "transport":{
                "upstream":"http", "mode":"basis_points", "requestPreparationMs":0,
                "clientFirstOutputMs":60, "clientResponseReadyMs":130
            }
        }))
        .unwrap(),
    );
    event.latency_ms = 120;
    event.ttft_ms = Some(30);
    event.generation_ms = Some(90);
    BufferedResponseTiming::default().apply(&mut event);
    assert_eq!(event.latency_ms, 120);
    assert_eq!(event.ttft_ms, Some(30));
    assert_eq!(event.generation_ms, Some(90));
    let transport = event.routing.as_ref().unwrap().transport.as_ref().unwrap();
    assert_eq!(transport.upstream_response_ms, None);

    let mut timing = BufferedResponseTiming::default();
    timing.observe(
        &crate::gateway::streaming::parse_sse_event(
            b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"synthetic\"}\n\n",
        ),
        0,
    );
    timing.finish(120);
    timing.apply(&mut event);
    assert_eq!(event.ttft_ms, Some(0));
    assert_eq!(event.generation_ms, Some(120));
    let transport = event.routing.as_ref().unwrap().transport.as_ref().unwrap();
    assert_eq!(transport.upstream_response_ms, Some(120));
    assert_eq!(transport.request_preparation_ms, Some(0));
    assert_eq!(transport.client_first_output_ms, Some(60));
    assert_eq!(transport.client_response_ready_ms, Some(130));
}

#[tokio::test]
async fn basis_points_collector_times_fragmented_output_and_keeps_partial_failures() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    for outcome in ["completed", "failed", "truncated", "json"] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 1024];
            assert!(socket.read(&mut request).await.unwrap() > 0);
            let response = serde_json::json!({
                "id":"resp_test", "model":"gpt-6-astra", "status":"completed",
                "output":[{"type":"message","content":[{"type":"output_text","text":"synthetic"}]}],
                "usage":{"input_tokens":4,"output_tokens":2,"total_tokens":6}
            });
            if outcome == "json" {
                let body = response.to_string();
                socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
                return;
            }
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n").await.unwrap();
            let terminal = if outcome == "failed" {
                "data: {\"type\":\"response.failed\",\"response\":{\"status\":\"failed\",\"error\":{\"code\":\"synthetic_failure\",\"message\":\"Synthetic failure\"}}}\n\n".to_string()
            } else {
                format!("data: {{\"type\":\"response.completed\",\"response\":{response}}}\n\n")
            };
            for frame in [
                ": heartbeat\n\ndata: {\"type\":\"response.created\"}\n\n".to_string(),
                "data: {\"type\":\"response.output_text.delta\",\"delta\":\"syn".to_string(),
                "thetic\"}\n\n".to_string(),
            ] {
                socket
                    .write_all(format!("{:x}\r\n{frame}\r\n", frame.len()).as_bytes())
                    .await
                    .unwrap();
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            if outcome == "truncated" {
                return;
            }
            socket
                .write_all(format!("{:x}\r\n{terminal}\r\n", terminal.len()).as_bytes())
                .await
                .unwrap();
            std::future::pending::<()>().await;
        });
        let started = std::time::Instant::now();
        let upstream = reqwest::get(format!("http://{address}/")).await.unwrap();
        let mut timing = BufferedResponseTiming::default();
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            collect_basis_points_response(upstream, Some("gpt-6-astra"), started, &mut timing),
        )
        .await;
        server.abort();
        let result = result.expect("collector must stop without waiting for EOF");
        let bytes = match result {
            Ok(bytes) => {
                assert!(matches!(outcome, "completed" | "json"));
                bytes
            }
            Err(failure) => {
                assert!(matches!(outcome, "failed" | "truncated"));
                failure.partial_body
            }
        };
        let mut event = test_usage_event();
        event.latency_ms = u64::MAX;
        timing.apply(&mut event);
        assert_ne!(event.latency_ms, u64::MAX);
        if outcome == "json" {
            assert_eq!(event.ttft_ms, None);
            assert_eq!(event.generation_ms, None);
        } else {
            let first = event.ttft_ms.expect("complete output delta was observed");
            assert!(first <= event.latency_ms);
            assert_eq!(event.generation_ms, Some(event.latency_ms - first));
            assert!(String::from_utf8_lossy(&bytes).contains("synthetic"));
        }
    }
}

#[test]
fn buffered_json_and_account_stream_keep_original_failure_details() {
    for (body, account_stream, expected_code, expected_message) in [
        (
            br#"{"status":"failed","error":{"code":"future_constraint","message":"Constraint check failed"},"input":"synthetic-private"}"#.as_slice(),
            false,
            "future_constraint",
            "Constraint check failed",
        ),
        (
            br#"{"status":"cancelled","error":{"code":"client_cancelled","message":"Request was cancelled"}}"#.as_slice(),
            false,
            "client_cancelled",
            "Request was cancelled",
        ),
        (
            b"data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"future_constraint\",\"message\":\"Constraint check failed\"}}}\n\n".as_slice(),
            true,
            "future_constraint",
            "Constraint check failed",
        ),
    ] {
        let failure = completed_upstream_response(body, account_stream, None).unwrap_err();
        let details = failure.upstream_error.unwrap();
        assert_eq!(details.code.as_deref(), Some(expected_code));
        assert_eq!(details.message.as_deref(), Some(expected_message));
        if expected_code == "future_constraint" {
            assert!(!serde_json::to_string(&details).unwrap().contains("synthetic-private"));
            assert_eq!(failure.preserved.unwrap().message, expected_message);
        }
    }
}

#[test]
fn non_stream_usage_preserves_reported_cache_reasoning_and_total_tokens() {
    let mut event = test_usage_event();
    populate_tokens(
        &mut event,
        br#"{"response":{"response":{"service_tier":"priority","usage":{"input_tokens":16,"input_tokens_details":{"cached_tokens":30},"output_tokens":5,"output_tokens_details":{"reasoning_tokens":30},"total_tokens":10}}}}"#,
    );

    assert_eq!(event.input_tokens, Some(16));
    assert_eq!(event.cached_input_tokens, Some(30));
    assert_eq!(event.reasoning_tokens, Some(30));
    assert_eq!(event.output_tokens, Some(5));
    assert_eq!(event.total_tokens, Some(10));
    assert_eq!(event.applied_service_tier, Some("priority".to_string()));
}

#[test]
fn truncated_stream_keeps_usage_from_complete_frames_without_marking_success() {
    let mut event = test_usage_event();
    event.success = false;
    populate_tokens(
        &mut event,
        b"data: {\"type\":\"response.created\",\"response\":{\"usage\":{\"input_tokens\":17,\"input_tokens_details\":{\"cached_tokens\":5},\"total_tokens\":17}}}\n\n\
          data: {\"type\":\"response.in_progress\",\"response\":{\"usage\":{\"output_tokens\":3,\"output_tokens_details\":{\"reasoning_tokens\":1}}}}\n\n\
          data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":999",
    );
    assert_eq!(event.input_tokens, Some(17));
    assert_eq!(event.cached_input_tokens, Some(5));
    assert_eq!(event.output_tokens, Some(3));
    assert_eq!(event.reasoning_tokens, Some(1));
    assert_eq!(event.total_tokens, Some(20));
    assert!(!event.success);
}

#[test]
fn partial_usage_updates_replace_stale_totals_and_keep_known_counters() {
    use serde_json::json;

    for (first, second, unknown, expected_input, expected_output) in [
        (
            json!({"usage":{"input_tokens":3,"output_tokens":0,"total_tokens":3}}),
            json!({"usage":{"output_tokens":2}}),
            json!({"usage":{"input_tokens":null,"output_tokens":null,"total_tokens":null}}),
            3,
            2,
        ),
        (
            json!({"usage":{"prompt_tokens":3,"completion_tokens":0,"total_tokens":3}}),
            json!({"usage":{"completion_tokens":2}}),
            json!({"usage":{"prompt_tokens":null,"completion_tokens":null,"total_tokens":null}}),
            3,
            2,
        ),
        (
            json!({"message":{"usage":{"input_tokens":3,"output_tokens":0,
                "cache_read_input_tokens":5,"cache_creation_input_tokens":2,"total_tokens":10}}}),
            json!({"usage":{"output_tokens":2}}),
            json!({"usage":{"input_tokens":null,"output_tokens":null,"cache_read_input_tokens":null,
                "cache_creation_input_tokens":null,"total_tokens":null}}),
            10,
            2,
        ),
        (
            json!({"usageMetadata":{"promptTokenCount":3,"candidatesTokenCount":0,
                "thoughtsTokenCount":1,"totalTokenCount":4}}),
            json!({"usageMetadata":{"candidatesTokenCount":2}}),
            json!({"usageMetadata":{"promptTokenCount":null,"candidatesTokenCount":null,
                "thoughtsTokenCount":null,"totalTokenCount":null}}),
            3,
            3,
        ),
    ] {
        let frames = format!("data: {first}\n\ndata: {second}\n\ndata: {unknown}\n\n");
        let mut event = test_usage_event();
        populate_tokens(&mut event, frames.as_bytes());
        assert_eq!(event.input_tokens, Some(expected_input), "{first}");
        assert_eq!(event.output_tokens, Some(expected_output), "{first}");
        assert_eq!(
            event.total_tokens,
            Some(expected_input + expected_output),
            "{first}"
        );
    }
}

#[test]
fn usage_overflow_does_not_reuse_an_older_total() {
    let mut event = test_usage_event();
    let frames = format!(
        "data: {{\"usage\":{{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}}\n\n\
         data: {{\"usage\":{{\"input_tokens\":{}}}}}\n\n",
        u64::MAX
    );
    populate_tokens(&mut event, frames.as_bytes());
    assert_eq!(event.input_tokens, Some(u64::MAX));
    assert_eq!(event.output_tokens, Some(1));
    assert_eq!(event.total_tokens, None);
}

#[test]
fn response_service_tier_preserves_upstream_text_and_prefers_nested_response() {
    assert_eq!(
        response_service_tier(&serde_json::json!({"service_tier": "flex"})),
        Some("flex".to_string())
    );
    assert_eq!(
        response_service_tier(
            &serde_json::json!({"service_tier": "default", "response": {"service_tier": "ultrafast"}})
        ),
        Some("ultrafast".to_string())
    );
    assert_eq!(response_service_tier(&serde_json::json!({})), None);
    assert_eq!(
        response_service_tier(&serde_json::json!({"service_tier": "not a tier"})),
        None
    );
}

#[test]
fn gemini_cached_input_and_thoughts_are_counted_once_for_json_and_sse() {
    let body = serde_json::json!({"usageMetadata":{
        "promptTokenCount":100,"cachedContentTokenCount":60,
        "candidatesTokenCount":6,"thoughtsTokenCount":4,"totalTokenCount":110
    }});
    for payload in [body.to_string(), format!("data: {body}\n\n")] {
        let mut event = test_usage_event();
        populate_tokens(&mut event, payload.as_bytes());
        assert_eq!(event.input_tokens, Some(100));
        assert_eq!(event.cached_input_tokens, Some(60));
        assert_eq!(event.cache_write_input_tokens, None);
        assert_eq!(event.output_tokens, Some(10));
        assert_eq!(event.reasoning_tokens, Some(4));
        assert_eq!(event.total_tokens, Some(110));
    }
}

#[test]
fn deepseek_cache_hits_are_part_of_prompt_tokens_not_additional_input() {
    let mut event = test_usage_event();
    populate_tokens(
        &mut event,
        br#"{"usage":{"prompt_tokens":100,"prompt_cache_hit_tokens":60,"prompt_cache_miss_tokens":40,"completion_tokens":10,"total_tokens":110}}"#,
    );
    assert_eq!(event.input_tokens, Some(100));
    assert_eq!(event.cached_input_tokens, Some(60));
    assert_eq!(event.cache_write_input_tokens, None);
    assert_eq!(event.output_tokens, Some(10));
    assert_eq!(event.total_tokens, Some(110));

    // A measured zero in the standard field must not fall back to an alias.
    populate_tokens(
        &mut event,
        br#"{"usage":{"prompt_tokens":100,"prompt_tokens_details":{"cached_tokens":0},"prompt_cache_hit_tokens":60,"completion_tokens":10}}"#,
    );
    assert_eq!(event.cached_input_tokens, Some(0));
}

#[test]
fn anthropic_usage_adds_cache_read_and_creation_to_total_input() {
    let mut event = test_usage_event();
    populate_tokens(
        &mut event,
        br#"{"usage":{"input_tokens":100,"cache_read_input_tokens":40,"cache_creation_input_tokens":20,"output_tokens":10}}"#,
    );

    assert_eq!(event.input_tokens, Some(160));
    assert_eq!(event.cached_input_tokens, Some(40));
    assert_eq!(event.cache_write_input_tokens, Some(20));
    assert_eq!(event.output_tokens, Some(10));
    assert_eq!(event.total_tokens, Some(170));
}

#[test]
fn anthropic_cache_creation_reports_actual_write_lifetime() {
    let mut event = test_usage_event();
    populate_tokens(
        &mut event,
        br#"{"usage":{"input_tokens":100,"cache_creation_input_tokens":20,"cache_creation":{"ephemeral_1h_input_tokens":20},"output_tokens":10}}"#,
    );
    assert_eq!(event.cache_write_ttl.as_deref(), Some("1h"));

    populate_tokens(
        &mut event,
        br#"{"usage":{"input_tokens":100,"cache_creation_input_tokens":20,"cache_creation":{"ephemeral_5m_input_tokens":20},"output_tokens":10}}"#,
    );
    assert_eq!(event.cache_write_ttl.as_deref(), Some("5m"));
}

#[test]
fn cache_usage_keeps_all_reported_provider_windows() {
    let mut event = test_usage_event();
    populate_tokens(
        &mut event,
        br#"{"usage":{"input_tokens":100,"cache_creation_input_tokens":20,"cache_creation":{"ephemeral_1h_input_tokens":10,"ephemeral_5m_input_tokens":10,"ephemeral_15m_input_tokens":0},"output_tokens":10}}"#,
    );
    assert_eq!(event.cache_write_ttl.as_deref(), Some("5m, 1h"));

    let mut event = test_usage_event();
    populate_tokens(
        &mut event,
        br#"{"usage":{"input_tokens":100,"cache_creation_input_tokens":20,"cache_write_ttl":"15m","output_tokens":10}}"#,
    );
    assert_eq!(event.cache_write_ttl.as_deref(), Some("15m"));
}

#[test]
fn cache_usage_accepts_generic_provider_window_fields() {
    let usage = serde_json::json!({
        "cache_creation_ttl": "2h",
        "cache_creation_tokens_5m": 2,
        "input_tokens_details": {
            "cache_write_tokens_30m": 5
        },
        "prompt_cache_ttl": "24h",
        "cache_creation_input_tokens": 7
    });

    assert_eq!(
        cache_write_ttl_from_usage(&usage).as_deref(),
        Some("5m, 30m, 2h")
    );
}

#[test]
fn cache_usage_keeps_unknown_windows_unreported() {
    let mut event = test_usage_event();
    populate_tokens(
        &mut event,
        br#"{"usage":{"input_tokens":100,"cache_creation_input_tokens":20,"output_tokens":10}}"#,
    );
    assert_eq!(event.cache_write_input_tokens, Some(20));
    assert_eq!(event.cache_write_ttl, None);
}

#[test]
fn cache_usage_keeps_provider_reported_ttl_without_write_counter() {
    let mut event = test_usage_event();
    populate_tokens(
        &mut event,
        br#"{"usage":{"input_tokens":100,"cache_write_ttl":"45m","output_tokens":10}}"#,
    );
    assert_eq!(event.cache_write_input_tokens, None);
    assert_eq!(event.cache_write_ttl.as_deref(), Some("45m"));
}

#[test]
fn proxy_keeps_safe_native_retry_and_request_headers_only() {
    let mut upstream = reqwest::header::HeaderMap::new();
    upstream.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    upstream.insert("retry-after", HeaderValue::from_static("12"));
    upstream.insert("request-id", HeaderValue::from_static("req_native"));
    upstream.insert(
        "x-codex-turn-state",
        HeaderValue::from_static("account-scoped-state"),
    );
    upstream.insert(
        "anthropic-ratelimit-requests-reset",
        HeaderValue::from_static("2026-08-02T00:00:00Z"),
    );
    upstream.insert("authorization", HeaderValue::from_static("Bearer secret"));
    upstream.insert("set-cookie", HeaderValue::from_static("session=secret"));
    upstream.insert("server", HeaderValue::from_static("provider-internal"));

    let response = proxy_response(
        reqwest::StatusCode::TOO_MANY_REQUESTS,
        &upstream,
        Body::empty(),
    );
    assert_eq!(response.headers()[CONTENT_TYPE], "application/json");
    assert_eq!(response.headers()["retry-after"], "12");
    assert_eq!(response.headers()["request-id"], "req_native");
    assert!(response.headers().get("x-codex-turn-state").is_none());
    assert_eq!(
        response.headers()["anthropic-ratelimit-requests-reset"],
        "2026-08-02T00:00:00Z"
    );
    assert!(response.headers().get("authorization").is_none());
    assert!(response.headers().get("set-cookie").is_none());
    assert!(response.headers().get("server").is_none());
}

#[test]
fn buffered_success_with_a_degraded_model_stays_retryable_only_while_blocking() {
    let body = br#"{"id":"resp_test","model":"gpt-6-astra-degrade2","output":[]}"#;
    let failure = completed_upstream_response(body, false, Some("gpt-6-astra")).unwrap_err();
    assert_eq!(failure.failure.category, "upstream_route_degraded");
    assert_eq!(
        failure.execution.certainty,
        crate::scheduler::rotation::ExecutionCertainty::NotSent
    );
    assert!(completed_upstream_response(body, false, None).is_ok());
}

#[test]
fn buffered_model_mismatch_is_rejected_but_generated_output_is_not_replayed() {
    for body in [
        br#"{"model":"gpt-5.6-luna","status":"completed","output":[{"type":"message","content":[{"type":"output_text","text":"synthetic"}]}]}"#.as_slice(),
        b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"synthetic\"}\n\ndata: {\"type\":\"response.completed\",\"response\":{\"model\":\"gpt-5.6-luna\",\"status\":\"completed\",\"output\":[{\"type\":\"message\",\"content\":[{\"type\":\"output_text\",\"text\":\"synthetic\"}]}]}}\n\n".as_slice(),
    ] {
        let failure = completed_upstream_response(body, true, Some("gpt-6-astra")).unwrap_err();
        assert_eq!(failure.failure.category, "upstream_route_degraded");
        assert_eq!(failure.execution.certainty, crate::scheduler::rotation::ExecutionCertainty::Accepted);
        assert!(completed_upstream_response(body, true, None).is_ok());
    }
}
#[tokio::test]
async fn buffered_account_collectors_stop_at_terminal_without_waiting_for_upstream_eof() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    for basis_points in [false, true] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 1024];
            assert!(socket.read(&mut request).await.unwrap() > 0);
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n").await.unwrap();
            let frame = concat!(
                "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_test\",\"model\":\"gpt-6-astra\",\"status\":\"completed\",\"output\":[]}}\n\n",
                "data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"ignored_tail\"}}}\n\n",
            );
            socket
                .write_all(format!("{:x}\r\n{frame}\r\n", frame.len()).as_bytes())
                .await
                .unwrap();
            std::future::pending::<()>().await;
        });
        let upstream = reqwest::get(format!("http://{address}/")).await.unwrap();
        let result = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            if basis_points {
                let mut timing = BufferedResponseTiming::default();
                let bytes = super::collect_basis_points_response(
                    upstream,
                    Some("gpt-6-astra"),
                    std::time::Instant::now(),
                    &mut timing,
                )
                .await;
                let mut event = test_usage_event();
                timing.apply(&mut event);
                assert_eq!(event.ttft_ms, None);
                assert_eq!(event.generation_ms, None);
                bytes
            } else {
                super::collect_upstream_response(upstream, true, Some("gpt-6-astra")).await
            }
        })
        .await;
        server.abort();
        let bytes = result
            .expect("terminal event must not wait for EOF")
            .unwrap_or_else(|failure| {
                panic!(
                    "unexpected buffered response failure: {}",
                    failure.failure.category
                )
            });
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["id"],
            "resp_test"
        );
    }
}

#[test]
fn failed_terminal_with_output_does_not_authorize_replay() {
    let payload = serde_json::json!({
        "type": "response.failed",
        "response": {
            "status": "failed",
            "error": {"code": "previous_response_not_found", "message": "previous response not found"},
            "output": [{"type": "message", "content": [{"type": "output_text", "text": "already generated"}]}]
        }
    });
    for bytes in [
        serde_json::to_vec(&payload).unwrap(),
        format!("data: {payload}\n\n").into_bytes(),
    ] {
        let failure = completed_upstream_response(&bytes, true, None).unwrap_err();
        assert_eq!(
            failure.failure.category,
            crate::error_codes::UPSTREAM_PREVIOUS_RESPONSE_NOT_FOUND
        );
        assert_eq!(
            failure.execution.certainty,
            crate::scheduler::rotation::ExecutionCertainty::Accepted
        );
    }
}
