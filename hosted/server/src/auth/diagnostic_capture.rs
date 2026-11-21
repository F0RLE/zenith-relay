use super::*;
use crate::config::CaptureLimits;
use axum::{
    body::to_bytes,
    extract::{Path, RawQuery},
};
use confirmation::{Approval, Context};
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

pub(super) struct Command {
    start: Start,
    limits: Option<CaptureLimits>,
}

impl Command {
    async fn parse(req: Request, limits: Option<CaptureLimits>) -> Option<Self> {
        if req.uri().query().is_some()
            || browser::one(req.headers(), "content-type").and_then(|v| v.split(';').next())
                != Some("application/json")
        {
            return None;
        }
        let bytes = tokio::time::timeout(Duration::from_secs(5), to_bytes(req.into_body(), 512))
            .await
            .ok()?
            .ok()?;
        let start: Start = serde_json::from_slice(&bytes).ok()?;
        if start.operation_id.is_nil()
            || start.request_id.is_nil()
            || !(1..=1800).contains(&start.seconds)
        {
            return None;
        }
        Some(Self { start, limits })
    }

    async fn apply(
        &self,
        s: &App,
        tag: &hmac::Tag,
        approval: Option<(&Context, &Approval)>,
    ) -> sqlx::Result<Option<Value>> {
        let query = sqlx::query_scalar(
            "SELECT relay_hosted.start_diagnostic_capture($1,$2,$3,$4,$5::text::relay_hosted.capture_reason,
                $6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16)",
        )
        .bind(&s.origin)
        .bind(tag.as_ref())
        .bind(self.start.operation_id)
        .bind(self.start.request_id)
        .bind(self.start.reason.code())
        .bind(self.start.seconds)
        .bind(self.limits.map(|c| c.max_seconds))
        .bind(self.limits.map(|c| c.max_events))
        .bind(self.limits.map(|c| c.max_bytes))
        .bind(self.limits.map(|c| c.max_events_per_second));
        confirmation::bind_proof(query, approval)
            .fetch_one(&s.pool)
            .await
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Reason {
    RequestTiming,
    StreamInterruption,
    PendingOutcome,
}

impl Reason {
    fn code(&self) -> &'static str {
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
        Some(code) => return confirmation::error(code).unwrap_or_else(unavailable),
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

pub(super) async fn receipt(State(s): State<App>, req: Request) -> Response {
    prepare(&s, req, false).await
}

pub(super) async fn options(State(s): State<App>, req: Request) -> Response {
    prepare(&s, req, true).await
}

async fn prepare(s: &App, req: Request, issue_options: bool) -> Response {
    let tag = match owner_mutation(s, req.headers()) {
        Ok(tag) => tag,
        Err(e) => return e.response(),
    };
    let Some(command) = Command::parse(req, s.capture).await else {
        return invalid();
    };
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return unavailable();
    }
    let result = command.apply(s, &tag, None).await;
    if issue_options && matches!(&result, Ok(Some(data)) if data["error"] == "fresh_required") {
        let admin = s.auth.as_ref().expect("authenticated");
        return confirmation::start(s, &admin.capture_starts, tag, command).await;
    }
    response(result).await
}

pub(super) async fn verify(State(s): State<App>, Path(id): Path<String>, req: Request) -> Response {
    let Some(admin) = &s.auth else {
        return denied();
    };
    let (command, ctx, proof) =
        match confirmation::verify(&s, &admin.capture_starts, &id, req).await {
            Ok(verified) => verified,
            Err(error) => return error.response(),
        };
    if command.limits != s.capture {
        return http::error(
            StatusCode::CONFLICT,
            "capture_limits_changed",
            "Review the current diagnostic capture limits.",
        );
    }
    response(command.apply(&s, &ctx.tag, Some((&ctx, &proof))).await).await
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
