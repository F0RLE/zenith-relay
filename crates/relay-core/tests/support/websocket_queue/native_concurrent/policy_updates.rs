use super::*;
use zenith_relay_core::{CandidateScope, RuntimeCandidatePolicy};

#[derive(Clone, Copy, Debug)]
enum Restriction {
    Visibility,
    KeyScope,
    SourceModel,
}

impl Restriction {
    fn apply(self, runtime: &GatewayRuntime, denied: bool) {
        match self {
            Self::Visibility => {
                runtime.set_hidden_models(if denied { vec![MODEL.into()] } else { vec![] })
            }
            Self::KeyScope => assert!(runtime.update_key_scope(
                "local-key",
                CandidateScope {
                    source_ids: denied.then(BTreeSet::new),
                    account_ids: denied.then(BTreeSet::new),
                    model_rules: Default::default(),
                }
            )),
            Self::SourceModel => assert!(runtime.update_source_policy(
                "native-concurrent",
                RuntimeCandidatePolicy {
                    enabled: true,
                    draining: false,
                    priority: 0,
                    weight: 1,
                    allowed_models: vec![],
                    excluded_models: if denied { vec![MODEL.into()] } else { vec![] },
                },
                0
            )),
        }
    }
}

async fn model_visible(gateway: &TestServer) -> bool {
    let models: Value = reqwest::Client::new()
        .get(format!("{}/v1/models", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    models["data"]
        .as_array()
        .unwrap()
        .iter()
        .any(|model| model["id"] == MODEL)
}

#[tokio::test]
async fn websocket_native_policy_edits_preserve_active_owners_and_revalidate_queued_work() {
    for restriction in [
        Restriction::Visibility,
        Restriction::KeyScope,
        Restriction::SourceModel,
    ] {
        let (upstream, probe) = fixture().await;
        let (gateway, events) = gateway_for(&upstream).await;
        let runtime = gateway.runtime.as_ref().unwrap();
        let mut socket = connect(&gateway).await;
        socket.send(create(0, Some("main"))).await.unwrap();
        created(&mut socket, Some("main"), 0).await;
        socket.send(create(1, Some("other"))).await.unwrap();
        created(&mut socket, Some("other"), 1).await;
        assert!(model_visible(&gateway).await);
        continuation(&mut socket, "main", "synthetic queued before policy edit").await;
        reject_marker(&mut socket).await;
        restriction.apply(runtime, true);
        assert!(!model_visible(&gateway).await, "{restriction:?}");
        assert_eq!(runtime.candidate_runtime_order()[0].in_flight, 2);
        probe.complete(Some("main"), "native-0", 2);
        assert_eq!(
            receive_websocket_json(&mut socket).await["type"],
            "response.completed"
        );
        let denied = receive_websocket_json(&mut socket).await;
        assert_eq!(denied["type"], "error", "{restriction:?}: {denied}");
        assert_eq!(denied["stream_id"], "main");
        assert_eq!(probe.requests.lock().unwrap().len(), 2);
        assert_eq!(runtime.candidate_runtime_order()[0].in_flight, 1);
        probe.emit(json!({"type":"response.output_text.delta", "stream_id":"other", "response_id":"native-1", "delta":"synthetic output survives policy edit"}));
        assert_eq!(
            receive_websocket_json(&mut socket).await["delta"],
            "synthetic output survives policy edit"
        );
        probe.complete(Some("other"), "native-1", 3);
        assert_eq!(
            receive_websocket_json(&mut socket).await["type"],
            "response.completed"
        );
        probe.wait_settled(&gateway).await;
        restriction.apply(runtime, false);
        assert!(model_visible(&gateway).await, "{restriction:?}");
        continuation(
            &mut socket,
            "main",
            "synthetic fresh request after policy restore",
        )
        .await;
        created(&mut socket, Some("main"), 2).await;
        assert_eq!(
            probe.requests.lock().unwrap()[2]["previous_response_id"],
            "native-0"
        );
        probe.complete(Some("main"), "native-2", 5);
        assert_eq!(
            receive_websocket_json(&mut socket).await["type"],
            "response.completed"
        );
        probe.wait_settled(&gateway).await;
        assert_eq!(probe.sockets.lock().unwrap().len(), 1, "{restriction:?}");
        assert_eq!(probe.requests.lock().unwrap().len(), 3);
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 3, "{restriction:?}");
        assert!(
            events
                .iter()
                .all(|event| event.success && event.attempt == 1)
        );
        assert_eq!(
            events
                .iter()
                .map(|event| event.input_tokens.unwrap())
                .sum::<u64>(),
            10
        );
    }
}
