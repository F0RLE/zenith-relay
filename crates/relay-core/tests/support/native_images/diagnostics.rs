use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};

#[tokio::test]
async fn native_image_diagnostics_observe_final_send_without_changing_response_or_retrying() {
    let mut header = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
    header.extend_from_slice(&1672_u32.to_be_bytes());
    header.extend_from_slice(&941_u32.to_be_bytes());
    for debug in [false, true] {
        for status in [StatusCode::OK, StatusCode::BAD_REQUEST] {
            let payload = json!({
                "model":"gpt-image-2-codex", "quality":{"ignored":"synthetic-private"},
                "size":"auto", "output_format":"png",
                "data":[{"b64_json":STANDARD.encode(&header), "quality":"high"}],
                "usage":{"input_tokens":318,"output_tokens":1158,"total_tokens":1476}
            });
            let (upstream, state) =
                spawn_upstream(vec![Reply::Json(status, payload.clone())]).await;
            let authority = ready_authority("image-account", "synthetic-access").await;
            let (gateway, events, _, _) = spawn_mixed_gateway_with_options(
                Vec::new(),
                vec![account(
                    "image-account",
                    "synthetic-provider",
                    &upstream,
                    10,
                )],
                vec![mixed_key(None, None)],
                authority,
                refresh_adapter(),
                Arc::new(PersistenceAdapter::default()),
                GatewayRuntimeOptions {
                    native_codex_images_only: true,
                    diagnostics_enabled: if debug { || true } else { || false },
                    ..Default::default()
                },
            )
            .await;
            let response = reqwest::Client::new().post(format!("{}/v1/images/generations", gateway.base_url))
                .bearer_auth(LOCAL_KEY)
                .json(&json!({"model":"gpt-image-2.6-sunburst", "prompt":"synthetic-private-prompt", "size":"3840x2160", "quality":"max", "complexity":"high", "output_format":"png", "n":1}))
                .send().await.unwrap();
            assert_eq!(response.status(), status);
            let bytes = response.bytes().await.unwrap();
            if status.is_success() {
                assert_eq!(bytes.as_ref(), serde_json::to_vec(&payload).unwrap());
            }
            let requests = state.requests.lock().unwrap();
            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0].body["size"], "3840x2160");
            assert_eq!(requests[0].body["quality"], "max");
            drop(requests);
            let events = events.lock().unwrap();
            assert_eq!(events.len(), 1);
            assert_eq!(events[0].success, status.is_success());
            assert_eq!(events[0].total_tokens, Some(1476));
            let diagnostics = events[0].routing.as_ref().unwrap().images.as_ref();
            if !debug {
                assert!(diagnostics.is_none());
                continue;
            }
            let diagnostics = diagnostics.unwrap();
            assert_eq!(diagnostics.requested.size.as_deref(), Some("3840x2160"));
            assert_eq!(
                diagnostics.sent.as_ref().unwrap().quality.as_deref(),
                Some("max")
            );
            assert_eq!(
                diagnostics.sent.as_ref().unwrap().model.as_deref(),
                Some("gpt-image-2.6-sunburst")
            );
            assert_eq!(diagnostics.reported[0].size.as_deref(), Some("auto"));
            assert_eq!(diagnostics.reported[0].quality, None);
            assert_eq!(diagnostics.outputs[0].dimensions.unwrap().width, 1672);
            assert_eq!(diagnostics.outputs[0].dimensions.unwrap().height, 941);
            assert_eq!(diagnostics.outputs[0].sent_size_matches, Some(false));
            assert!(
                !serde_json::to_string(diagnostics)
                    .unwrap()
                    .contains("synthetic-private")
            );
        }
    }
}
