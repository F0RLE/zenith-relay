use super::*;
use axum::extract::{Path, RawQuery};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::types::Json;
use uuid::Uuid;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Bounds {
    model: String,
    max_input_bytes: i32,
    max_output_tokens: i32,
    tokens_per_byte: i32,
    overhead_tokens: i32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    operation_id: Uuid,
    expected_revision: String,
    inputs: Bounds,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReceiptQuery {
    operation: Option<Uuid>,
}

pub(super) struct Change {
    source: Uuid,
    op: Uuid,
    revision: i64,
    inputs: Bounds,
    snapshot: Option<String>,
}

impl Change {
    fn parse(source: Uuid, input: Input) -> Option<Self> {
        let revision: i64 = input.expected_revision.parse().ok()?;
        let b = &input.inputs;
        if source.is_nil()
            || input.operation_id.is_nil()
            || revision < 0
            || input.expected_revision != revision.to_string()
            || !crate::qualification::valid_model(&b.model)
            || !crate::qualification::valid_bounds(
                b.max_input_bytes,
                b.max_output_tokens,
                b.tokens_per_byte,
                b.overhead_tokens,
            )
        {
            return None;
        }
        Some(Self {
            source,
            revision,
            op: input.operation_id,
            inputs: input.inputs,
            snapshot: None,
        })
    }

    async fn execute(
        &self,
        s: &App,
        tag: &hmac::Tag,
        proof: Option<(&confirmation::Context, &confirmation::Approval)>,
    ) -> sqlx::Result<Value> {
        confirmation::bind_proof(
            sqlx::query_scalar("SELECT relay_hosted.approve_web_model_check($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)")
                .bind(&s.origin).bind(tag.as_ref()).bind(self.source).bind(self.op)
                .bind(self.revision).bind(Json(&self.inputs)).bind(&self.snapshot),
            proof,
        )
        .fetch_one(&s.pool)
        .await
    }

    pub(super) async fn apply(
        &self,
        s: &App,
        ctx: &confirmation::Context,
        proof: &confirmation::Approval,
    ) -> Response {
        respond(self.execute(s, &ctx.tag, Some((ctx, proof))).await, self.op)
    }
}

pub(super) async fn options(
    State(s): State<App>,
    Path(source): Path<Uuid>,
    req: Request,
) -> Response {
    let tag = match owner_mutation(&s, req.headers()) {
        Ok(tag) => tag,
        Err(e) => return e.response(),
    };
    let admin = s.auth.as_ref().expect("authenticated");
    if !admin.web_model_checks {
        return http::not_found().await;
    }
    let Some(mut command) = crate::setup::credential::<Input>(req)
        .await
        .and_then(|input| Change::parse(source, input))
    else {
        return invalid();
    };
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return unavailable();
    }
    let result = command.execute(&s, &tag, None).await;
    if let Ok(data) = &result
        && data["state"] == "fresh_required"
        && let Some(snapshot) = data["snapshot"].as_str().filter(|s| s.len() == 64)
    {
        command.snapshot = Some(snapshot.into());
        return confirmation::start(
            &s,
            &admin.supply_changes,
            tag,
            supply_confirmation::Command::Check(command),
        )
        .await;
    }
    respond(result, command.op)
}

pub(super) async fn read(
    State(s): State<App>,
    Path(source): Path<Uuid>,
    RawQuery(q): RawQuery,
    h: HeaderMap,
) -> Response {
    let (Some(admin), Some(token)) = (&s.auth, browser::cookie(&h, SESSION)) else {
        return denied();
    };
    if !admin.web_model_checks {
        return http::not_found().await;
    }
    let Ok(query) = connections::parse_query::<ReceiptQuery>(q.as_deref(), 64) else {
        return invalid();
    };
    if source.is_nil() || query.operation.is_some_and(|op| op.is_nil()) {
        return invalid();
    }
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return unavailable();
    }
    match sqlx::query_scalar::<_, Option<Value>>(
        "SELECT relay_hosted.owner_web_model_check($1,$2,$3,$4)",
    )
    .bind(&s.origin)
    .bind(admin.tag(b"session-v1", &token).as_ref())
    .bind(source)
    .bind(query.operation)
    .fetch_one(&s.pool)
    .await
    {
        Ok(Some(data)) => http::success(data),
        Ok(None) => denied(),
        Err(_) => unavailable(),
    }
}

fn invalid() -> Response {
    http::error(
        StatusCode::BAD_REQUEST,
        "invalid_model_check",
        "Invalid model check.",
    )
}

fn respond(result: sqlx::Result<Value>, op: Uuid) -> Response {
    match result.as_ref().ok().and_then(|v| v["state"].as_str()) {
        Some("approved") => http::success(json!({"applied":true,"operation_id":op})),
        Some("denied") => denied(),
        Some("invalid") => invalid(),
        Some("busy") => http::error(
            StatusCode::CONFLICT,
            "model_check_running",
            "A check is already pending.",
        ),
        Some("already_verified") => http::error(
            StatusCode::CONFLICT,
            "model_check_already_verified",
            "This model already has a current check.",
        ),
        Some("conflict") => http::error(
            StatusCode::CONFLICT,
            "model_check_changed",
            "The connection, test scope or operation changed.",
        ),
        Some("limit") => http::error(
            StatusCode::TOO_MANY_REQUESTS,
            "model_check_limit",
            "Model check limit reached.",
        ),
        Some(other) => confirmation::error(other).unwrap_or_else(unavailable),
        None => unavailable(),
    }
}
