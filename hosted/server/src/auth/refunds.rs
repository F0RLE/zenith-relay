use super::*;
use axum::extract::{Path, RawQuery};
use confirmation::{Approval, Context};
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

pub(super) struct Decision {
    id: Uuid,
    body: Review,
}

impl Decision {
    async fn execute(
        &self,
        s: &App,
        tag: &hmac::Tag,
        proof: Option<(&Context, &Approval)>,
    ) -> sqlx::Result<String> {
        let query = sqlx::query_scalar(
            "SELECT relay_hosted.review_payment_refund($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)",
        )
        .bind(&s.origin)
        .bind(tag.as_ref())
        .bind(self.id)
        .bind(self.body.operation_id)
        .bind(&self.body.expected_state)
        .bind(&self.body.action);
        confirmation::bind_proof(query, proof)
            .fetch_one(&s.pool)
            .await
    }

    pub(super) async fn apply(
        &self,
        s: &App,
        tag: &hmac::Tag,
        proof: Option<(&Context, &Approval)>,
    ) -> Response {
        respond(self.execute(s, tag, proof).await, self.body.operation_id).await
    }
}

fn invalid() -> Response {
    http::error(
        StatusCode::BAD_REQUEST,
        "invalid_refund",
        "Invalid refund review.",
    )
}

pub(super) async fn review(State(s): State<App>, Path(id): Path<String>, req: Request) -> Response {
    submit(s, &id, req, false).await
}

pub(super) async fn options(
    State(s): State<App>,
    Path(id): Path<String>,
    req: Request,
) -> Response {
    submit(s, &id, req, true).await
}

async fn submit(s: App, id: &str, req: Request, prepare: bool) -> Response {
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
    let decision = Decision { id, body };
    let result = decision.execute(&s, &tag, None).await;
    if prepare && matches!(result.as_deref(), Ok("fresh_required")) {
        let admin = s.auth.as_ref().expect("authenticated");
        let command = payment_confirmation::Command::Refund(decision);
        return confirmation::start(&s, &admin.payment_changes, tag, command).await;
    }
    respond(result, decision.body.operation_id).await
}

async fn respond(result: sqlx::Result<String>, operation_id: Uuid) -> Response {
    match result.as_deref() {
        Ok("applied") => http::success(json!({"applied":true,"operation_id":operation_id})),
        Ok("denied") => denied(),
        Ok("invalid") => invalid(),
        Ok("missing") => http::not_found().await,
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
        Ok(code) => confirmation::error(code).unwrap_or_else(unavailable),
        Err(_) => unavailable(),
    }
}
