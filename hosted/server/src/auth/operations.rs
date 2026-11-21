use super::*;
use axum::extract::RawQuery;
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    before: Option<String>,
}

fn invalid() -> Response {
    http::error(
        StatusCode::BAD_REQUEST,
        "invalid_filter",
        "Invalid operations cursor.",
    )
}

fn result(data: Result<Option<Value>, sqlx::Error>) -> Response {
    match data {
        Ok(Some(v)) if v.get("error").is_none() => http::success(v),
        Ok(Some(v)) if v["error"] == "cursor_unavailable" => http::error(
            StatusCode::BAD_REQUEST,
            "cursor_unavailable",
            "Operations history cursor is no longer available.",
        ),
        Ok(None) => denied(),
        _ => unavailable(),
    }
}

pub(super) async fn status(State(s): State<App>, headers: HeaderMap) -> Response {
    let (Some(admin), Some(token)) = (&s.auth, browser::cookie(&headers, SESSION)) else {
        return denied();
    };
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return unavailable();
    }
    result(
        sqlx::query_scalar("SELECT relay_hosted.owner_operations_status($1,$2)")
            .bind(&s.origin)
            .bind(admin.tag(b"session-v1", &token).as_ref())
            .fetch_one(&s.pool)
            .await,
    )
}

pub(super) async fn events(
    State(s): State<App>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
) -> Response {
    let (Some(admin), Some(token)) = (&s.auth, browser::cookie(&headers, SESSION)) else {
        return denied();
    };
    let raw = raw.as_deref().unwrap_or_default();
    if raw.len() > 64 {
        return invalid();
    }
    let Ok(cursor) = serde_urlencoded::from_str::<Cursor>(raw) else {
        return invalid();
    };
    let before = if let Some(text) = cursor.before {
        match text.parse::<i64>() {
            Ok(id) if id > 0 && id.to_string() == text => Some(id),
            _ => return invalid(),
        }
    } else {
        None
    };
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return unavailable();
    }
    result(
        sqlx::query_scalar("SELECT relay_hosted.owner_operations_events($1,$2,$3)")
            .bind(&s.origin)
            .bind(admin.tag(b"session-v1", &token).as_ref())
            .bind(before)
            .fetch_one(&s.pool)
            .await,
    )
}
