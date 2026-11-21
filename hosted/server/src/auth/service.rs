use super::*;
use axum::extract::Path;
use confirmation::{Approval, Context};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Transition {
    operation_id: Uuid,
    expected_revision: String,
    target: String,
    reason_code: String,
    candidate_ids: Vec<Uuid>,
}

pub(super) struct Change {
    transition: Transition,
    revision: i64,
}

impl Change {
    async fn parse(req: Request) -> Option<Self> {
        let transition = crate::setup::credential::<Transition>(req).await?;
        let revision = transition.expected_revision.parse::<i64>().ok()?;
        if revision < 1
            || revision == i64::MAX
            || revision.to_string() != transition.expected_revision
            || transition.operation_id.is_nil()
            || transition.candidate_ids.len() > 16
            || transition.candidate_ids.iter().any(Uuid::is_nil)
        {
            return None;
        }
        Some(Self {
            transition,
            revision,
        })
    }
}

pub(super) async fn read(State(s): State<App>, headers: HeaderMap) -> Response {
    let (Some(admin), Some(token)) = (&s.auth, browser::cookie(&headers, SESSION)) else {
        return denied();
    };
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return unavailable();
    }
    let result =
        sqlx::query_as::<_, (String, i64)>("SELECT * FROM relay_hosted.owner_service_view($1,$2)")
            .bind(&s.origin)
            .bind(admin.tag(b"session-v1", &token).as_ref())
            .fetch_optional(&s.pool)
            .await;
    match result {
        Ok(Some((mode, revision))) => http::success(json!({
            "mode":mode,"revision":revision.to_string(),"identity_configured":s.customer.is_some(),
        })),
        Ok(None) => denied(),
        Err(_) => unavailable(),
    }
}

pub(super) async fn receipt(State(s): State<App>, req: Request) -> Response {
    let tag = match owner_mutation(&s, req.headers()) {
        Ok(tag) => tag,
        Err(error) => return error.response(),
    };
    let Some(change) = Change::parse(req).await else {
        return invalid();
    };
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return unavailable();
    }
    let result = apply(&s, &tag, &change, None).await;
    respond(result, change.transition.operation_id)
}

pub(super) async fn options(State(s): State<App>, req: Request) -> Response {
    let tag = match owner_mutation(&s, req.headers()) {
        Ok(tag) => tag,
        Err(error) => return error.response(),
    };
    let Some(change) = Change::parse(req).await else {
        return invalid();
    };
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return unavailable();
    }
    let result = apply(&s, &tag, &change, None).await;
    if !matches!(result.as_deref(), Ok("fresh_required")) {
        return respond(result, change.transition.operation_id);
    }

    let admin = s.auth.as_ref().expect("authenticated");
    confirmation::start(&s, &admin.transitions, tag, change).await
}

pub(super) async fn verify(State(s): State<App>, Path(id): Path<String>, req: Request) -> Response {
    let Some(admin) = &s.auth else {
        return denied();
    };
    let (change, ctx, approval) = match confirmation::verify(&s, &admin.transitions, &id, req).await
    {
        Ok(verified) => verified,
        Err(error) => return error.response(),
    };
    let result = apply(&s, &ctx.tag, &change, Some((&ctx, &approval))).await;
    respond(result, change.transition.operation_id)
}

async fn apply(
    s: &App,
    tag: &hmac::Tag,
    change: &Change,
    approval: Option<(&Context, &Approval)>,
) -> sqlx::Result<String> {
    let cmd = &change.transition;
    let query = sqlx::query_scalar(
        "SELECT relay_hosted.transition_customer_scope($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15)",
    )
    .bind(&s.origin)
    .bind(tag.as_ref())
    .bind(cmd.operation_id)
    .bind(change.revision)
    .bind(&cmd.target)
    .bind(&cmd.reason_code)
    .bind(&cmd.candidate_ids)
    .bind(s.customer.as_ref().map(|c| c.provider_id()))
    .bind(s.customer.as_ref().map(|c| c.binding()));
    confirmation::bind_proof(query, approval)
        .fetch_one(&s.pool)
        .await
}

fn invalid() -> Response {
    http::error(
        StatusCode::BAD_REQUEST,
        "invalid_scope",
        "Invalid identity scope.",
    )
}

fn respond(result: sqlx::Result<String>, operation_id: Uuid) -> Response {
    match result.as_deref() {
        Ok("applied") => http::success(json!({"operation_id":operation_id,"applied":true})),
        Ok("denied") => denied(),
        Ok("invalid") => invalid(),
        Ok("conflict") => http::error(
            StatusCode::CONFLICT,
            "revision_conflict",
            "The service configuration changed.",
        ),
        Ok(code) => confirmation::error(code).unwrap_or_else(unavailable),
        Err(_) => unavailable(),
    }
}
