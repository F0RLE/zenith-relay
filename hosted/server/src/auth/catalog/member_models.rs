use super::*;
use crate::qualification::valid_model;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelFlag {
    model: String,
    allowed: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestChange {
    operation_id: Uuid,
    scope_id: Uuid,
    expected_source_revision: String,
    expected_revision: String,
    changes: Vec<ModelFlag>,
}

pub(in crate::auth) struct Change {
    source: Uuid,
    operation_id: Uuid,
    scope_id: Uuid,
    source_revision: i64,
    revision: i64,
    flags: Value,
}

impl Change {
    fn prepare(source: Uuid, cmd: RequestChange) -> Option<Self> {
        if source.is_nil()
            || cmd.operation_id.is_nil()
            || cmd.scope_id.is_nil()
            || !(1..=128).contains(&cmd.changes.len())
        {
            return None;
        }
        let mut flags = serde_json::Map::new();
        for flag in cmd.changes {
            if !valid_model(&flag.model)
                || flags
                    .insert(flag.model, Value::Bool(flag.allowed))
                    .is_some()
            {
                return None;
            }
        }
        Some(Self {
            source,
            operation_id: cmd.operation_id,
            scope_id: cmd.scope_id,
            source_revision: revision(&cmd.expected_source_revision)?,
            revision: revision(&cmd.expected_revision)?,
            flags: Value::Object(flags),
        })
    }

    async fn execute(
        &self,
        s: &App,
        tag: &hmac::Tag,
        proof: Option<(&Context, &Approval)>,
    ) -> sqlx::Result<String> {
        let query = sqlx::query_scalar(
            "SELECT relay_hosted.change_member_models($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14)",
        )
        .bind(&s.origin)
        .bind(tag.as_ref())
        .bind(self.source)
        .bind(self.operation_id)
        .bind(self.scope_id)
        .bind(self.source_revision)
        .bind(self.revision)
        .bind(&self.flags);
        confirmation::bind_proof(query, proof)
            .fetch_one(&s.pool)
            .await
    }

    pub(in crate::auth) async fn apply(
        &self,
        s: &App,
        ctx: &Context,
        proof: &Approval,
    ) -> Response {
        respond(
            self.execute(s, &ctx.tag, Some((ctx, proof))).await,
            self.operation_id,
        )
        .await
    }
}

pub(in crate::auth) async fn read(
    State(s): State<App>,
    Path(source): Path<Uuid>,
    RawQuery(query): RawQuery,
    h: HeaderMap,
) -> Response {
    let (Some(admin), Some(token)) = (&s.auth, browser::cookie(&h, SESSION)) else {
        return denied();
    };
    if query.is_some() || source.is_nil() {
        return invalid();
    }
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return unavailable();
    }
    match sqlx::query_scalar::<_, Option<Value>>(
        "SELECT relay_hosted.owner_member_models($1,$2,$3)",
    )
    .bind(&s.origin)
    .bind(admin.tag(b"session-v1", &token).as_ref())
    .bind(source)
    .fetch_one(&s.pool)
    .await
    {
        Ok(Some(view)) if view.get("error").is_none() => http::success(view),
        Ok(Some(_)) => http::not_found().await,
        Ok(None) => denied(),
        Err(_) => unavailable(),
    }
}

pub(in crate::auth) async fn receipt(
    State(s): State<App>,
    Path(source): Path<Uuid>,
    RawQuery(query): RawQuery,
    req: Request,
) -> Response {
    submit(s, source, query, req, false).await
}

pub(in crate::auth) async fn options(
    State(s): State<App>,
    Path(source): Path<Uuid>,
    RawQuery(query): RawQuery,
    req: Request,
) -> Response {
    submit(s, source, query, req, true).await
}

async fn submit(
    s: App,
    source: Uuid,
    query: Option<String>,
    req: Request,
    prepare: bool,
) -> Response {
    let tag = match owner_mutation(&s, req.headers()) {
        Ok(tag) => tag,
        Err(e) => return e.response(),
    };
    if query.is_some() {
        return invalid();
    }
    let Some(change) = crate::setup::credential::<RequestChange>(req)
        .await
        .and_then(|cmd| Change::prepare(source, cmd))
    else {
        return invalid();
    };
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return unavailable();
    }
    let result = change.execute(&s, &tag, None).await;
    if prepare && matches!(result.as_deref(), Ok("fresh_required")) {
        let admin = s.auth.as_ref().expect("authenticated");
        return confirmation::start(
            &s,
            &admin.supply_changes,
            tag,
            supply_confirmation::Command::MemberModels(change),
        )
        .await;
    }
    respond(result, change.operation_id).await
}

fn invalid() -> Response {
    http::error(
        StatusCode::BAD_REQUEST,
        "invalid_member_models",
        "Invalid member model policy.",
    )
}

async fn respond(result: sqlx::Result<String>, operation_id: Uuid) -> Response {
    match result.as_deref() {
        Ok("applied") => http::success(json!({"applied":true,"operation_id":operation_id})),
        Ok("missing") => http::not_found().await,
        Ok("denied") => denied(),
        Ok("invalid") => invalid(),
        Ok("conflict") => http::error(
            StatusCode::CONFLICT,
            "member_models_changed",
            "Reload the current member model policy.",
        ),
        Ok(code) => confirmation::error(code).unwrap_or_else(unavailable),
        Err(_) => unavailable(),
    }
}
