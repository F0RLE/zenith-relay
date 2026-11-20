use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, Request, Response, StatusCode};
use axum::{Json, Router, routing::post};
use futures_util::{future::BoxFuture, stream};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering::SeqCst},
};
use std::time::Duration;
use tokio::sync::Semaphore;
use zenith_relay_core::gateway::hosted::*;
use zenith_relay_core::{ProviderSource, WireApi};

const SOURCE: &str = "fixture-source";
const MODEL: &str = "fixture-model";

fn body_for(request_id: &str) -> String {
    json!({"model":MODEL,"input":request_id,"max_output_tokens":16}).to_string()
}

fn admission(id: &str) -> HostedAttempt {
    let request_id = format!("request-{id}");
    HostedAttempt {
        workspace_id: format!("workspace-{id}"),
        key_id: format!("key-{id}"),
        body_binding: HostedBodyBinding::from_bytes(body_for(&request_id).as_bytes()),
        request_id,
        attempt_id: format!("attempt-{id}"),
        authority_revision: 1,
        tariff_revision: 4,
        bound_micro_usd: 100,
        max_input_bytes: 128,
        max_output_tokens: 32,
        source_id: SOURCE.into(),
        model_id: MODEL.into(),
    }
}

#[derive(Default)]
struct Records {
    approved: HashMap<String, HostedAttempt>,
    consumed: HashMap<String, HostedAttempt>,
    revoked: HashSet<String>,
}

struct Authority {
    records: Mutex<Records>,
    claims: AtomicUsize,
    entered: Semaphore,
    acknowledgements: Semaphore,
    lose_ack: bool,
}

impl HostedDispatchAuthority for Authority {
    fn claim<'a>(&'a self, attempt: &'a HostedAttempt) -> BoxFuture<'a, Result<(), ClaimError>> {
        Box::pin(async move {
            self.claims.fetch_add(1, SeqCst);
            {
                let mut records = self.records.lock().unwrap();
                if records.approved.get(&attempt.attempt_id) != Some(attempt)
                    || records.revoked.contains(&attempt.workspace_id)
                {
                    return Err(ClaimError::Denied);
                }
                if records.consumed.contains_key(&attempt.attempt_id) {
                    return Err(ClaimError::AlreadyClaimed);
                }
                records
                    .consumed
                    .insert(attempt.attempt_id.clone(), attempt.clone());
            }
            self.entered.add_permits(1);
            self.acknowledgements.acquire().await.unwrap().forget();
            if self.lose_ack {
                Err(ClaimError::Unconfirmed)
            } else {
                Ok(())
            }
        })
    }
}

#[derive(Clone, Copy)]
enum Reply {
    Ok,
    Rejected,
    Interrupted,
    WrongModel,
    NotTerminal,
}

struct Provider {
    requests: Mutex<Vec<(HeaderMap, Value)>>,
    active: AtomicUsize,
    peak: AtomicUsize,
    received: Semaphore,
    release: Semaphore,
    reply: Reply,
}

async fn upstream(
    State(s): State<Arc<Provider>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response<Body> {
    s.requests.lock().unwrap().push((headers, body.clone()));
    let n = s.active.fetch_add(1, SeqCst) + 1;
    s.peak.fetch_max(n, SeqCst);
    s.received.add_permits(1);
    let (status, reply) = match s.reply {
        Reply::Rejected => (
            StatusCode::TOO_MANY_REQUESTS,
            json!({"error":{"code":"insufficient_quota"}}),
        ),
        _ => (
            StatusCode::OK,
            json!({
                "id":"resp_fixture","object":"response",
                "model":if matches!(s.reply, Reply::WrongModel) { "unoffered" } else { MODEL },
                "status":if matches!(s.reply, Reply::NotTerminal) { "in_progress" } else { "completed" },
                "output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":body["input"]}]}]
            }),
        ),
    };
    // Headers may arrive immediately; keep the physical request active until the body.
    let data = stream::once(async move {
        s.release.acquire().await.unwrap().forget();
        s.active.fetch_sub(1, SeqCst);
        if matches!(s.reply, Reply::Interrupted) {
            Err(std::io::Error::other("fixture disconnect"))
        } else {
            Ok(axum::body::Bytes::from(reply.to_string()))
        }
    });
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(Body::from_stream(data))
        .unwrap()
}

