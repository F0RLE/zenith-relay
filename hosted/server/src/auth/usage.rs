use super::*;
use axum::extract::{Path, RawQuery};
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Filter {
    hours: Option<i32>,
    from_ms: Option<i64>,
    to_ms: Option<i64>,
    model: Option<String>,
    workspace_id: Option<Uuid>,
    key_id: Option<Uuid>,
    source_id: Option<Uuid>,
    execution: Option<String>,
    accounting: Option<String>,
    stage: Option<String>,
    origin: Option<String>,
    category: Option<String>,
    severity: Option<String>,
    code: Option<String>,
    before: Option<Uuid>,
}

fn invalid() -> Response {
    http::error(
        StatusCode::BAD_REQUEST,
        "invalid_filter",
        "Invalid usage filter.",
    )
}

async fn result(value: Result<Option<Value>, sqlx::Error>) -> Response {
    match value {
        Ok(Some(v)) => match v.get("error").and_then(Value::as_str) {
            None => http::success(v),
            Some("invalid_filter") => invalid(),
            Some("cursor_unavailable") => http::error(
                StatusCode::BAD_REQUEST,
                "cursor_unavailable",
                "History cursor is no longer available.",
            ),
            Some("not_found") => http::not_found().await,
            _ => unavailable(),
        },
        Ok(None) => denied(),
        Err(_) => unavailable(),
    }
}

pub(super) async fn requests(
    State(s): State<App>,
    h: HeaderMap,
    RawQuery(raw): RawQuery,
) -> Response {
    list(&s, &h, raw, false).await
}

pub(super) async fn errors(
    State(s): State<App>,
    h: HeaderMap,
    RawQuery(raw): RawQuery,
) -> Response {
    list(&s, &h, raw, true).await
}

async fn list(s: &App, h: &HeaderMap, raw: Option<String>, errors: bool) -> Response {
    let (Some(admin), Some(token)) = (&s.auth, browser::cookie(h, SESSION)) else {
        return denied();
    };
    let raw = raw.as_deref().unwrap_or_default();
    if raw.len() > 1024 {
        return invalid();
    }
    let Ok(f) = serde_urlencoded::from_str::<Filter>(raw) else {
        return invalid();
    };
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return unavailable();
    }
    result(
        sqlx::query_scalar(
            "SELECT relay_hosted.owner_request_page($1,$2,$3,$4,$5,
                ROW($6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16)::relay_hosted.owner_request_filter,$17,$18)",
        )
        .bind(&s.origin)
        .bind(admin.tag(b"session-v1", &token).as_ref())
        .bind(f.hours)
        .bind(f.from_ms)
        .bind(f.to_ms)
        .bind(f.model)
        .bind(f.workspace_id)
        .bind(f.key_id)
        .bind(f.source_id)
        .bind(f.execution)
        .bind(f.accounting)
        .bind(f.stage)
        .bind(f.origin)
        .bind(f.category)
        .bind(f.severity)
        .bind(f.code)
        .bind(f.before)
        .bind(errors)
        .fetch_one(&s.pool)
        .await,
    )
    .await
}

pub(super) async fn detail(
    State(s): State<App>,
    h: HeaderMap,
    Path(raw): Path<String>,
) -> Response {
    let (Some(admin), Some(token)) = (&s.auth, browser::cookie(&h, SESSION)) else {
        return denied();
    };
    let Ok(id) = Uuid::parse_str(&raw) else {
        return http::not_found().await;
    };
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return unavailable();
    }
    result(
        sqlx::query_scalar("SELECT relay_hosted.owner_request_detail($1,$2,$3)")
            .bind(&s.origin)
            .bind(admin.tag(b"session-v1", &token).as_ref())
            .bind(id)
            .fetch_one(&s.pool)
            .await,
    )
    .await
}
