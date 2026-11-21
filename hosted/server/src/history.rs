mod reports;
mod status;

use crate::{db, http, identity};
use axum::{
    Router,
    extract::{RawQuery, State},
    http::{HeaderMap, StatusCode},
    response::Response,
    routing::get,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
use uuid::Uuid;

type App = Arc<http::StateData>;
pub(crate) fn routes() -> Router<App> {
    Router::new()
        .merge(reports::routes())
        .route("/api/hosted/v1/models", get(models))
        .route("/api/hosted/v1/models/status", get(status::read))
        .route("/api/hosted/v1/wallet/entries", get(page))
}
async fn models(State(s): State<App>, h: HeaderMap) -> Response {
    let a = match identity::credential(&s, &h, false) {
        Ok(a) => a,
        Err(e) => return e.response(),
    };
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return identity::unavailable();
    }
    match sqlx::query_scalar::<_, Option<Value>>(
        "SELECT relay_hosted.customer_text_models($1,$2,$3,$4)",
    )
    .bind(&s.origin)
    .bind(a.provider)
    .bind(a.binding)
    .bind(a.session.as_ref())
    .fetch_one(&s.pool)
    .await
    {
        Ok(Some(items)) => http::success(json!({"items":items})),
        Ok(None) => identity::denied(),
        Err(_) => identity::unavailable(),
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Filter {
    hours: Option<i32>,
    before: Option<Uuid>,
    id: Option<Uuid>,
}
async fn page(State(s): State<App>, h: HeaderMap, RawQuery(raw): RawQuery) -> Response {
    let a = match identity::credential(&s, &h, false) {
        Ok(a) => a,
        Err(e) => return e.response(),
    };
    let bad = || {
        http::error(
            StatusCode::BAD_REQUEST,
            "invalid_filter",
            "Invalid history filter.",
        )
    };
    let q = raw.as_deref().unwrap_or("");
    if q.len() > 160 {
        return bad();
    }
    let Ok(f) = serde_urlencoded::from_str::<Filter>(q) else {
        return bad();
    };
    let hours = f.hours.unwrap_or(24);
    if !matches!(hours, 24 | 168 | 720) || f.before.is_some() && f.id.is_some() {
        return bad();
    }
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return identity::unavailable();
    }
    match sqlx::query_scalar::<_, Option<Value>>(
        "SELECT relay_hosted.customer_history($1,$2,$3,$4,$5,$6,$7,$8)",
    )
    .bind(&s.origin)
    .bind(a.provider)
    .bind(a.binding)
    .bind(a.session.as_ref())
    .bind("wallet")
    .bind(hours)
    .bind(f.before)
    .bind(f.id)
    .fetch_one(&s.pool)
    .await
    {
        Ok(Some(Value::Array(mut rows))) => {
            let more = rows.len() > 32;
            rows.truncate(32);
            let next = more.then(|| &rows[31]["id"]);
            http::success(json!({"items":rows,"next_before":next}))
        }
        Ok(None) => identity::denied(),
        _ => identity::unavailable(),
    }
}
