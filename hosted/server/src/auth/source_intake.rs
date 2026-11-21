use super::*;
use axum::extract::Path;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashSet;
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    operation_id: Uuid,
    label: String,
    endpoint: String,
    declared_models: Vec<String>,
    proxied: bool,
    verifier_digest: String,
    payload_mac: String,
}

pub(super) struct Approval {
    input: Input,
    digest: Token,
    mac: Token,
}

impl Approval {
    fn parse(input: Input) -> Option<Self> {
        let mut names = HashSet::new();
        if input.operation_id.is_nil()
            || input.label.is_empty()
            || input.label.trim() != input.label
            || input.label.encode_utf16().count() > 80
            || input.label.chars().any(char::is_control)
            || !crate::outbound::valid_text_endpoint(&input.endpoint)
            || input.declared_models.len() > 128
            || !input.declared_models.iter().all(|id| {
                crate::qualification::valid_model(id) && names.insert(id.to_ascii_lowercase())
            })
        {
            return None;
        }
        Some(Self {
            digest: browser::decode(&input.verifier_digest)?,
            mac: browser::decode(&input.payload_mac)?,
            input,
        })
    }

    async fn execute(
        &self,
        s: &App,
        tag: &hmac::Tag,
        proof: Option<(&confirmation::Context, &confirmation::Approval)>,
    ) -> sqlx::Result<String> {
        let input = &self.input;
        confirmation::bind_proof(
            sqlx::query_scalar(
                "SELECT relay_hosted.approve_web_source($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15)",
            )
            .bind(&s.origin)
            .bind(tag.as_ref())
            .bind(input.operation_id)
            .bind(&input.label)
            .bind(&input.endpoint)
            .bind(&input.declared_models)
            .bind(input.proxied)
            .bind(self.digest.as_ref())
            .bind(self.mac.as_ref()),
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
        respond(
            self.execute(s, &ctx.tag, Some((ctx, proof))).await,
            self.input.operation_id,
        )
    }
}

pub(super) async fn options(State(s): State<App>, req: Request) -> Response {
    let tag = match owner_mutation(&s, req.headers()) {
        Ok(tag) => tag,
        Err(e) => return e.response(),
    };
    let admin = s.auth.as_ref().expect("authenticated");
    if !admin.source_intake_enabled {
        return http::not_found().await;
    }
    let Some(command) = crate::setup::credential::<Input>(req)
        .await
        .and_then(Approval::parse)
    else {
        return invalid();
    };
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return unavailable();
    }
    let result = command.execute(&s, &tag, None).await;
    if matches!(result.as_deref(), Ok("fresh_required")) {
        return confirmation::start(
            &s,
            &admin.supply_changes,
            tag,
            supply_confirmation::Command::Intake(command),
        )
        .await;
    }
    respond(result, command.input.operation_id)
}

pub(super) async fn receipt(State(s): State<App>, Path(op): Path<Uuid>, h: HeaderMap) -> Response {
    let (Some(admin), Some(token)) = (&s.auth, browser::cookie(&h, SESSION)) else {
        return denied();
    };
    if op.is_nil() {
        return invalid();
    }
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return unavailable();
    }
    let tag = admin.tag(b"session-v1", &token);
    match sqlx::query_scalar::<_, Option<Value>>("SELECT relay_hosted.web_source_receipt($1,$2,$3)")
        .bind(&s.origin)
        .bind(tag.as_ref())
        .bind(op)
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
        "invalid_source_intake",
        "Invalid source configuration.",
    )
}

fn respond(result: sqlx::Result<String>, op: Uuid) -> Response {
    match result.as_deref() {
        Ok("approved") => http::success(json!({"state":"approved","operation_id":op})),
        Ok("denied") => denied(),
        Ok("invalid") => invalid(),
        Ok("conflict") => http::error(
            StatusCode::CONFLICT,
            "source_intake_conflict",
            "Source operation already exists.",
        ),
        Ok("limit") => http::error(
            StatusCode::TOO_MANY_REQUESTS,
            "source_intake_limit",
            "Pending source approval limit reached.",
        ),
        Ok(other) => confirmation::error(other).unwrap_or_else(unavailable),
        Err(_) => unavailable(),
    }
}
