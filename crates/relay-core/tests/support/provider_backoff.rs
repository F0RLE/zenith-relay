use super::*;
use std::time::{SystemTime, UNIX_EPOCH};

#[tokio::test]
async fn usage_distinguishes_provider_deadline_from_inferred_and_configured_backoff() {
    for explicit in [false, true] {
        let rejection = json!({"error":{"code":"rate_limit_exceeded","message":"Synthetic limit"}});
        let first = if explicit {
            Reply::JsonWithHeaders(
                StatusCode::TOO_MANY_REQUESTS,
                rejection,
                vec![("retry-after", "30")],
            )
        } else {
            Reply::Json(StatusCode::TOO_MANY_REQUESTS, rejection)
        };
        let (upstream, state) = spawn_upstream(vec![
            first,
            Reply::Json(
                StatusCode::OK,
                json!({
                    "id":"synthetic-backoff-completed","status":"completed","output":[],
                    "usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}
                }),
            ),
        ])
        .await;
        let mut sources = vec![
            source("synthetic-source-a", &upstream, "synthetic-a", 10),
            source("synthetic-source-b", &upstream, "synthetic-b", 10),
        ];
        for source in &mut sources {
            source.recovery_delay_seconds = 60;
        }
        let (gateway, events, _, _) = spawn_mixed_gateway(
            sources,
            Vec::new(),
            vec![mixed_key(None, None)],
            Arc::new(TokenAuthority::new(4).unwrap()),
            refresh_adapter(),
            Arc::new(PersistenceAdapter::default()),
        )
        .await;
        let response = reqwest::Client::new()
            .post(format!("{}/v1/responses", gateway.base_url))
            .bearer_auth(LOCAL_KEY)
            .json(&json!({"model":MODEL,"input":"synthetic request"}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let _ = response.bytes().await.unwrap();
        assert_eq!(state.requests.lock().unwrap().len(), 2);
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].request_id, events[1].request_id);
        let first = &events[0];
        let routing = first.routing.as_ref().unwrap();
        assert!(!first.success);
        assert_eq!(first.cooldown_scope.as_deref(), Some(MODEL));
        let encoded = serde_json::to_value(routing).unwrap();
        if explicit {
            let provider = routing.provider_backoff.as_ref().unwrap();
            assert_eq!(provider.scope, MODEL);
            let observed_at_ms = provider.observed_at_ms.unwrap();
            assert_eq!(provider.not_before_ms - observed_at_ms, 30_000);
            // The provider clock starts at normalization; the local recovery
            // floor starts at settlement, which can occur later.
            let checked_at_ms = u64::try_from(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_millis(),
            )
            .unwrap();
            assert!(
                (observed_at_ms + 60_000..=checked_at_ms + 60_000)
                    .contains(&first.retry_at_ms.unwrap())
            );
            assert_eq!(encoded["providerBackoff"]["scope"], MODEL);
            assert_eq!(encoded["providerBackoff"]["observedAtMs"], observed_at_ms);
            assert_eq!(
                encoded["providerBackoff"]["notBeforeMs"],
                provider.not_before_ms
            );
        } else {
            assert!(routing.provider_backoff.is_none());
            assert!(encoded.get("providerBackoff").is_none());
            assert!(first.retry_at_ms.is_some());
        }
        assert!(
            events[1]
                .routing
                .as_ref()
                .unwrap()
                .provider_backoff
                .is_none()
        );
    }
}
