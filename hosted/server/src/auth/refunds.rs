use super::*;
use axum::extract::{Path, RawQuery};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Filter {
    payment_id: Option<Uuid>,
    before: Option<Uuid>,
}

pub(super) async fn list(
    State(s): State<App>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
) -> Response {
    let (Some(admin), Some(token)) = (&s.auth, browser::cookie(&headers, SESSION)) else {
        return denied();
    };
    let raw = raw.as_deref().unwrap_or_default();
    let Some(filter) = (raw.len() <= 96)
        .then(|| serde_urlencoded::from_str::<Filter>(raw).ok())
        .flatten()
    else {
        return invalid();
    };
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return unavailable();
    }
    match sqlx::query_scalar::<_, Option<Value>>(
        "SELECT relay_hosted.owner_refund_cases($1,$2,$3,$4)",
    )
    .bind(&s.origin)
    .bind(admin.tag(b"session-v1", &token).as_ref())
    .bind(filter.payment_id)
    .bind(filter.before)
    .fetch_one(&s.pool)
    .await
    {
        Ok(Some(Value::Array(mut rows))) => {
            let more = rows.len() > 32;
            rows.truncate(32);
            let next = more.then(|| &rows[31]["id"]);
            http::success(json!({"items":rows,"next_before":next}))
        }
        Ok(None) => denied(),
        _ => unavailable(),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Review {
    operation_id: Uuid,
    expected_state: String,
    action: String,
}

fn invalid() -> Response {
    http::error(
        StatusCode::BAD_REQUEST,
        "invalid_refund",
        "Invalid refund review.",
    )
}

pub(super) async fn review(State(s): State<App>, Path(id): Path<String>, req: Request) -> Response {
    let tag = match owner_mutation(&s, req.headers()) {
        Ok(tag) => tag,
        Err(e) => return e.response(),
    };
    let Ok(id) = id.parse::<Uuid>() else {
        return http::not_found().await;
    };
    let Some(body) = crate::setup::credential::<Review>(req).await else {
        return invalid();
    };
    if body.operation_id.is_nil()
        || body.expected_state != "requested"
        || !matches!(body.action.as_str(), "approve" | "decline")
    {
        return invalid();
    }
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return unavailable();
    }
    let result = sqlx::query_scalar::<_, String>(
        "SELECT relay_hosted.review_payment_refund($1,$2,$3,$4,$5,$6)",
    )
    .bind(&s.origin)
    .bind(tag.as_ref())
    .bind(id)
    .bind(body.operation_id)
    .bind(body.expected_state)
    .bind(body.action)
    .fetch_one(&s.pool)
    .await;
    match result.as_deref() {
        Ok("applied") => http::success(json!({"applied":true,"operation_id":body.operation_id})),
        Ok("denied") => denied(),
        Ok("invalid") => invalid(),
        Ok("missing") => http::not_found().await,
        Ok("fresh_required") => http::error(
            StatusCode::FORBIDDEN,
            "fresh_passkey_required",
            "Verify the owner passkey again.",
        ),
        Ok("conflict") => http::error(
            StatusCode::CONFLICT,
            "refund_conflict",
            "The refund request has changed.",
        ),
        Ok("remainder") => http::error(
            StatusCode::CONFLICT,
            "refund_remainder",
            "The amount exceeds the refundable credit.",
        ),
        Ok("balance") => http::error(
            StatusCode::CONFLICT,
            "refund_balance",
            "The wallet has insufficient available funds.",
        ),
        Ok("unavailable") => http::error(
            StatusCode::FORBIDDEN,
            "refund_closed",
            "Refunds are limited to local qualification.",
        ),
        _ => unavailable(),
    }
}
