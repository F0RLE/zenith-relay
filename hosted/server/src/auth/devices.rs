use super::*;
use axum::extract::RawQuery;
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;
use zenith_relay_hosted_device::{DeviceKey, ReadScope, valid_read_scopes};

pub(super) mod observations;
pub(super) mod pool;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    operation_id: Uuid,
    device_id: Uuid,
    expected_revision: String,
    action: String,
    label: Option<String>,
    approval_code: Option<String>,
    read_scopes: Option<Vec<ReadScope>>,
    manage_pool: Option<bool>,
}

pub(super) struct Change {
    operation: Uuid,
    device: Uuid,
    revision: i64,
    action: String,
    label: Option<String>,
    device_proof: Option<[u8; 32]>,
    read_scopes: Vec<ReadScope>,
    manage_pool: bool,
}

impl Input {
    fn command(self) -> Option<Change> {
        let revision = self.expected_revision.parse::<i64>().ok()?;
        let device_proof = self
            .approval_code
            .as_deref()
            .and_then(|code| DeviceKey::review_proof(self.device_id, code));
        if self.device_id.is_nil()
            || self.operation_id.is_nil()
            || !(0..=2).contains(&revision)
            || revision.to_string() != self.expected_revision
            || !matches!(self.action.as_str(), "invite" | "approve" | "revoke")
            || (self.action == "invite") != self.label.is_some()
            || self.label.as_deref().is_some_and(|v| {
                v.trim() != v || v.is_empty() || v.len() > 64 || v.chars().any(char::is_control)
            })
            || (self.action == "approve") != self.approval_code.is_some()
            || self.approval_code.is_some() != device_proof.is_some()
            || self
                .read_scopes
                .as_deref()
                .is_some_and(|scopes| self.action == "revoke" || !valid_read_scopes(scopes))
            || (self.action == "revoke" && self.manage_pool == Some(true))
        {
            return None;
        }
        Some(Change {
            operation: self.operation_id,
            device: self.device_id,
            revision,
            action: self.action,
            label: self.label,
            device_proof,
            read_scopes: self.read_scopes.unwrap_or_default(),
            manage_pool: self.manage_pool.unwrap_or(false),
        })
    }
}

pub(super) async fn list(State(s): State<App>, h: HeaderMap) -> Response {
    let (Some(admin), Some(token)) = (&s.auth, browser::cookie(&h, SESSION)) else {
        return denied();
    };
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return unavailable();
    }
    match sqlx::query_scalar::<_, Option<Value>>("SELECT relay_hosted.owner_devices($1,$2)")
        .bind(&s.origin)
        .bind(admin.tag(b"session-v1", &token).as_ref())
        .fetch_one(&s.pool)
        .await
    {
        Ok(Some(v)) => http::success(v),
        Ok(None) => denied(),
        Err(_) => unavailable(),
    }
}

pub(super) async fn change(State(s): State<App>, req: Request) -> Response {
    prepare(s, req, false).await
}

pub(super) async fn options(State(s): State<App>, req: Request) -> Response {
    prepare(s, req, true).await
}

async fn prepare(s: App, req: Request, challenge: bool) -> Response {
    let tag = match owner_mutation(&s, req.headers()) {
        Ok(tag) => tag,
        Err(error) => return error.response(),
    };
    let Some(command) = crate::setup::credential::<Input>(req)
        .await
        .and_then(Input::command)
    else {
        return invalid();
    };
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return unavailable();
    }
    let result = command.submit(&s, &tag, None).await;
    if challenge && matches!(result.as_deref(), Ok("fresh_required")) {
        let admin = s.auth.as_ref().expect("authenticated");
        return confirmation::start(
            &s,
            &admin.desktop_changes,
            tag,
            desktop_confirmation::Command::Device(command),
        )
        .await;
    }
    outcome(command.operation, result)
}

impl Change {
    async fn submit(
        &self,
        s: &App,
        tag: &hmac::Tag,
        approval: Option<(&confirmation::Context, &confirmation::Approval)>,
    ) -> Result<String, sqlx::Error> {
        let scopes: Vec<_> = self.read_scopes.iter().map(|s| s.as_str()).collect();
        confirmation::bind_proof(
            sqlx::query_scalar(
                "SELECT relay_hosted.change_device($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16)",
            )
            .bind(&s.origin)
            .bind(tag.as_ref())
            .bind(self.operation)
            .bind(self.device)
            .bind(self.revision)
            .bind(&self.action)
            .bind(&self.label)
            .bind(self.device_proof.as_ref().map(|p| p.as_slice()))
            .bind(scopes)
            .bind(self.manage_pool),
            approval,
        )
        .fetch_one(&s.pool)
        .await
    }

    pub(super) async fn apply(
        self,
        s: &App,
        ctx: &confirmation::Context,
        proof: &confirmation::Approval,
    ) -> Response {
        outcome(
            self.operation,
            self.submit(s, &ctx.tag, Some((ctx, proof))).await,
        )
    }
}

fn invalid() -> Response {
    http::error(
        StatusCode::BAD_REQUEST,
        "invalid_pairing",
        "Invalid device review.",
    )
}

fn outcome(op: Uuid, result: Result<String, sqlx::Error>) -> Response {
    match result.as_deref() {
        Ok("applied") => desktop_confirmation::receipt(op),
        Ok("denied") => denied(),
        Ok("invalid") => invalid(),
        Ok("conflict") => http::error(
            StatusCode::CONFLICT,
            "device_changed",
            "The device review changed or expired.",
        ),
        Ok("limit") => http::error(
            StatusCode::TOO_MANY_REQUESTS,
            "device_limit",
            "Device limit reached.",
        ),
        Ok(other) => confirmation::error(other).unwrap_or_else(unavailable),
        Err(_) => unavailable(),
    }
}

fn device_denied() -> Response {
    http::error(
        StatusCode::UNAUTHORIZED,
        "device_unauthorized",
        "Approved device with the required grant needed.",
    )
}

fn device_proof(s: &App, h: &HeaderMap) -> Option<(Uuid, Uuid, [u8; 32])> {
    if s.auth.is_none() || h.contains_key(header::COOKIE) || h.contains_key(header::ORIGIN) {
        return None;
    }
    browser::one(h, "authorization")
        .and_then(|h| h.strip_prefix("Bearer "))
        .and_then(|b| DeviceKey::presented(&s.origin, b))
}

pub(super) async fn inventory(
    State(s): State<App>,
    h: HeaderMap,
    RawQuery(q): RawQuery,
) -> Response {
    read_inventory(s, h, q, false).await
}

async fn read_inventory(s: App, h: HeaderMap, q: Option<String>, pool: bool) -> Response {
    let Some((iid, device, proof)) = device_proof(&s, &h) else {
        return device_denied();
    };
    let Ok(after) = connections::cursor(q.as_deref()) else {
        return http::error(
            StatusCode::BAD_REQUEST,
            "invalid_cursor",
            "Invalid connection cursor.",
        );
    };
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return unavailable();
    }
    let query = if pool {
        "SELECT relay_hosted.device_pool_connections($1,$2,$3,$4,$5)"
    } else {
        "SELECT relay_hosted.device_connections($1,$2,$3,$4,$5)"
    };
    match sqlx::query_scalar::<_, Option<Value>>(query)
        .bind(&s.origin)
        .bind(iid)
        .bind(device)
        .bind(proof.as_slice())
        .bind(after)
        .fetch_one(&s.pool)
        .await
    {
        Ok(Some(page)) => http::success(page),
        Ok(None) => device_denied(),
        Err(_) => unavailable(),
    }
}
