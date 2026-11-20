use super::*;
use axum::{
    body::to_bytes,
    extract::{Path, RawQuery},
};
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Start {
    request_id: Uuid,
    operation_id: Uuid,
    reason: Reason,
    seconds: i32,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Reason {
    RequestTiming,
    StreamInterruption,
    PendingOutcome,
}

impl Reason {
    fn code(self) -> &'static str {
        match self {
            Self::RequestTiming => "request_timing",
            Self::StreamInterruption => "stream_interruption",
            Self::PendingOutcome => "pending_outcome",
        }
    }
}

fn invalid() -> Response {
    http::error(
        StatusCode::BAD_REQUEST,
        "invalid_capture",
        "Invalid diagnostic capture request.",
    )
}

async fn response(data: Result<Option<Value>, sqlx::Error>) -> Response {
    let v = match data {
        Ok(Some(v)) => v,
        Ok(None) => return denied(),
        Err(_) => return unavailable(),
    };
    let (status, code, message) = match v.get("error").and_then(Value::as_str) {
        None => return http::success(v),
        Some("invalid_capture") => return invalid(),
        Some("not_found") => return http::not_found().await,
        Some("fresh_passkey_required") => (
            StatusCode::FORBIDDEN,
            "fresh_passkey_required",
            "Verify the owner passkey again.",
        ),
        Some("capture_disabled") => (
            StatusCode::CONFLICT,
            "capture_disabled",
            "Diagnostic capture is not configured.",
        ),
        Some("capture_busy") => (
            StatusCode::CONFLICT,
            "capture_busy",
            "A diagnostic capture is already active.",
        ),
        Some("capture_request_unavailable") => (
            StatusCode::CONFLICT,
            "capture_request_unavailable",
            "The request is not in progress.",
        ),
        Some("capture_conflict") => (
            StatusCode::CONFLICT,
            "capture_conflict",
            "The operation has a different capture scope.",
        ),
        Some("capture_retired") => (
            StatusCode::CONFLICT,
            "capture_retired",
            "This capture is no longer retained.",
        ),
        Some("capture_history_full") => (
            StatusCode::TOO_MANY_REQUESTS,
            "capture_history_full",
            "The diagnostic capture retention limit was reached.",
        ),
        _ => return unavailable(),
    };
    http::error(status, code, message)
}

pub(super) async fn list(
    State(s): State<App>,
    headers: HeaderMap,
    RawQuery(query): RawQuery,
) -> Response {
    read(&s, &headers, query, None).await
}

pub(super) async fn detail(
    State(s): State<App>,
    headers: HeaderMap,
    RawQuery(query): RawQuery,
    Path(id): Path<String>,
) -> Response {
    read(&s, &headers, query, Some(&id)).await
}

async fn read(s: &App, headers: &HeaderMap, query: Option<String>, raw: Option<&str>) -> Response {
    let (Some(admin), Some(token)) = (&s.auth, browser::cookie(headers, SESSION)) else {
        return denied();
    };
    if query.is_some() {
        return invalid();
    }
    let id = match raw.map(Uuid::parse_str).transpose() {
        Ok(id) => id,
        Err(_) => return http::not_found().await,
    };
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return unavailable();
    }
    let mut data = sqlx::query_scalar::<_, Option<Value>>(
        "SELECT relay_hosted.owner_diagnostic_captures($1,$2,$3)",
    )
    .bind(&s.origin)
    .bind(admin.tag(b"session-v1", &token).as_ref())
    .bind(id)
    .fetch_one(&s.pool)
    .await;
    if id.is_none()
        && let Ok(Some(Value::Object(data))) = &mut data
    {
        data.insert("config".into(), serde_json::json!(s.capture));
    }
    response(data).await
}

pub(super) async fn start(State(s): State<App>, req: Request) -> Response {
    let proof = match owner_mutation(&s, req.headers()) {
        Ok(tag) => tag,
        Err(e) => return e.response(),
    };
    if req.uri().query().is_some()
        || browser::one(req.headers(), "content-type").and_then(|v| v.split(';').next())
            != Some("application/json")
    {
        return invalid();
    }
    let Ok(Ok(bytes)) =
        tokio::time::timeout(Duration::from_secs(5), to_bytes(req.into_body(), 512)).await
    else {
        return invalid();
    };
    let Ok(start) = serde_json::from_slice::<Start>(&bytes) else {
        return invalid();
    };
    if start.operation_id.is_nil()
        || start.request_id.is_nil()
        || !(1..=1800).contains(&start.seconds)
    {
        return invalid();
    }
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return unavailable();
    }
    response(
        sqlx::query_scalar(
            "SELECT relay_hosted.start_diagnostic_capture($1,$2,$3,$4,$5::text::relay_hosted.capture_reason,$6,$7,$8,$9,$10)",
        )
        .bind(&s.origin)
        .bind(proof.as_ref())
        .bind(start.operation_id)
        .bind(start.request_id)
        .bind(start.reason.code())
        .bind(start.seconds)
        .bind(s.capture.map(|c| c.max_seconds))
        .bind(s.capture.map(|c| c.max_events))
        .bind(s.capture.map(|c| c.max_bytes))
        .bind(s.capture.map(|c| c.max_events_per_second))
        .fetch_one(&s.pool)
        .await,
    )
    .await
}

pub(super) async fn stop(State(s): State<App>, Path(raw): Path<String>, req: Request) -> Response {
    let proof = match owner_mutation(&s, req.headers()) {
        Ok(tag) => tag,
        Err(e) => return e.response(),
    };
    let Ok(id) = Uuid::parse_str(&raw) else {
        return http::not_found().await;
    };
    if req.uri().query().is_some()
        || !matches!(
            tokio::time::timeout(Duration::from_secs(5), to_bytes(req.into_body(), 0)).await,
            Ok(Ok(v)) if v.is_empty()
        )
    {
        return invalid();
    }
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return unavailable();
    }
    response(
        sqlx::query_scalar("SELECT relay_hosted.stop_diagnostic_capture($1,$2,$3)")
            .bind(&s.origin)
            .bind(proof.as_ref())
            .bind(id)
            .fetch_one(&s.pool)
            .await,
    )
    .await
}
