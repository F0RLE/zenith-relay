use super::*;
use std::collections::BTreeSet;

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Mode {
    Automatic,
    Failover,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::auth) struct Change {
    operation_id: Uuid,
    scope_id: Uuid,
    expected_revision: String,
    mode: Mode,
    member_order: Vec<Uuid>,
}

impl Change {
    fn valid(&self) -> bool {
        !self.operation_id.is_nil()
            && !self.scope_id.is_nil()
            && self.expected_revision.len() == 64
            && self
                .expected_revision
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            && self.member_order.len() <= 128
            && (!matches!(self.mode, Mode::Failover) || !self.member_order.is_empty())
            && self.member_order.iter().all(|id| !id.is_nil())
            && self.member_order.iter().collect::<BTreeSet<_>>().len() == self.member_order.len()
    }

    async fn execute(
        &self,
        s: &App,
        tag: &hmac::Tag,
        proof: Option<(&Context, &Approval)>,
    ) -> sqlx::Result<String> {
        let query = sqlx::query_scalar(
            "SELECT relay_hosted.change_pool_rotation($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)",
        )
        .bind(&s.origin)
        .bind(tag.as_ref())
        .bind(self.operation_id)
        .bind(self.scope_id)
        .bind(&self.expected_revision)
        .bind(match self.mode {
            Mode::Automatic => "automatic",
            Mode::Failover => "failover",
        })
        .bind(&self.member_order);
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
    RawQuery(query): RawQuery,
    h: HeaderMap,
) -> Response {
    let (Some(admin), Some(token)) = (&s.auth, browser::cookie(&h, SESSION)) else {
        return denied();
    };
    if query.is_some() {
        return invalid();
    }
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return unavailable();
    }
    match sqlx::query_scalar::<_, Option<Value>>("SELECT relay_hosted.owner_pool_rotation($1,$2)")
        .bind(&s.origin)
        .bind(admin.tag(b"session-v1", &token).as_ref())
        .fetch_one(&s.pool)
        .await
    {
        Ok(Some(view)) if view.get("error").is_none() => http::success(view),
        Ok(Some(view)) if view["error"] == "missing" => http::not_found().await,
        Ok(Some(_)) => capacity(),
        Ok(None) => denied(),
        Err(_) => unavailable(),
    }
}

pub(in crate::auth) async fn receipt(
    State(s): State<App>,
    RawQuery(query): RawQuery,
    req: Request,
) -> Response {
    submit(s, query, req, false).await
}

pub(in crate::auth) async fn options(
    State(s): State<App>,
    RawQuery(query): RawQuery,
    req: Request,
) -> Response {
    submit(s, query, req, true).await
}

async fn submit(s: App, query: Option<String>, req: Request, prepare: bool) -> Response {
    let tag = match owner_mutation(&s, req.headers()) {
        Ok(tag) => tag,
        Err(e) => return e.response(),
    };
    let Some(change) = crate::setup::credential::<Change>(req)
        .await
        .filter(|c| query.is_none() && c.valid())
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
            supply_confirmation::Command::Rotation(change),
        )
        .await;
    }
    respond(result, change.operation_id).await
}

fn invalid() -> Response {
    http::error(
        StatusCode::BAD_REQUEST,
        "invalid_pool_rotation",
        "Invalid pool rotation policy.",
    )
}

fn capacity() -> Response {
    http::error(
        StatusCode::CONFLICT,
        "pool_rotation_capacity",
        "The member list exceeds the rotation editor limit.",
    )
}

async fn respond(result: sqlx::Result<String>, operation_id: Uuid) -> Response {
    match result.as_deref() {
        Ok("applied") => http::success(json!({"applied":true,"operation_id":operation_id})),
        Ok("denied") => denied(),
        Ok("invalid") => invalid(),
        Ok("capacity") => capacity(),
        Ok("conflict") => http::error(
            StatusCode::CONFLICT,
            "pool_rotation_changed",
            "Reload the current rotation policy.",
        ),
        Ok(code) => confirmation::error(code).unwrap_or_else(unavailable),
        Err(_) => unavailable(),
    }
}
