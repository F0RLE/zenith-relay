//! Separate loopback worker behind the HTTPS edge; ordinary web serves review metadata only.
use crate::{
    auth::browser,
    http,
    vault::{Error, Vault},
};
use axum::{
    Router,
    body::to_bytes,
    extract::{Request, State},
    http::{HeaderMap, StatusCode, header},
    middleware,
    response::Response,
    routing::{get, post},
};
use serde_json::json;
use std::{sync::Arc, time::Duration};
use zenith_relay_hosted_device::{COPY_COMMIT_PATH, COPY_STATUS_PATH, CopyProof, CopyState};

struct Worker {
    origin: String,
    vault: Vault,
    accepting: tokio::sync::Semaphore,
}

pub async fn router(origin: &str, vault: Vault) -> Result<Router, &'static str> {
    vault.check_copy_worker(origin).await?;
    Ok(Router::new()
        .route(COPY_STATUS_PATH, get(status))
        .route(COPY_COMMIT_PATH, post(commit))
        .fallback(http::not_found)
        .method_not_allowed_fallback(http::wrong_method)
        .with_state(Arc::new(Worker {
            origin: origin.to_owned(),
            vault,
            accepting: tokio::sync::Semaphore::new(4),
        }))
        .layer(middleware::from_fn(http::response_headers)))
}

fn proof(h: &HeaderMap) -> Option<CopyProof> {
    if h.contains_key(header::COOKIE) || h.contains_key(header::ORIGIN) {
        return None;
    }
    let value = browser::one(h, "authorization")?.strip_prefix("Bearer ")?;
    CopyProof::presented(value)
}

fn denied() -> Response {
    http::error(
        StatusCode::UNAUTHORIZED,
        "copy_not_authorized",
        "Source copy authorization required.",
    )
}

fn unavailable() -> Response {
    http::error(
        StatusCode::SERVICE_UNAVAILABLE,
        "copy_result_unconfirmed",
        "Source copy result unavailable.",
    )
}

fn reply(proof: &CopyProof, result: Result<Option<CopyState>, Error>) -> Response {
    let state = match result {
        Ok(Some(v)) => v,
        Ok(None) => return denied(),
        Err(Error::Conflict | Error::Stale) => CopyState::Conflict,
        Err(Error::Invalid) => return invalid(),
        Err(_) => return unavailable(),
    };
    http::success(json!({
        "installation_id":proof.installation_id, "device_id":proof.device_id,
        "operation_id":proof.operation_id, "state":state,
    }))
}

fn invalid() -> Response {
    http::error(
        StatusCode::BAD_REQUEST,
        "invalid_source_copy",
        "Invalid source copy.",
    )
}

async fn status(State(w): State<Arc<Worker>>, h: HeaderMap) -> Response {
    let Some(proof) = proof(&h) else {
        return denied();
    };
    reply(&proof, w.vault.copy_status(&w.origin, &proof).await)
}

async fn commit(State(w): State<Arc<Worker>>, req: Request) -> Response {
    let Some(proof) = proof(req.headers()) else {
        return denied();
    };
    if browser::one(req.headers(), "content-type") != Some("application/octet-stream") {
        return invalid();
    }
    let Ok(_slot) = w.accepting.try_acquire() else {
        return unavailable();
    };
    match w.vault.copy_status(&w.origin, &proof).await {
        Ok(Some(CopyState::Approved)) => (),
        Ok(Some(CopyState::AwaitingApproval)) => return denied(),
        other => return reply(&proof, other),
    }
    let Ok(Ok(body)) =
        tokio::time::timeout(Duration::from_secs(5), to_bytes(req.into_body(), 3073)).await
    else {
        return invalid();
    };
    let Some((provider, proxy)) = std::str::from_utf8(&body)
        .ok()
        .and_then(|v| v.split_once('\n'))
    else {
        return invalid();
    };
    reply(
        &proof,
        w.vault
            .commit_source_copy(
                &w.origin,
                &proof,
                provider,
                (!proxy.is_empty()).then_some(proxy),
            )
            .await,
    )
}
