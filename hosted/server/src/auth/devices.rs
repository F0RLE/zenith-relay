use super::*;
use axum::extract::RawQuery;
use serde::Deserialize;
use serde_json::{Value, json};
use subtle::ConstantTimeEq;
use uuid::Uuid;
use zenith_relay_hosted_device::DeviceKey;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Change {
    operation_id: Uuid,
    device_id: Uuid,
    expected_revision: String,
    action: String,
    label: Option<String>,
    approval_code: Option<String>,
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
    let h = req.headers();
    let (Some(admin), Some(token)) = (&s.auth, browser::cookie(h, SESSION)) else {
        return denied();
    };
    if !browser::same_origin(h, &s.origin)
        || !browser::one(h, "x-relay-csrf")
            .and_then(browser::decode)
            .is_some_and(|v| bool::from(admin.tag(b"csrf-v1", &token).as_ref().ct_eq(v.as_ref())))
    {
        return forbidden();
    }
    let invalid = || {
        http::error(
            StatusCode::BAD_REQUEST,
            "invalid_pairing",
            "Invalid device review.",
        )
    };
    let Some(c) = crate::setup::credential::<Change>(req).await else {
        return invalid();
    };
    let Ok(rev) = c.expected_revision.parse::<i64>() else {
        return invalid();
    };
    let proof = c
        .approval_code
        .as_deref()
        .and_then(|v| DeviceKey::review_proof(c.device_id, v));
    if c.device_id.is_nil()
        || c.operation_id.is_nil()
        || !(0..=2).contains(&rev)
        || rev.to_string() != c.expected_revision
        || !matches!(c.action.as_str(), "invite" | "approve" | "revoke")
        || (c.action == "invite") != c.label.is_some()
        || c.label.as_deref().is_some_and(|v| {
            v.trim() != v || v.is_empty() || v.len() > 64 || v.chars().any(char::is_control)
        })
        || (c.action == "approve") != c.approval_code.is_some()
        || c.approval_code.is_some() != proof.is_some()
    {
        return invalid();
    }
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return unavailable();
    }
    let result = sqlx::query_scalar::<_, String>(
        "SELECT relay_hosted.change_device($1,$2,$3,$4,$5,$6,$7,$8)",
    )
    .bind(&s.origin)
    .bind(admin.tag(b"session-v1", &token).as_ref())
    .bind(c.operation_id)
    .bind(c.device_id)
    .bind(rev)
    .bind(c.action)
    .bind(c.label)
    .bind(proof.as_ref().map(|p| p.as_slice()))
    .fetch_one(&s.pool)
    .await;
    match result.as_deref() {
        Ok("applied") => http::success(json!({"operation_id":c.operation_id,"applied":true})),
        Ok("denied") => denied(),
        Ok("invalid") => invalid(),
        Ok("fresh_required") => http::error(
            StatusCode::FORBIDDEN,
            "fresh_passkey_required",
            "Verify the owner passkey again.",
        ),
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
        _ => unavailable(),
    }
}

pub(super) async fn inventory(
    State(s): State<App>,
    h: HeaderMap,
    RawQuery(q): RawQuery,
) -> Response {
    let denied = || {
        http::error(
            StatusCode::UNAUTHORIZED,
            "device_unauthorized",
            "Approved device required.",
        )
    };
    if s.auth.is_none() || h.contains_key(header::COOKIE) || h.contains_key(header::ORIGIN) {
        return denied();
    }
    let Some((iid, device, proof)) = browser::one(&h, "authorization")
        .and_then(|h| h.strip_prefix("Bearer "))
        .and_then(|b| DeviceKey::presented(&s.origin, b))
    else {
        return denied();
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
    match sqlx::query_scalar::<_, Option<Value>>(
        "SELECT relay_hosted.device_connections($1,$2,$3,$4,$5)",
    )
    .bind(&s.origin)
    .bind(iid)
    .bind(device)
    .bind(proof.as_slice())
    .bind(after)
    .fetch_one(&s.pool)
    .await
    {
        Ok(Some(page)) => http::success(page),
        Ok(None) => denied(),
        Err(_) => unavailable(),
    }
}
