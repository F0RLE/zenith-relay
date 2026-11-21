use super::*;
use axum::extract::RawQuery;
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Filter {
    hours: Option<i32>,
    model: Option<String>,
}

pub(super) async fn overview(
    State(s): State<App>,
    headers: HeaderMap,
    RawQuery(query): RawQuery,
) -> Response {
    let (Some(admin), Some(token)) = (&s.auth, browser::cookie(&headers, SESSION)) else {
        return denied();
    };
    let invalid = || {
        http::error(
            StatusCode::BAD_REQUEST,
            "invalid_filter",
            "Invalid overview filter.",
        )
    };
    let raw = query.as_deref().unwrap_or_default();
    if raw.len() > 416 {
        return invalid();
    }
    let Ok(filter) = serde_urlencoded::from_str::<Filter>(raw) else {
        return invalid();
    };
    let hours = filter.hours.unwrap_or(24);
    if !matches!(hours, 0 | 24 | 168 | 720) {
        return invalid();
    }
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return unavailable();
    }
    let result = sqlx::query_scalar::<_, Option<Value>>(
        "SELECT relay_hosted.owner_usage_overview($1,$2,$3,$4)",
    )
    .bind(&s.origin)
    .bind(admin.tag(b"session-v1", &token).as_ref())
    .bind(hours)
    .bind(filter.model)
    .fetch_one(&s.pool)
    .await;
    match result {
        Ok(Some(v)) if v.get("error").is_none() => http::success(v),
        Ok(Some(_)) => invalid(),
        Ok(None) => denied(),
        Err(_) => unavailable(),
    }
}
