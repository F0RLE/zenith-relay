use crate::{
    db, http,
    identity::{self, CustomerAccessError},
};
use axum::{
    Router,
    extract::{Path, RawQuery, Request, State},
    http::{HeaderMap, StatusCode},
    response::Response,
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tokio::sync::Semaphore;
use uuid::Uuid;

type App = Arc<http::StateData>;

pub(crate) fn routes() -> Router<App> {
    Router::new()
        .route("/api/hosted/v1/payments/methods", get(methods))
        .route("/api/hosted/v1/payments/quotes", post(quote))
        .route("/api/hosted/v1/payments", get(history).post(confirm))
        .route("/api/hosted/v1/payments/{id}", get(detail))
        .route("/api/hosted/v1/payments/{id}/resume", post(resume))
        .route("/api/hosted/v1/payments/{id}/refund-requests", post(refund))
}

async fn authorize<'a>(
    s: &'a App,
    headers: &HeaderMap,
    mutation: bool,
) -> Result<identity::CustomerCredential<'a>, CustomerAccessError> {
    let a = identity::credential(s, headers, mutation)?;
    db::closed_setup(&s.pool, &s.origin)
        .await
        .map_err(|_| CustomerAccessError::Unavailable)?;
    Ok(a)
}

async fn methods(State(s): State<App>, headers: HeaderMap) -> Response {
    let a = match authorize(&s, &headers, false).await {
        Ok(a) => a,
        Err(e) => return e.response(),
    };
    match sqlx::query_scalar::<_, Option<Value>>(
        "SELECT relay_hosted.customer_checkout_methods($1,$2,$3,$4)",
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
struct Quote {
    method_id: Uuid,
    credit_micro_usd: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Confirm {
    quote_id: Uuid,
    operation_id: Uuid,
}

fn invalid() -> Response {
    http::error(
        StatusCode::BAD_REQUEST,
        "invalid_checkout",
        "Invalid checkout selection.",
    )
}

fn credit(value: &str) -> Option<i64> {
    if value.len() > 19 || value.starts_with('0') || !value.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}

async fn quote(State(s): State<App>, req: Request) -> Response {
    let a = match authorize(&s, req.headers(), true).await {
        Ok(a) => a,
        Err(e) => return e.response(),
    };
    let Some(q) = crate::setup::credential::<Quote>(req).await else {
        return invalid();
    };
    let Some(amount) = credit(&q.credit_micro_usd).filter(|_| !q.method_id.is_nil()) else {
        return invalid();
    };
    let result = sqlx::query_as("SELECT * FROM relay_hosted.quote_checkout($1,$2,$3,$4,$5,$6)")
        .bind(&s.origin)
        .bind(a.provider)
        .bind(a.binding)
        .bind(a.session.as_ref())
        .bind(q.method_id)
        .bind(amount)
        .fetch_one(&s.pool)
        .await;
    checkout_result(result, "ready", "quote").await
}

async fn confirm(State(s): State<App>, req: Request) -> Response {
    let a = match authorize(&s, req.headers(), true).await {
        Ok(a) => a,
        Err(e) => return e.response(),
    };
    let Some(c) = crate::setup::credential::<Confirm>(req).await else {
        return invalid();
    };
    if c.quote_id.is_nil() || c.operation_id.is_nil() {
        return invalid();
    }
    let result = sqlx::query_as("SELECT * FROM relay_hosted.confirm_checkout($1,$2,$3,$4,$5,$6)")
        .bind(&s.origin)
        .bind(a.provider)
        .bind(a.binding)
        .bind(a.session.as_ref())
        .bind(c.quote_id)
        .bind(c.operation_id)
        .fetch_one(&s.pool)
        .await;
    checkout_result(result, "accepted", "payment").await
}

async fn checkout_result(
    result: Result<(String, Option<Value>), sqlx::Error>,
    expected: &str,
    key: &str,
) -> Response {
    let Ok((outcome, payload)) = result else {
        return identity::unavailable();
    };
    match outcome.as_str() {
        code if code == expected && payload.is_some() => http::success(json!({key:payload})),
        "denied" => identity::denied(),
        "missing" => http::not_found().await,
        "amount" => invalid(),
        "unavailable" => http::error(
            StatusCode::CONFLICT,
            "payment_method_unavailable",
            "Payment method unavailable.",
        ),
        "conflict" => http::error(
            StatusCode::CONFLICT,
            "checkout_conflict",
            "Payment confirmation conflicts with an existing operation.",
        ),
        "expired" => http::error(StatusCode::GONE, "quote_expired", "Payment quote expired."),
        "quote_limit" => http::error(
            StatusCode::TOO_MANY_REQUESTS,
            "quote_limit",
            "Too many active quotes.",
        ),
        "invoice_limit" => http::error_details(
            StatusCode::TOO_MANY_REQUESTS,
            "open_invoice_limit",
            "Too many open invoices.",
            payload,
        ),
        _ => identity::unavailable(),
    }
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Filter {
    operation_id: Option<Uuid>,
    before: Option<Uuid>,
}

async fn history(State(s): State<App>, headers: HeaderMap, RawQuery(raw): RawQuery) -> Response {
    let raw = raw.as_deref().unwrap_or_default();
    if raw.len() > 128 {
        return invalid();
    }
    let Ok(f) = serde_urlencoded::from_str::<Filter>(raw) else {
        return invalid();
    };
    if f.before.is_some() && f.operation_id.is_some() {
        return invalid();
    }
    read(&s, &headers, None, f).await
}

async fn detail(State(s): State<App>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    let Ok(id) = id.parse::<Uuid>() else {
        return http::not_found().await;
    };
    read(&s, &headers, Some(id), Filter::default()).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RefundRequest {
    operation_id: Uuid,
    amount_micro_usd: String,
}

async fn refund(State(s): State<App>, Path(id): Path<String>, req: Request) -> Response {
    let a = match authorize(&s, req.headers(), true).await {
        Ok(a) => a,
        Err(e) => return e.response(),
    };
    let Ok(id) = id.parse::<Uuid>() else {
        return http::not_found().await;
    };
    let invalid = || {
        http::error(
            StatusCode::BAD_REQUEST,
            "invalid_refund",
            "Enter a positive amount in whole cents.",
        )
    };
    let Some(body) = crate::setup::credential::<RefundRequest>(req).await else {
        return invalid();
    };
    let Some(amount) =
        credit(&body.amount_micro_usd).filter(|n| n % 10_000 == 0 && !body.operation_id.is_nil())
    else {
        return invalid();
    };
    let result = sqlx::query_as::<_, (String, Option<Value>)>(
        "SELECT * FROM relay_hosted.request_payment_refund($1,$2,$3,$4,$5,$6,$7)",
    )
    .bind(&s.origin)
    .bind(a.provider)
    .bind(a.binding)
    .bind(a.session.as_ref())
    .bind(id)
    .bind(body.operation_id)
    .bind(amount)
    .fetch_one(&s.pool)
    .await;
    let Ok((outcome, payload)) = result else {
        return identity::unavailable();
    };
    match outcome.as_str() {
        "accepted" if payload.is_some() => http::success(json!({"refund":payload})),
        "denied" => identity::denied(),
        "missing" => http::not_found().await,
        "amount" => invalid(),
        "conflict" => http::error(
            StatusCode::CONFLICT,
            "refund_conflict",
            "This payment changed or already has an open refund.",
        ),
        "remainder" => http::error(
            StatusCode::CONFLICT,
            "refund_remainder",
            "The amount exceeds the refundable credit.",
        ),
        "unavailable" => http::error(
            StatusCode::FORBIDDEN,
            "refund_closed",
            "Refunds are limited to local qualification.",
        ),
        _ => identity::unavailable(),
    }
}

static RESUME_SLOTS: Semaphore = Semaphore::const_new(16);

async fn resume(State(s): State<App>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    let a = match authorize(&s, &headers, true).await {
        Ok(a) => a,
        Err(e) => return e.response(),
    };
    let Ok(id) = id.parse::<Uuid>() else {
        return http::not_found().await;
    };
    let Ok(_slot) = RESUME_SLOTS.try_acquire() else {
        return identity::unavailable();
    };
    let ticket = Uuid::new_v4();
    let read = |enqueue| {
        sqlx::query_as::<_, (String, Option<Value>, Option<String>)>(
            "SELECT * FROM relay_hosted.customer_checkout_resume($1,$2,$3,$4,$5,$6,$7)",
        )
        .bind(&s.origin)
        .bind(a.provider)
        .bind(a.binding)
        .bind(a.session.as_ref())
        .bind(id)
        .bind(ticket)
        .bind(enqueue)
        .fetch_one(&s.pool)
    };
    let Ok(Ok((outcome, payment, handoff))) = tokio::time::timeout(Duration::from_secs(8), async {
        let mut result = read(true).await?;
        while result.0 == "waiting" {
            tokio::time::sleep(Duration::from_millis(250)).await;
            result = read(false).await?;
        }
        Ok::<_, sqlx::Error>(result)
    })
    .await
    else {
        return identity::unavailable();
    };
    match outcome.as_str() {
        "ready" if payment.is_some() => {
            http::success(json!({"payment":payment,"handoff_url":handoff}))
        }
        "denied" => identity::denied(),
        "missing" => http::not_found().await,
        _ => identity::unavailable(),
    }
}

async fn read(s: &App, headers: &HeaderMap, target: Option<Uuid>, f: Filter) -> Response {
    let a = match authorize(s, headers, false).await {
        Ok(a) => a,
        Err(e) => return e.response(),
    };
    match sqlx::query_scalar::<_, Option<Value>>(
        "SELECT relay_hosted.customer_checkout_history($1,$2,$3,$4,$5,$6,$7)",
    )
    .bind(&s.origin)
    .bind(a.provider)
    .bind(a.binding)
    .bind(a.session.as_ref())
    .bind(target)
    .bind(f.operation_id)
    .bind(f.before)
    .fetch_one(&s.pool)
    .await
    {
        Ok(Some(Value::Array(mut rows))) => {
            if target.is_some() {
                return match rows.pop() {
                    Some(row) => http::success(json!({"payment":row})),
                    None => http::not_found().await,
                };
            }
            let more = rows.len() > 32;
            rows.truncate(32);
            let next = more.then(|| &rows[31]["id"]);
            http::success(json!({"items":rows,"next_before":next}))
        }
        Ok(None) => identity::denied(),
        _ => identity::unavailable(),
    }
}
