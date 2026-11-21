use super::*;
use crate::setup;
use axum::extract::Path;
use confirmation::{Approval, Context};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

pub(super) async fn read(
    State(s): State<App>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let (Some(admin), Some(token)) = (&s.auth, browser::cookie(&headers, SESSION)) else {
        return denied();
    };
    let Ok(id) = Uuid::parse_str(&id) else {
        return http::not_found().await;
    };
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return unavailable();
    }
    match sqlx::query_scalar::<_, Option<Value>>(
        "SELECT relay_hosted.owner_customer_case($1,$2,$3)",
    )
    .bind(&s.origin)
    .bind(admin.tag(b"session-v1", &token).as_ref())
    .bind(id)
    .fetch_one(&s.pool)
    .await
    {
        Ok(Some(value)) if value["error"] == "not_found" => http::not_found().await,
        Ok(Some(value)) => http::success(value),
        Ok(None) => denied(),
        Err(_) => unavailable(),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    operation_id: Uuid,
    expected_revision: String,
    action: String,
    reason_code: String,
}

pub(super) struct Change {
    target: Uuid,
    operation: Uuid,
    expected: i64,
    action: String,
    reason: String,
}

impl Change {
    fn parse(raw: Input, target: Uuid) -> Option<Self> {
        let expected: i64 = raw.expected_revision.parse().ok()?;
        (!target.is_nil()
            && !raw.operation_id.is_nil()
            && (1..i64::MAX).contains(&expected)
            && expected.to_string() == raw.expected_revision
            && matches!(
                (raw.action.as_str(), raw.reason_code.as_str()),
                ("suspend", "security_review" | "policy_review")
                    | ("reinstate", "review_complete")
                    | ("close", "verified_customer_request")
            ))
        .then_some(Self {
            target,
            operation: raw.operation_id,
            expected,
            action: raw.action,
            reason: raw.reason_code,
        })
    }

    async fn execute(
        &self,
        s: &App,
        tag: &hmac::Tag,
        proof: Option<(&Context, &Approval)>,
    ) -> sqlx::Result<String> {
        let query = sqlx::query_scalar(
            "SELECT relay_hosted.change_customer_access($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)",
        )
        .bind(&s.origin)
        .bind(tag.as_ref())
        .bind(self.target)
        .bind(self.operation)
        .bind(self.expected)
        .bind(&self.action)
        .bind(&self.reason);
        confirmation::bind_proof(query, proof)
            .fetch_one(&s.pool)
            .await
    }

    async fn respond(&self, result: sqlx::Result<String>) -> Response {
        match result.as_deref() {
            Ok("applied") => http::success(json!({
                "applied":true,"operation_id":self.operation,"workspace_id":self.target,
                "revision":(self.expected+1).to_string()
            })),
            Ok("conflict") => http::error(
                StatusCode::CONFLICT,
                "customer_access_changed",
                "Customer access changed. Review the current case.",
            ),
            Ok("denied") => denied(),
            Ok("not_found") => http::not_found().await,
            Ok("invalid") => invalid(),
            Ok(code) => confirmation::error(code).unwrap_or_else(unavailable),
            Err(_) => unavailable(),
        }
    }
}

fn invalid() -> Response {
    http::error(
        StatusCode::BAD_REQUEST,
        "invalid_customer_access",
        "Invalid customer access change.",
    )
}

pub(super) async fn receipt(
    State(s): State<App>,
    Path(id): Path<String>,
    req: Request,
) -> Response {
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
        Err(error) => return error.response(),
    };
    let Ok(id) = Uuid::parse_str(id) else {
        return http::not_found().await;
    };
    let Some(change) = setup::credential::<Input>(req)
        .await
        .and_then(|raw| Change::parse(raw, id))
    else {
        return invalid();
    };
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return unavailable();
    }
    let result = change.execute(&s, &tag, None).await;
    if prepare && matches!(result.as_deref(), Ok("fresh_required")) {
        let admin = s.auth.as_ref().expect("authenticated");
        return confirmation::start(&s, &admin.customer_changes, tag, change).await;
    }
    change.respond(result).await
}

pub(super) async fn verify(State(s): State<App>, Path(id): Path<String>, req: Request) -> Response {
    let Some(admin) = &s.auth else {
        return denied();
    };
    let (change, ctx, approval) =
        match confirmation::verify(&s, &admin.customer_changes, &id, req).await {
            Ok(verified) => verified,
            Err(error) => return error.response(),
        };
    let result = change.execute(&s, &ctx.tag, Some((&ctx, &approval))).await;
    change.respond(result).await
}
