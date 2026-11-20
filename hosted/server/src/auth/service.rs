use super::*;
use serde::Deserialize;
use serde_json::json;
use subtle::ConstantTimeEq;
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

pub(super) async fn transition(State(s): State<App>, req: Request) -> Response {
    let headers = req.headers();
    let (Some(admin), Some(token)) = (&s.auth, browser::cookie(headers, SESSION)) else {
        return denied();
    };
    if !browser::same_origin(headers, &s.origin)
        || !browser::one(headers, "x-relay-csrf")
            .and_then(browser::decode)
            .is_some_and(|v| bool::from(admin.tag(b"csrf-v1", &token).as_ref().ct_eq(v.as_ref())))
    {
        return forbidden();
    }
    let invalid = || {
        http::error(
            StatusCode::BAD_REQUEST,
            "invalid_scope",
            "Invalid identity scope.",
        )
    };
    let Some(change) = crate::setup::credential::<Transition>(req).await else {
        return invalid();
    };
    let Ok(revision) = change.expected_revision.parse::<i64>() else {
        return invalid();
    };
    if revision < 1
        || revision.to_string() != change.expected_revision
        || change.operation_id.is_nil()
        || change.candidate_ids.len() > 16
        || change.candidate_ids.iter().any(Uuid::is_nil)
    {
        return invalid();
    }
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return unavailable();
    }
    let result = sqlx::query_scalar::<_, String>(
        "SELECT relay_hosted.transition_customer_scope($1,$2,$3,$4,$5,$6,$7,$8,$9)",
    )
    .bind(&s.origin)
    .bind(admin.tag(b"session-v1", &token).as_ref())
    .bind(change.operation_id)
    .bind(revision)
    .bind(change.target)
    .bind(change.reason_code)
    .bind(change.candidate_ids)
    .bind(s.customer.as_ref().map(|c| c.provider_id()))
    .bind(s.customer.as_ref().map(|c| c.binding()))
    .fetch_one(&s.pool)
    .await;
    match result.as_deref() {
        Ok("applied") => http::success(json!({"operation_id":change.operation_id,"applied":true})),
        Ok("denied") => denied(),
        Ok("fresh_required") => http::error(
            StatusCode::FORBIDDEN,
            "fresh_passkey_required",
            "Verify the owner passkey again.",
        ),
        Ok("invalid") => invalid(),
        Ok("conflict") => http::error(
            StatusCode::CONFLICT,
            "revision_conflict",
            "The service configuration changed.",
        ),
        _ => unavailable(),
    }
}
