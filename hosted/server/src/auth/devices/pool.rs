use super::*;
use axum::extract::Path;
use zenith_relay_hosted_device::PoolChange;

pub(in crate::auth) async fn connections(
    State(s): State<App>,
    h: HeaderMap,
    RawQuery(q): RawQuery,
) -> Response {
    read_inventory(s, h, q, true).await
}

pub(in crate::auth) async fn prepare(State(s): State<App>, req: Request) -> Response {
    let Some((iid, device, proof)) = device_proof(&s, req.headers()) else {
        return device_denied();
    };
    let Some(cmd) = crate::setup::credential::<PoolChange>(req)
        .await
        .filter(PoolChange::valid)
    else {
        return invalid_change();
    };
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return unavailable();
    }
    match sqlx::query_scalar::<_, Option<Value>>(
        "SELECT relay_hosted.prepare_desktop_pool_review($1,$2,$3,$4,$5,$6,$7)",
    )
    .bind(&s.origin)
    .bind(iid)
    .bind(device)
    .bind(proof.as_slice())
    .bind(cmd.source_id)
    .bind(cmd.action.as_str())
    .bind(&cmd.expected_revision)
    .fetch_one(&s.pool)
    .await
    {
        Ok(Some(v)) => review_response(v).await,
        Ok(None) => device_denied(),
        Err(_) => unavailable(),
    }
}

pub(in crate::auth) async fn review(
    State(s): State<App>,
    Path(op): Path<Uuid>,
    headers: HeaderMap,
) -> Response {
    let (Some(admin), Some(token)) = (&s.auth, browser::cookie(&headers, SESSION)) else {
        return denied();
    };
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return unavailable();
    }
    match sqlx::query_scalar::<_, Option<Value>>(
        "SELECT relay_hosted.owner_desktop_pool_review($1,$2,$3)",
    )
    .bind(&s.origin)
    .bind(admin.tag(b"session-v1", &token).as_ref())
    .bind(op)
    .fetch_one(&s.pool)
    .await
    {
        Ok(Some(v)) => review_response(v).await,
        Ok(None) => denied(),
        Err(_) => unavailable(),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    operation_id: Uuid,
}

pub(in crate::auth) struct Approval(Uuid);

pub(in crate::auth) async fn options(State(s): State<App>, req: Request) -> Response {
    let tag = match owner_mutation(&s, req.headers()) {
        Ok(tag) => tag,
        Err(error) => return error.response(),
    };
    let Some(input) = crate::setup::credential::<Input>(req)
        .await
        .filter(|cmd| !cmd.operation_id.is_nil())
    else {
        return invalid_change();
    };
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return unavailable();
    }
    let cmd = Approval(input.operation_id);
    let result = cmd.submit(&s, &tag, None).await;
    if matches!(result.as_deref(), Ok("fresh_required")) {
        let admin = s.auth.as_ref().expect("authenticated");
        return confirmation::start(
            &s,
            &admin.desktop_changes,
            tag,
            desktop_confirmation::Command::Pool(cmd),
        )
        .await;
    }
    applied(input.operation_id, result).await
}

impl Approval {
    async fn submit(
        &self,
        s: &App,
        tag: &hmac::Tag,
        proof: Option<(&confirmation::Context, &confirmation::Approval)>,
    ) -> sqlx::Result<String> {
        confirmation::bind_proof(
            sqlx::query_scalar(
                "SELECT relay_hosted.apply_desktop_pool_review($1,$2,$3,$4,$5,$6,$7,$8,$9)",
            )
            .bind(&s.origin)
            .bind(tag.as_ref())
            .bind(self.0),
            proof,
        )
        .fetch_one(&s.pool)
        .await
    }

    pub(in crate::auth) async fn apply(
        self,
        s: &App,
        ctx: &confirmation::Context,
        proof: &confirmation::Approval,
    ) -> Response {
        applied(self.0, self.submit(s, &ctx.tag, Some((ctx, proof))).await).await
    }
}

fn invalid_change() -> Response {
    http::error(
        StatusCode::BAD_REQUEST,
        "invalid_pool_review",
        "Invalid pool review.",
    )
}

fn conflict() -> Response {
    http::error(
        StatusCode::CONFLICT,
        "desktop_pool_changed",
        "The device or pool review changed.",
    )
}

async fn review_response(v: Value) -> Response {
    match v.get("error").and_then(Value::as_str) {
        None => http::success(v),
        Some("invalid") => invalid_change(),
        Some("conflict") => conflict(),
        Some("missing") => http::not_found().await,
        _ => unavailable(),
    }
}

async fn applied(op: Uuid, result: sqlx::Result<String>) -> Response {
    match result.as_deref() {
        Ok("applied") => desktop_confirmation::receipt(op),
        Ok("denied") => denied(),
        Ok("conflict") => conflict(),
        Ok("missing") => http::not_found().await,
        Ok(other) => confirmation::error(other).unwrap_or_else(unavailable),
        Err(_) => unavailable(),
    }
}
