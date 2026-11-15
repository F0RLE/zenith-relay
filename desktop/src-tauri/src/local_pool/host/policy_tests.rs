use super::GatewayManager;
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{HeaderMap, Response, header::AUTHORIZATION};
use axum::routing::post;
use axum::{Json, Router};
use futures_util::{StreamExt, stream};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::convert::Infallible;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;
use tokio::sync::Notify;
use tokio::time::{Duration, timeout};
use zenith_relay_core::{
    CandidateScope, GatewayRuntime, GatewayRuntimeOptions, LocalGatewayKey, ProviderSource,
    RuntimeCandidatePolicy, RuntimeLocalKey, RuntimeSource, UsageEvent, WireApi,
};

const MODEL: &str = "synthetic-policy-model";
const KEY: &str = "synthetic-local-key";

#[derive(Clone, Default)]
struct UpstreamState {
    requests: Arc<Mutex<Vec<(String, Value)>>>,
    release: Arc<Notify>,
}

struct UpstreamTask(tokio::task::JoinHandle<()>);

impl Drop for UpstreamTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

fn completion(index: usize) -> String {
    format!(
        "data: {}\n\n",
        json!({
            "type":"response.completed", "response":{
                "id":format!("synthetic-{index}"),"status":"completed",
                "output":[{"type":"message","id":format!("synthetic-message-{index}"),
                    "role":"assistant","status":"completed","content":[{
                        "type":"output_text","text":"synthetic completed answer","annotations":[]
                    }]}],
                "usage":{"input_tokens":index+1,"output_tokens":1,"total_tokens":index+2}
            }
        })
    )
}

