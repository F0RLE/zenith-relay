use crate::{db, http, identity};
use axum::{
    Router,
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    response::Response,
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::HashSet, sync::Arc};
use uuid::Uuid;

type App = Arc<http::StateData>;

pub(crate) fn routes() -> Router<App> {
    Router::new()
        .route("/api/hosted/v1/notifications", get(list))
        .route("/api/hosted/v1/notifications/read", post(mark))
}

async fn list(State(s): State<App>, headers: HeaderMap) -> Response {
    let a = match identity::credential(&s, &headers, false) {
        Ok(a) => a,
        Err(e) => return e.response(),
    };
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return identity::unavailable();
    }
    match sqlx::query_scalar::<_, Option<Value>>(
        "SELECT relay_hosted.customer_service_notices($1,$2,$3,$4)",
    )
    .bind(&s.origin)
    .bind(a.provider)
    .bind(a.binding)
    .bind(a.session.as_ref())
    .fetch_one(&s.pool)
    .await
    {
        Ok(Some(items)) => http::success(items),
        Ok(None) => identity::denied(),
        Err(_) => identity::unavailable(),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Mark {
    ids: Vec<Uuid>,
}

fn invalid() -> Response {
    http::error(
        StatusCode::BAD_REQUEST,
        "invalid_notices",
        "Invalid notification selection.",
    )
}

async fn mark(State(s): State<App>, req: Request) -> Response {
    let a = match identity::credential(&s, req.headers(), true) {
        Ok(a) => a,
        Err(e) => return e.response(),
    };
    let Some(cmd) = crate::setup::credential::<Mark>(req).await else {
        return invalid();
    };
    if !(1..=32).contains(&cmd.ids.len())
        || cmd.ids.iter().any(Uuid::is_nil)
        || cmd.ids.iter().collect::<HashSet<_>>().len() != cmd.ids.len()
    {
        return invalid();
    }
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return identity::unavailable();
    }
    let result =
        sqlx::query_scalar::<_, String>("SELECT relay_hosted.read_service_notices($1,$2,$3,$4,$5)")
            .bind(&s.origin)
            .bind(a.provider)
            .bind(a.binding)
            .bind(a.session.as_ref())
            .bind(cmd.ids)
            .fetch_one(&s.pool)
            .await;
    match result.as_deref() {
        Ok("applied") => http::success(json!({"applied":true})),
        Ok("denied") => identity::denied(),
        Ok("missing") => http::not_found().await,
        Ok("invalid") => invalid(),
        _ => identity::unavailable(),
    }
}