struct Fixture {
    runtime: HostedTextRuntime,
    host: Arc<Authority>,
    provider: Arc<Provider>,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl Fixture {
    async fn new(reply: Reply, lose_ack: bool) -> Self {
        Self::with_proxy(reply, lose_ack, None).await
    }

    async fn with_proxy(reply: Reply, lose_ack: bool, proxy_url: Option<String>) -> Self {
        let provider = Arc::new(Provider {
            requests: Mutex::default(),
            active: 0.into(),
            peak: 0.into(),
            received: Semaphore::new(0),
            release: Semaphore::new(0),
            reply,
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let router = Router::new()
            .route("/v1/responses", post(upstream))
            .with_state(provider.clone());
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let host = Arc::new(Authority {
            records: Mutex::default(),
            claims: 0.into(),
            entered: Semaphore::new(0),
            acknowledgements: Semaphore::new(0),
            lose_ack,
        });
        for id in ["a", "b"] {
            let a = admission(id);
            host.records
                .lock()
                .unwrap()
                .approved
                .insert(a.attempt_id.clone(), a);
        }
        let client = reqwest::Client::builder()
            .no_proxy()
            .http1_only()
            .retry(reqwest::retry::never())
            .redirect(reqwest::redirect::Policy::none());
        let client = match proxy_url.as_deref() {
            Some(proxy) => zenith_relay_core::ProxyConfig::parse(proxy)
                .unwrap()
                .apply(client),
            None => client,
        };
        let runtime = HostedTextRuntime::new(
            ProviderSource {
                id: SOURCE.into(),
                name: "Fixture".into(),
                base_url: url,
                api_key: "fixture-upstream".into(),
                wire_api: WireApi::Responses,
                models: vec![MODEL.into()],
                proxy_url,
            },
            client.build().unwrap(),
        )
        .unwrap();
        Self {
            runtime,
            provider,
            host,
            server,
        }
    }

    async fn run(&self, a: &HostedAttempt) -> Result<Value, HostedExecutionError> {
        let body = body_for(&a.request_id);
        let request = Request::post("/v1/responses")
            .header("x-workspace-id", "workspace-b")
            .header("authorization", "Bearer not-an-upstream-key")
            .body(body.as_bytes())
            .unwrap();
        self.runtime.execute(a, request, self.host.as_ref()).await
    }

    fn open(&self) {
        self.host.acknowledgements.add_permits(8);
        self.provider.release.add_permits(8);
    }

    fn count(&self) -> usize {
        self.provider.requests.lock().unwrap().len()
    }
}

async fn reached(gate: &Semaphore) {
    tokio::time::timeout(Duration::from_secs(5), gate.acquire())
        .await
        .expect("barrier")
        .unwrap()
        .forget()
}

#[tokio::test]
async fn one_ack_one_send_bound_to_principal_route_and_price() {
    let f = Fixture::new(Reply::Ok, false).await;
    let a = admission("a");
    let send = f.run(&a);
    tokio::pin!(send);
    tokio::select! { biased; _ = &mut send => panic!("sent without ack"), () = reached(&f.host.entered) => {} }
    assert_eq!(f.count(), 0);
    f.open();
    assert_eq!(
        send.await.unwrap()["output"][0]["content"][0]["text"],
        a.request_id
    );
    assert_eq!(
        f.run(&a).await,
        Err(HostedExecutionError::Claim(ClaimError::AlreadyClaimed))
    );
    let mut b = admission("b");
    b.request_id.clone_from(&a.request_id);
    b.body_binding = a.body_binding;
    b.attempt_id.clone_from(&a.attempt_id);
    assert_eq!(
        f.run(&b).await,
        Err(HostedExecutionError::Claim(ClaimError::Denied))
    );
    let mut altered = admission("b");
    altered.bound_micro_usd += 1;
    assert_eq!(
        f.run(&altered).await,
        Err(HostedExecutionError::Claim(ClaimError::Denied))
    );
    let posts = f.provider.requests.lock().unwrap();
    assert_eq!(posts.len(), 1);
    assert!(!posts[0].0.contains_key("x-workspace-id"));
    assert_eq!(posts[0].0["authorization"], "Bearer fixture-upstream");
    assert_eq!(posts[0].1["store"], false);
}

#[tokio::test]
async fn refusal_and_unoffered_operations_never_send() {
    let f = Fixture::new(Reply::Ok, false).await;
    f.open();
    let a = admission("a");
    f.host
        .records
        .lock()
        .unwrap()
        .revoked
        .insert(a.workspace_id.clone());
    assert_eq!(
        f.run(&a).await,
        Err(HostedExecutionError::Claim(ClaimError::Denied))
    );
    let substituted = body_for("different input");
    let request = Request::post("/v1/responses")
        .body(substituted.as_bytes())
        .unwrap();
    assert_eq!(
        f.runtime.execute(&a, request, f.host.as_ref()).await,
        Err(HostedExecutionError::InvalidRequest)
    );
    for (path, extra) in [
        ("/v1/images/generations", json!({})),
        ("/v1/responses", json!({"stream":true})),
        ("/v1/responses", json!({"tools":[]})),
        ("/v1/responses", json!({"max_output_tokens":33})),
        (
            "/v1/responses",
            json!({"input":"x".repeat(a.max_input_bytes + 1)}),
        ),
        ("/v1/responses", json!({"workspace_id":"workspace-a"})),
    ] {
        let mut body = json!({"model":MODEL,"max_output_tokens":16,"input":"fixture"});
        body.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let bytes = serde_json::to_vec(&body).unwrap();
        let req = Request::post(path).body(bytes.as_slice()).unwrap();
        let mut attempt = a.clone();
        attempt.body_binding = HostedBodyBinding::from_bytes(&bytes);
        assert_eq!(
            f.runtime.execute(&attempt, req, f.host.as_ref()).await,
            Err(HostedExecutionError::InvalidRequest)
        );
    }
    assert_eq!(f.host.claims.load(SeqCst), 1);
    assert_eq!(f.count(), 0);
    assert!(f.run(&admission("b")).await.is_ok());
}

#[tokio::test]
async fn two_principals_compete_for_one_physical_slot_and_recheck_after_waiting() {
    for revoke in [false, true] {
        let f = Fixture::new(Reply::Ok, false).await;
        let (a, b) = (admission("a"), admission("b"));
        f.host.acknowledgements.add_permits(2);
        let first = f.run(&a);
        tokio::pin!(first);
        tokio::select! { biased; _ = &mut first => panic!("body not released"), () = reached(&f.provider.received) => {} }
        let second = f.run(&b);
        tokio::pin!(second);
        // Poll B into the actual scheduler while A's upstream body is held.
        assert!(futures_util::poll!(&mut second).is_pending());
        assert_eq!(f.host.claims.load(SeqCst), 1);
        assert_eq!(f.count(), 1);
        if revoke {
            f.host
                .records
                .lock()
                .unwrap()
                .revoked
                .insert(b.workspace_id.clone());
        }
        f.provider.release.add_permits(2);
        assert!(first.await.is_ok());
        let result = tokio::time::timeout(Duration::from_secs(5), second)
            .await
            .unwrap();
        if revoke {
            assert_eq!(result, Err(HostedExecutionError::Claim(ClaimError::Denied)));
        } else {
            assert_eq!(
                result.unwrap()["output"][0]["content"][0]["text"],
                b.request_id
            );
        }
        assert_eq!(f.count(), if revoke { 1 } else { 2 });
        assert_eq!(f.provider.peak.load(SeqCst), 1);
    }
}

#[tokio::test]
async fn lost_ack_and_lost_outcome_never_replay() {
    for (lose_ack, count) in [(true, 0), (false, 1)] {
        let f = Fixture::new(Reply::Interrupted, lose_ack).await;
        f.open();
        let a = admission("a");
        let result = f.run(&a).await;
        if lose_ack {
            assert_eq!(
                result,
                Err(HostedExecutionError::Claim(ClaimError::Unconfirmed))
            );
        } else {
            // The connection may close before the client receives the headers.
            assert!(
                matches!(
                    result,
                    Err(HostedExecutionError::OutcomeUnknown(
                        HostedUnknown::SendUnconfirmed | HostedUnknown::ReadInterrupted
                    ))
                ),
                "{result:?}"
            );
        }
        assert_eq!(
            f.run(&a).await,
            Err(HostedExecutionError::Claim(ClaimError::AlreadyClaimed))
        );
        assert_eq!(f.count(), count);
        assert_eq!(
            f.host.records.lock().unwrap().consumed.get(&a.attempt_id),
            Some(&a)
        );
    }
}

#[tokio::test]
async fn pre_execution_refusal_does_not_authorize_another_attempt() {
    let f = Fixture::new(Reply::Rejected, false).await;
    f.open();
    let a = admission("a");
    assert_eq!(
        f.run(&a).await,
        Err(HostedExecutionError::UpstreamStatus(429))
    );
    let mut next = a.clone();
    next.attempt_id.push_str("-2");
    assert_eq!(
        f.run(&next).await,
        Err(HostedExecutionError::Claim(ClaimError::Denied))
    );
    assert_eq!(
        f.run(&a).await,
        Err(HostedExecutionError::Claim(ClaimError::AlreadyClaimed))
    );
    assert_eq!(f.count(), 1);
}

#[tokio::test]
async fn cancelling_a_queue_entry_or_unacknowledged_claim_does_not_send() {
    let f = Fixture::new(Reply::Ok, false).await;
    let (a, b) = (admission("a"), admission("b"));
    {
        let first = f.run(&a);
        tokio::pin!(first);
        tokio::select! { biased; _ = &mut first => panic!("unacknowledged"), () = reached(&f.host.entered) => {} }
        {
            let mut queued = Box::pin(f.run(&b));
            assert!(futures_util::poll!(&mut queued).is_pending());
        }
        assert_eq!(f.host.claims.load(SeqCst), 1);
    }
    f.open();
    assert_eq!(
        f.run(&a).await,
        Err(HostedExecutionError::Claim(ClaimError::AlreadyClaimed))
    );
    assert_eq!(f.count(), 0);
    assert!(
        tokio::time::timeout(Duration::from_secs(5), f.run(&b))
            .await
            .unwrap()
            .is_ok()
    );
    assert_eq!(f.count(), 1);
}

#[tokio::test]
async fn an_unavailable_assigned_proxy_never_falls_back_to_the_working_direct_route() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    let proxy = Router::new().fallback(move || {
        counted.fetch_add(1, SeqCst);
        async { StatusCode::BAD_GATEWAY }
    });
    let task = tokio::spawn(async move { axum::serve(listener, proxy).await.unwrap() });
    let f = Fixture::with_proxy(Reply::Ok, false, Some(url)).await;
    f.open();
    let a = admission("a");
    assert_eq!(
        f.run(&a).await,
        Err(HostedExecutionError::UpstreamStatus(502))
    );
    assert_eq!(
        f.run(&a).await,
        Err(HostedExecutionError::Claim(ClaimError::AlreadyClaimed))
    );
    assert_eq!((calls.load(SeqCst), f.count()), (1, 0));
    task.abort();
}

#[tokio::test]
async fn wrong_model_and_nonterminal_success_are_uncertain_not_retryable() {
    for reply in [Reply::WrongModel, Reply::NotTerminal] {
        let f = Fixture::new(reply, false).await;
        f.open();
        let a = admission("a");
        assert_eq!(
            f.run(&a).await,
            Err(HostedExecutionError::OutcomeUnknown(
                HostedUnknown::InvalidResponse
            ))
        );
        assert_eq!(
            f.run(&a).await,
            Err(HostedExecutionError::Claim(ClaimError::AlreadyClaimed))
        );
        assert_eq!(f.count(), 1);
    }
}
