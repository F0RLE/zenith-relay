use super::*;
use axum::extract::RawQuery;
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    after: Option<Uuid>,
}

pub(super) fn cursor(q: Option<&str>) -> Result<Option<Uuid>, ()> {
    let q = q.unwrap_or_default();
    if q.len() > 64 {
        return Err(());
    }
    serde_urlencoded::from_str::<Cursor>(q)
        .map(|v| v.after)
        .map_err(|_| ())
}

pub(super) async fn read(State(s): State<App>, h: HeaderMap, RawQuery(q): RawQuery) -> Response {
    let (Some(admin), Some(token)) = (&s.auth, browser::cookie(&h, SESSION)) else {
        return denied();
    };
    let Ok(after) = cursor(q.as_deref()) else {
        return http::error(
            StatusCode::BAD_REQUEST,
            "invalid_cursor",
            "Invalid connection cursor.",
        );
    };
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return unavailable();
    }
    match sqlx::query_scalar::<_, Option<Value>>("SELECT relay_hosted.owner_connections($1,$2,$3)")
        .bind(&s.origin)
        .bind(admin.tag(b"session-v1", &token).as_ref())
        .bind(after)
        .fetch_one(&s.pool)
        .await
    {
        Ok(Some(page)) => http::success(page),
        Ok(None) => denied(),
        Err(_) => unavailable(),
    }
}
