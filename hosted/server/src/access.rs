//! Customer-owned inference credentials. Managing or verifying one does not admit a request.
mod secret;
pub(crate) use secret::proof;

use crate::{db, http, identity};
use axum::{
    Router,
    extract::{Path, RawQuery, Request, State},
    http::{HeaderMap, StatusCode},
    response::Response,
    routing::{delete, get},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
use uuid::Uuid;

type App = Arc<http::StateData>;

pub(crate) fn routes() -> Router<App> {
    Router::new()
        .route("/api/hosted/v1/keys", get(list).post(issue))
        .route("/api/hosted/v1/keys/{id}", delete(revoke))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Filter {
    operation_id: Option<Uuid>,
    before: Option<Uuid>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Issue {
    operation_id: Uuid,
    name: String,
    replaces_key_id: Option<Uuid>,
    expires_at_s: Option<i64>,
}

fn invalid() -> Response {
    http::error(
        StatusCode::BAD_REQUEST,
        "invalid_key_request",
        "Invalid key request.",
    )
}

async fn list(State(s): State<App>, h: HeaderMap, RawQuery(raw): RawQuery) -> Response {
    let auth = match identity::credential(&s, &h, false) {
        Ok(a) => a,
        Err(error) => return error.response(),
    };
    let query = raw.as_deref().unwrap_or("");
    if query.len() > 128 {
        return invalid();
    }
    let Ok(f) = serde_urlencoded::from_str::<Filter>(query) else {
        return invalid();
    };
    if f.operation_id.is_some() && f.before.is_some() {
        return invalid();
    }
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return identity::unavailable();
    }
    match sqlx::query_scalar::<_, Option<Value>>(
        "SELECT relay_hosted.read_customer_keys($1,$2,$3,$4,$5,$6)",
    )
    .bind(&s.origin)
    .bind(auth.provider)
    .bind(auth.binding)
    .bind(auth.session.as_ref())
    .bind(f.operation_id)
    .bind(f.before)
    .fetch_one(&s.pool)
    .await
    {
        Ok(Some(Value::Array(mut rows))) => {
            let more = rows.len() > 32;
            rows.truncate(32);
            let next = more.then(|| &rows[31]["id"]);
            http::success(json!({"keys":rows,"next_before":next}))
        }
        Ok(None) => identity::denied(),
        _ => identity::unavailable(),
    }
}

async fn issue(State(s): State<App>, req: Request) -> Response {
    let auth = match identity::credential(&s, req.headers(), true) {
        Ok(a) => a,
        Err(error) => return error.response(),
    };
    let Some(draft) = crate::setup::credential::<Issue>(req).await else {
        return invalid();
    };
    if draft.operation_id.is_nil()
        || draft.replaces_key_id.is_some_and(|id| id.is_nil())
        || !(1..=80).contains(&draft.name.chars().count())
        || draft.name.trim() != draft.name
        || draft.name.chars().any(char::is_control)
    {
        return invalid();
    }
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return identity::unavailable();
    }
    let Some(fresh) = secret::mint() else {
        return identity::unavailable();
    };
    let result = sqlx::query_as::<_, (String, Option<Value>)>(
        "SELECT * FROM relay_hosted.issue_customer_key($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
    )
    .bind(&s.origin)
    .bind(auth.provider)
    .bind(auth.binding)
    .bind(auth.session.as_ref())
    .bind(draft.operation_id)
    .bind(draft.name)
    .bind(draft.replaces_key_id)
    .bind(draft.expires_at_s)
    .bind(fresh.id)
    .bind(fresh.verifier.as_ref())
    .fetch_one(&s.pool)
    .await;
    match result {
        Ok((kind, Some(key))) if kind == "issued" => {
            http::success(json!({"key":key,"secret":fresh.bearer.as_str()}))
        }
        Ok((kind, Some(key))) if kind == "existing" => http::success(json!({"key":key})),
        Ok((kind, _)) if kind == "denied" => identity::denied(),
        Ok((kind, _)) if kind == "missing" => http::not_found().await,
        Ok((kind, _)) if kind == "expiry" => invalid(),
        Ok((kind, _)) if kind == "conflict" => http::error(
            StatusCode::CONFLICT,
            "key_conflict",
            "Key operation conflicts with current state.",
        ),
        Ok((kind, _)) if kind == "limit" => http::error(
            StatusCode::TOO_MANY_REQUESTS,
            "key_issue_limit",
            "Key issuance limit reached for this qualification.",
        ),
        _ => identity::unavailable(),
    }
}

async fn revoke(State(s): State<App>, Path(id): Path<String>, headers: HeaderMap) -> Response {
    let auth = match identity::credential(&s, &headers, true) {
        Ok(a) => a,
        Err(error) => return error.response(),
    };
    let Ok(id) = Uuid::parse_str(&id) else {
        return invalid();
    };
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return identity::unavailable();
    }
    match sqlx::query_scalar::<_, String>("SELECT relay_hosted.revoke_customer_key($1,$2,$3,$4,$5)")
        .bind(&s.origin)
        .bind(auth.provider)
        .bind(auth.binding)
        .bind(auth.session.as_ref())
        .bind(id)
        .fetch_one(&s.pool)
        .await
        .as_deref()
    {
        Ok("revoked") => http::success(json!({"id":id,"revoked":true})),
        Ok("missing") => http::not_found().await,
        Ok("denied") => identity::denied(),
        _ => identity::unavailable(),
    }
}

/// Authenticates ingress. The send claim separately fences the grant and hold.
pub async fn qualified_workspace(
    pool: &sqlx::PgPool,
    origin: &str,
    bearer: &str,
) -> Result<Option<Uuid>, sqlx::Error> {
    let Some((id, verifier)) = secret::proof(bearer) else {
        return Ok(None);
    };
    sqlx::query_scalar("SELECT relay_hosted.qualified_key_workspace($1,$2,$3)")
        .bind(origin)
        .bind(id)
        .bind(verifier.as_ref())
        .fetch_one(pool)
        .await
}
