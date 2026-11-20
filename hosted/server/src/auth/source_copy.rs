use super::*;
use serde::Deserialize;
use serde_json::Value;
use subtle::ConstantTimeEq;
use zenith_relay_hosted_device::CopyReview;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Review {
    code: String,
    approve: bool,
}

pub(super) async fn review(State(s): State<App>, request: Request) -> Response {
    let h = request.headers();
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
            "invalid_copy_review",
            "Invalid source copy review.",
        )
    };
    let Some(input) = crate::setup::credential::<Review>(request).await else {
        return invalid();
    };
    let Some(review) = CopyReview::from_code(&input.code).filter(|r| r.plan.origin == s.origin)
    else {
        return invalid();
    };
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return unavailable();
    }
    let p = &review.plan;
    let result = sqlx::query_scalar::<_, Value>(
        "SELECT relay_hosted.review_source_copy($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
    )
    .bind(&s.origin)
    .bind(admin.tag(b"session-v1", &token).as_ref())
    .bind(p.installation_id)
    .bind(p.device_id)
    .bind(p.operation_id)
    .bind(p.source_id)
    .bind(sqlx::types::Json(p))
    .bind(review.verifier_bytes().as_ref().map(|v| v.as_slice()))
    .bind(review.commitment_bytes().as_ref().map(|v| v.as_slice()))
    .bind(input.approve)
    .fetch_one(&s.pool)
    .await;
    let Ok(data) = result else {
        return unavailable();
    };
    match data["state"].as_str() {
        Some("awaiting_approval" | "approved" | "expired" | "committed") => http::success(data),
        Some("denied") => denied(),
        Some("invalid") => invalid(),
        Some("fresh_required") => http::error(
            StatusCode::FORBIDDEN,
            "fresh_passkey_required",
            "Verify the owner passkey again.",
        ),
        Some("conflict") => http::error(
            StatusCode::CONFLICT,
            "copy_changed",
            "Source copy already exists or the device is no longer eligible.",
        ),
        Some("limit") => http::error(
            StatusCode::TOO_MANY_REQUESTS,
            "copy_limit",
            "Pending copy limit reached.",
        ),
        _ => unavailable(),
    }
}
