//! Explicit, loopback-only qualification of the compiled transport and dispatch budget.
use super::{GatewayRuntime, GatewayRuntimeOptions, RuntimeLocalKey, RuntimeSource};
use crate::{LocalGatewayKey, ProviderSource, UsageEvent, WireApi};
use serde::Serialize;
use std::collections::BTreeSet;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;
use tokio::task::JoinSet;

mod fixture;
use fixture::{Fixture, Reply};

const MODEL: &str = "synthetic-transport-model";
const LOCAL_KEY: &str = "synthetic-diagnostic-key";

struct DiagnosticCase {
    name: &'static str,
    reply: Reply,
    candidate_limit: usize,
    expected_requests: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Http2RetryDiagnostic {
    pub passed: bool,
    pub default_client_requests: usize,
    pub cases: Vec<Http2DispatchObservation>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Http2DispatchObservation {
    pub case: &'static str,
    pub candidate_limit: usize,
    pub physical_requests: usize,
    pub recorded_attempts: usize,
    pub http_status: u16,
    pub one_logical_request: bool,
    pub passed: bool,
}

/// Uses synthetic sources and the real gateway. It never reads Desktop storage,
/// resolves a host name, uses a proxy, or contacts a provider.
pub async fn diagnose_http2_retries() -> Result<Http2RetryDiagnostic, &'static str> {
    tokio::time::timeout(Duration::from_secs(20), run())
        .await
        .map_err(|_| "http2_diagnostic_timeout")?
}

async fn run() -> Result<Http2RetryDiagnostic, &'static str> {
    let mut tasks = JoinSet::new();
    let control = Fixture::start(&mut tasks, Reply::Refused).await?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .http2_prior_knowledge()
        .build()
        .map_err(|_| "http2_control_client_unavailable")?;
    let result = tokio::time::timeout(
        Duration::from_secs(4),
        client.post(&control.url).body("synthetic request").send(),
    )
    .await
    .map_err(|_| "http2_control_timeout")?;
    if result.is_ok() {
        return Err("http2_control_did_not_reject");
    }
    let default_client_requests = control.requests.load(Ordering::SeqCst);
    drop(client);
    let mut cases = Vec::with_capacity(4);
    for (name, reply, limit, expected) in [
        ("refused_stream", Reply::Refused, 1, 1),
        ("uncertain_stream_reset", Reply::Cancelled, 2, 1),
        (
            "rate_limit_budget_exhausted",
            Reply::RateLimitThenSuccess,
            1,
            1,
        ),
        ("rate_limit_failover", Reply::RateLimitThenSuccess, 2, 2),
    ] {
        let case = DiagnosticCase {
            name,
            reply,
            candidate_limit: limit,
            expected_requests: expected,
        };
        cases.push(observe_dispatch(&mut tasks, &case).await?);
    }
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    Ok(Http2RetryDiagnostic {
        passed: default_client_requests > 1 && cases.iter().all(|case| case.passed),
        default_client_requests,
        cases,
    })
}

async fn observe_dispatch(
    tasks: &mut JoinSet<()>,
    case: &DiagnosticCase,
) -> Result<Http2DispatchObservation, &'static str> {
    let upstream = Fixture::start(tasks, case.reply).await?;
    let events = Arc::new(Mutex::new(Vec::<UsageEvent>::new()));
    let overflowed = Arc::new(AtomicBool::new(false));
    let runtime = diagnostic_runtime(case, &upstream.url, &events, &overflowed)?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|_| "http2_gateway_bind_failed")?;
    let address = listener
        .local_addr()
        .map_err(|_| "http2_gateway_address_failed")?;
    tasks.spawn(async move {
        let _ = axum::serve(listener, crate::gateway::router(Arc::new(runtime))).await;
    });
    let http_status = gateway_response(address).await?;
    let physical_requests = upstream.requests.load(Ordering::SeqCst);
    Ok(observe_events(
        case,
        &crate::poison::mutex(&events),
        physical_requests,
        http_status,
        overflowed.load(Ordering::SeqCst),
    ))
}