async fn upstream_response(
    State(state): State<UpstreamState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response<Body> {
    let index = {
        let mut requests = state.requests.lock().unwrap();
        let index = requests.len();
        requests.push((headers[AUTHORIZATION].to_str().unwrap().to_string(), body));
        index
    };
    let body = if index == 1 {
        let prefix = stream::once(async {
            Ok::<_, Infallible>(Bytes::from_static(
                b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"synthetic active output\"}\n\n",
            ))
        });
        let terminal = stream::once(async move {
            state.release.notified().await;
            Ok::<_, Infallible>(Bytes::from(completion(index)))
        });
        Body::from_stream(prefix.chain(terminal))
    } else {
        Body::from(completion(index))
    };
    Response::builder()
        .header("content-type", "text/event-stream")
        .body(body)
        .unwrap()
}

#[derive(Clone, Copy, Debug)]
enum Restriction {
    Source,
    Visibility,
    KeyScope,
}

impl Restriction {
    fn apply(self, runtime: &GatewayRuntime, denied: bool) {
        match self {
            Self::Source => assert!(runtime.update_source_policy(
                "synthetic-source",
                RuntimeCandidatePolicy {
                    enabled: !denied,
                    draining: false,
                    priority: 3,
                    weight: 2,
                    allowed_models: vec![],
                    excluded_models: vec![],
                },
                0
            )),
            Self::Visibility => {
                runtime.set_hidden_models(if denied { vec![MODEL.into()] } else { vec![] })
            }
            Self::KeyScope => assert!(runtime.update_key_scope(
                "synthetic-local",
                CandidateScope {
                    source_ids: denied.then(BTreeSet::new),
                    account_ids: denied.then(BTreeSet::new),
                    model_rules: Default::default(),
                }
            )),
        }
    }
}

async fn model_visible(client: &reqwest::Client, base: &str) -> bool {
    let models: Value = client
        .get(format!("{base}/v1/models"))
        .bearer_auth(KEY)
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
async fn hot_policy_edits_keep_listener_active_stream_and_saved_continuation() {
    let upstream = UpstreamState::default();
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let upstream_url = format!("http://{}/v1", listener.local_addr().unwrap());
    let router = Router::new()
        .route("/v1/responses", post(upstream_response))
        .with_state(upstream.clone());
    let _upstream_task = UpstreamTask(tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    }));
    let events = Arc::new(Mutex::new(Vec::<UsageEvent>::new()));
    let usage = events.clone();
    let runtime = Arc::new(
        GatewayRuntime::from_pool(
            vec![RuntimeSource::unrestricted(ProviderSource {
                id: "synthetic-source".into(),
                name: "Synthetic".into(),
                base_url: upstream_url,
                api_key: "synthetic-upstream-key".into(),
                proxy_url: None,
                wire_api: WireApi::Responses,
                models: vec![MODEL.into()],
            })],
            vec![RuntimeLocalKey::unrestricted(LocalGatewayKey {
                id: "synthetic-local".into(),
                secret: KEY.into(),
            })],
            GatewayRuntimeOptions::default(),
            Arc::new(move |event| usage.lock().unwrap().push(event)),
        )
        .unwrap(),
    );
    let manager = GatewayManager::default();
    let address = manager.start(runtime.clone(), 0).await.unwrap();
    let base = format!("http://{address}");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    let request = |previous: Option<&str>| {
        let mut body = json!({"model":MODEL,"stream":true,"input":"synthetic request"});
        if let Some(previous) = previous {
            body["previous_response_id"] = json!(previous);
        }
        client
            .post(format!("{base}/v1/responses"))
            .bearer_auth(KEY)
            .json(&body)
    };

    let first = request(None)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(first.contains("synthetic-0"));
    let response = request(Some("synthetic-0"))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let mut active = response.bytes_stream();
    let prefix = timeout(Duration::from_secs(3), active.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(String::from_utf8_lossy(&prefix).contains("synthetic active output"));
    assert_eq!(runtime.candidate_runtime_order()[0].in_flight, 1);

    for restriction in [
        Restriction::Source,
        Restriction::Visibility,
        Restriction::KeyScope,
    ] {
        restriction.apply(&runtime, true);
        assert_eq!(manager.address().await, Some(address));
        assert!(Arc::ptr_eq(&runtime, &manager.runtime().await.unwrap()));
        assert!(!model_visible(&client, &base).await, "{restriction:?}");
        let rejected = request(None).send().await.unwrap();
        assert!(!rejected.status().is_success(), "{restriction:?}");
        let _ = rejected.bytes().await.unwrap();
        assert_eq!(upstream.requests.lock().unwrap().len(), 2);
        assert_eq!(runtime.candidate_runtime_order()[0].in_flight, 1);
        restriction.apply(&runtime, false);
        assert!(model_visible(&client, &base).await, "{restriction:?}");
    }

    upstream.release.notify_one();
    let remaining = timeout(Duration::from_secs(3), async {
        let mut body = Vec::new();
        while let Some(chunk) = active.next().await {
            body.extend_from_slice(&chunk.unwrap());
        }
        body
    })
    .await
    .unwrap();
    assert!(String::from_utf8_lossy(&remaining).contains("synthetic-1"));
    let branch = request(Some("synthetic-0"))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(branch.contains("synthetic-2"));
    assert_eq!(manager.address().await, Some(address));
    assert!(Arc::ptr_eq(&runtime, &manager.runtime().await.unwrap()));
    assert_eq!(runtime.candidate_runtime_order()[0].in_flight, 0);
    {
        let requests = upstream.requests.lock().unwrap();
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[2].1["previous_response_id"], "synthetic-0");
        assert!(
            requests
                .iter()
                .all(|(authorization, _)| authorization == "Bearer synthetic-upstream-key")
        );
    }
    {
        let events = events.lock().unwrap();
        let dispatched = events
            .iter()
            .filter(|event| event.candidate_id.is_some())
            .collect::<Vec<_>>();
        assert_eq!(dispatched.len(), 3);
        assert!(
            dispatched
                .iter()
                .all(|event| event.success && event.attempt == 1)
        );
        assert_eq!(
            dispatched
                .iter()
                .map(|event| event.input_tokens.unwrap())
                .sum::<u64>(),
            6
        );
    }
    manager.stop().await;
}
