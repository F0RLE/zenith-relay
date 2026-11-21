use super::{App, clear_browser_session, credential, denied, unavailable};
use crate::{db, http};
use axum::{
    Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::Response,
    routing::{delete, get},
};
use serde_json::{Value, json};
use uuid::Uuid;

pub(super) fn routes() -> Router<App> {
    Router::new()
        .route("/api/hosted/v1/account/sessions", get(list))
        .route("/api/hosted/v1/account/sessions/{id}", delete(revoke))
}

async fn list(State(s): State<App>, headers: HeaderMap) -> Response {
    let auth = match credential(&s, &headers, false) {
        Ok(auth) => auth,
        Err(error) => return error.response(),
    };
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return unavailable();
    }
    match sqlx::query_scalar::<_, Option<Value>>(
        "SELECT relay_hosted.read_customer_sessions($1,$2,$3,$4)",
    )
    .bind(&s.origin)
    .bind(auth.provider)
    .bind(auth.binding)
    .bind(auth.session.as_ref())
    .fetch_one(&s.pool)
    .await
    {
        Ok(Some(sessions)) => http::success(json!({"sessions":sessions})),
        Ok(None) => denied(),
        Err(_) => unavailable(),
    }
}

async fn revoke(State(s): State<App>, Path(id): Path<String>, headers: HeaderMap) -> Response {
    let auth = match credential(&s, &headers, true) {
        Ok(auth) => auth,
        Err(error) => return error.response(),
    };
    let Ok(id) = Uuid::parse_str(&id) else {
        return http::error(
            StatusCode::BAD_REQUEST,
            "invalid_session_id",
            "Invalid session ID.",
        );
    };
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return unavailable();
    }
    match sqlx::query_scalar::<_, String>(
        "SELECT relay_hosted.revoke_customer_session($1,$2,$3,$4,$5)",
    )
    .bind(&s.origin)
    .bind(auth.provider)
    .bind(auth.binding)
    .bind(auth.session.as_ref())
    .bind(id)
    .fetch_one(&s.pool)
    .await
    .as_deref()
    {
        Ok("current") => clear_browser_session(
            &s,
            &headers,
            http::success(json!({"id":id,"revoked":true,"signed_out":true})),
        ),
        Ok("other") => http::success(json!({"id":id,"revoked":true,"signed_out":false})),
        Ok("missing") => http::not_found().await,
        Ok("denied") => denied(),
        _ => unavailable(),
    }
}
