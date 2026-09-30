use super::*;
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{HeaderMap, Response, StatusCode};
use axum::routing::post;
use axum::Router;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use zenith_relay_core::quota::QuotaWindowKind;

const ACCESS_TOKEN: &str = "access-private-secret";
const PROVIDER_ACCOUNT_ID: &str = "provider-private-account";

#[derive(Clone)]
struct TestState {
    response: Arc<Mutex<TestResponse>>,
    requests: Arc<Mutex<Vec<ObservedRequest>>>,
}

#[derive(Clone)]
struct TestResponse {
    status: StatusCode,
    body: Vec<u8>,
    content_length: Option<usize>,
}

#[derive(Clone, Debug)]
struct ObservedRequest {
    headers: HeaderMap,
    body: Value,
}

struct TestServer {
    endpoint: Url,
    task: JoinHandle<()>,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[tokio::test]
async fn sends_only_fixed_bounded_payload_and_private_headers() {
    let (server, state) = spawn_server(TestResponse {
        status: StatusCode::OK,
        body: json!({
            "output": [{"content": [{"text": "generated private text"}]}],
            "usage": {"input_tokens": 3, "output_tokens": 2, "total_tokens": 5}
        })
        .to_string()
        .into_bytes(),
        content_length: None,
    })
    .await;
    let client =
        CodexWakeClient::with_endpoint(server.endpoint.clone(), ACCESS_TOKEN, PROVIDER_ACCOUNT_ID)
            .unwrap();
    let result = client.execute(&request()).await.unwrap();
    assert_eq!(result.input_tokens, Some(3));
    assert_eq!(result.output_tokens, Some(2));
    assert_eq!(result.total_tokens, Some(5));

    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    let observed = &requests[0];
    assert_eq!(
        header(&observed.headers, AUTHORIZATION.as_str()).as_deref(),
        Some("Bearer access-private-secret")
    );
    assert_eq!(
        header(&observed.headers, ACCOUNT_ID_HEADER).as_deref(),
        Some(PROVIDER_ACCOUNT_ID)
    );
    assert_eq!(
        header(&observed.headers, ORIGINATOR_HEADER).as_deref(),
        Some(ORIGINATOR)
    );
    assert_eq!(observed.body["model"], "gpt-wake");
    assert_eq!(observed.body["input"], FIXED_WAKE_INPUT);
    assert_eq!(observed.body["stream"], false);
    assert_eq!(observed.body["store"], false);
    assert_eq!(observed.body["max_output_tokens"], 8);
    assert_eq!(observed.body.as_object().unwrap().len(), 5);
}

#[tokio::test]
async fn response_content_and_credentials_never_escape_metrics_or_debug() {
    let private_response = "generated-private-response";
    let (server, _) = spawn_server(TestResponse {
        status: StatusCode::OK,
        body: json!({
            "output_text": private_response,
            "provider_account_id": PROVIDER_ACCOUNT_ID,
            "token": ACCESS_TOKEN,
            "usage": {"input_tokens": 1}
        })
        .to_string()
        .into_bytes(),
        content_length: None,
    })
    .await;
    let client =
        CodexWakeClient::with_endpoint(server.endpoint.clone(), ACCESS_TOKEN, PROVIDER_ACCOUNT_ID)
            .unwrap();
    let metrics = client.execute(&request()).await.unwrap();
    let serialized = serde_json::to_string(&metrics).unwrap();
    let debug = format!("{client:?}");
    for secret in [
        private_response,
        PROVIDER_ACCOUNT_ID,
        ACCESS_TOKEN,
        FIXED_WAKE_INPUT,
    ] {
        assert!(!serialized.contains(secret));
        assert!(!debug.contains(secret));
    }
}

#[tokio::test]
async fn oversized_and_malformed_success_responses_fail_closed() {
    let (server, state) = spawn_server(TestResponse {
        status: StatusCode::OK,
        body: vec![b'x'; MAX_RESPONSE_BYTES + 1],
        content_length: None,
    })
    .await;
    let client =
        CodexWakeClient::with_endpoint(server.endpoint.clone(), ACCESS_TOKEN, PROVIDER_ACCOUNT_ID)
            .unwrap();
    let failure = client.execute(&request()).await.unwrap_err();
    assert_eq!(failure.code, WakeExecutionErrorCode::ResponseTooLarge);
    assert!(!failure.retryable);

    *state.response.lock().unwrap() = TestResponse {
        status: StatusCode::OK,
        body: b"not-json-private-body".to_vec(),
        content_length: None,
    };
    let failure = client.execute(&request()).await.unwrap_err();
    assert_eq!(failure.code, WakeExecutionErrorCode::InvalidResponse);
    assert!(!serde_json::to_string(&failure)
        .unwrap()
        .contains("not-json-private-body"));
}

#[tokio::test]
async fn http_failures_have_typed_retryability_without_body_leaks() {
    let (server, state) = spawn_server(TestResponse {
        status: StatusCode::UNAUTHORIZED,
        body: b"provider-secret-body".to_vec(),
        content_length: None,
    })
    .await;
    let client =
        CodexWakeClient::with_endpoint(server.endpoint.clone(), ACCESS_TOKEN, PROVIDER_ACCOUNT_ID)
            .unwrap();
    for (status, code, retryable) in [
        (
            StatusCode::UNAUTHORIZED,
            WakeExecutionErrorCode::Unauthorized,
            false,
        ),
        (
            StatusCode::FORBIDDEN,
            WakeExecutionErrorCode::Forbidden,
            false,
        ),
        (
            StatusCode::TOO_MANY_REQUESTS,
            WakeExecutionErrorCode::RateLimited,
            true,
        ),
        (
            StatusCode::SERVICE_UNAVAILABLE,
            WakeExecutionErrorCode::Upstream,
            true,
        ),
    ] {
        state.response.lock().unwrap().status = status;
        let failure = client.execute(&request()).await.unwrap_err();
        assert_eq!(failure.code, code);
        assert_eq!(failure.retryable, retryable);
        assert_eq!(failure.http_status, Some(status.as_u16()));
        assert!(!serde_json::to_string(&failure)
            .unwrap()
            .contains("provider-secret-body"));
    }
}

#[test]
fn completion_helper_uses_only_metrics_error_and_verification() {
    let success = Ok(WakeExecutionMetrics {
        http_status: 200,
        latency_ms: 10,
        input_tokens: Some(1),
        output_tokens: Some(1),
        total_tokens: Some(2),
    });
    let confirmed = completion_from_execution(
        &success,
        WakeVerificationOutcome::ConfirmedCountdownAdvanced,
        100,
    );
    assert_eq!(confirmed.outcome, WakeCompletionOutcome::Confirmed);
    assert_eq!(confirmed.input_tokens, Some(1));

    let unconfirmed =
        completion_from_execution(&success, WakeVerificationOutcome::Unconfirmed, 100);
    assert_eq!(unconfirmed.outcome, WakeCompletionOutcome::Unconfirmed);

    let failure = Err(WakeExecutionFailure::runtime(
        WakeExecutionErrorCode::RateLimited,
        true,
        Some(429),
        12,
    ));
    let failed = completion_from_execution(&failure, WakeVerificationOutcome::Unconfirmed, 100);
    assert_eq!(failed.outcome, WakeCompletionOutcome::Failed);
    assert_eq!(failed.error_code.as_deref(), Some("wake_rate_limited"));
    let serialized = format!("{failed:?}");
    assert!(!serialized.contains(ACCESS_TOKEN));
    assert!(!serialized.contains(PROVIDER_ACCOUNT_ID));
}

fn request() -> WakeExecutionRequest {
    WakeExecutionRequest {
        account_id: "relay-account".into(),
        model_id: "gpt-wake".into(),
        window_kind: QuotaWindowKind::Primary,
        output_token_cap: 8,
    }
}

async fn spawn_server(response: TestResponse) -> (TestServer, TestState) {
    let state = TestState {
        response: Arc::new(Mutex::new(response)),
        requests: Arc::new(Mutex::new(Vec::new())),
    };
    let app = Router::new()
        .route("/responses", post(handler))
        .with_state(state.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (
        TestServer {
            endpoint: Url::parse(&format!("http://{address}/responses")).unwrap(),
            task,
        },
        state,
    )
}

async fn handler(
    State(state): State<TestState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response<Body> {
    state.requests.lock().unwrap().push(ObservedRequest {
        headers,
        body: serde_json::from_slice(&body).unwrap_or(Value::Null),
    });
    let response = state.response.lock().unwrap().clone();
    let mut builder = Response::builder()
        .status(response.status)
        .header(CONTENT_TYPE, "application/json");
    if let Some(content_length) = response.content_length {
        builder = builder.header("content-length", content_length);
    }
    builder.body(Body::from(response.body)).unwrap()
}

fn header(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}