fn diagnostic_runtime(
    case: &DiagnosticCase,
    upstream: &str,
    events: &Arc<Mutex<Vec<UsageEvent>>>,
    overflowed: &Arc<AtomicBool>,
) -> Result<GatewayRuntime, &'static str> {
    let overflow = overflowed.clone();
    let captured = events.clone();
    let mut runtime = GatewayRuntime::from_pool(
        ["diagnostic-a", "diagnostic-b"]
            .into_iter()
            .map(|id| {
                RuntimeSource::unrestricted(ProviderSource {
                    id: id.into(),
                    name: "Synthetic transport source".into(),
                    base_url: upstream.into(),
                    api_key: "synthetic-upstream-key".into(),
                    proxy_url: None,
                    wire_api: WireApi::Responses,
                    models: vec![MODEL.into()],
                })
            })
            .collect(),
        vec![RuntimeLocalKey::unrestricted(LocalGatewayKey {
            id: "diagnostic-key".into(),
            secret: LOCAL_KEY.into(),
        })],
        GatewayRuntimeOptions {
            max_retry_candidates: case.candidate_limit,
            diagnostics_enabled: || true,
            ..Default::default()
        },
        Arc::new(move |event| {
            let mut events = crate::poison::mutex(&captured);
            if events.len() < 16 {
                events.push(event);
            } else {
                overflow.store(true, Ordering::SeqCst);
            }
        }),
    )
    .map_err(|_| "http2_diagnostic_runtime_invalid")?;
    // Prior knowledge is only the loopback fixture's negotiation mechanism.
    // Preserve the production builder's retry policy rather than replacing it here.
    runtime.clients.http = super::support::runtime_client_builder(None)
        .no_proxy()
        .http2_prior_knowledge()
        .build()
        .map_err(|_| "http2_runtime_client_unavailable")?;
    Ok(runtime)
}

async fn gateway_response(address: std::net::SocketAddr) -> Result<u16, &'static str> {
    let client = reqwest::Client::builder()
        .no_proxy()
        .http1_only()
        .retry(reqwest::retry::never())
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "http2_gateway_client_unavailable")?;
    let mut response = tokio::time::timeout(
        Duration::from_secs(4),
        client
            .post(format!("http://{address}/v1/responses"))
            .bearer_auth(LOCAL_KEY)
            .json(&serde_json::json!({"model":MODEL,"input":"synthetic request","stream":false}))
            .send(),
    )
    .await
    .map_err(|_| "http2_dispatch_timeout")?
    .map_err(|_| "http2_gateway_unavailable")?;
    let http_status = response.status().as_u16();
    let mut bytes = 0;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "http2_gateway_body_failed")?
    {
        bytes += chunk.len();
        if bytes > 64 * 1024 {
            return Err("http2_gateway_body_too_large");
        }
    }
    Ok(http_status)
}

fn observe_events(
    case: &DiagnosticCase,
    events: &[UsageEvent],
    physical_requests: usize,
    http_status: u16,
    overflowed: bool,
) -> Http2DispatchObservation {
    let sends: Vec<_> = events
        .iter()
        .filter(|event| {
            !event
                .routing
                .as_ref()
                .and_then(|routing| routing.request_timing.as_ref())
                .is_some_and(|timing| timing.projection_only)
        })
        .collect();
    let request_ids: BTreeSet<_> = events
        .iter()
        .map(|event| event.request_id.as_str())
        .collect();
    let attempts: BTreeSet<_> = sends.iter().map(|event| event.attempt).collect();
    let sources: BTreeSet<_> = sends.iter().map(|event| event.source_id.as_str()).collect();
    let recorded_attempts = sends.len();
    let one_logical_request = request_ids.len() == 1;
    let passed = physical_requests == case.expected_requests
        && !overflowed
        && recorded_attempts == physical_requests
        && attempts.len() == physical_requests
        && sources.len() == physical_requests
        && attempts
            .iter()
            .copied()
            .map(usize::from)
            .eq(1..=physical_requests)
        && one_logical_request
        && ((http_status == 200) == (case.expected_requests == 2))
        && sends.first().is_some_and(|event| !event.success)
        && (!matches!(case.reply, Reply::RateLimitThenSuccess)
            || sends.first().is_some_and(|event| event.http_status == 429))
        && sends
            .last()
            .is_some_and(|event| event.success == (http_status == 200));
    Http2DispatchObservation {
        case: case.name,
        candidate_limit: case.candidate_limit,
        physical_requests,
        recorded_attempts,
        http_status,
        one_logical_request,
        passed,
    }
}

#[cfg(test)]
mod tests;
