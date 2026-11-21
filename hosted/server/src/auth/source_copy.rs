use super::*;
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;
use zenith_relay_hosted_device::{CopyPlan, CopyReview};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PreviewInput {
    code: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ApprovalInput {
    code: String,
    operation_id: Uuid,
    expected_device_revision: String,
}

struct Review {
    plan: CopyPlan,
    verifier: [u8; 32],
    commitment: [u8; 32],
}

pub(super) struct Approval {
    review: Review,
    revision: i64,
}

impl Review {
    fn parse(s: &App, code: &str) -> Option<Self> {
        let review = CopyReview::from_code(code).filter(|r| r.plan.origin == s.origin)?;
        Some(Self {
            verifier: review.verifier_bytes()?,
            commitment: review.commitment_bytes()?,
            plan: review.plan,
        })
    }

    async fn submit(
        &self,
        s: &App,
        tag: &hmac::Tag,
        revision: Option<i64>,
        approval: Option<(&confirmation::Context, &confirmation::Approval)>,
    ) -> Result<Value, sqlx::Error> {
        let p = &self.plan;
        confirmation::bind_proof(
            sqlx::query_scalar(
                "SELECT relay_hosted.review_source_copy($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16)",
            )
            .bind(&s.origin)
            .bind(tag.as_ref())
            .bind(p.installation_id)
            .bind(p.device_id)
            .bind(p.operation_id)
            .bind(p.source_id)
            .bind(sqlx::types::Json(p))
            .bind(self.verifier.as_slice())
            .bind(self.commitment.as_slice())
            .bind(revision),
            approval,
        )
        .fetch_one(&s.pool)
        .await
    }
}

impl ApprovalInput {
    fn command(self, s: &App) -> Option<Approval> {
        let review = Review::parse(s, &self.code)?;
        let revision = self.expected_device_revision.parse::<i64>().ok()?;
        if self.operation_id != review.plan.operation_id
            || !(1..=3).contains(&revision)
            || revision.to_string() != self.expected_device_revision
        {
            return None;
        }
        Some(Approval { review, revision })
    }
}

impl Approval {
    pub(super) async fn apply(
        self,
        s: &App,
        ctx: &confirmation::Context,
        proof: &confirmation::Approval,
    ) -> Response {
        let result = self
            .review
            .submit(s, &ctx.tag, Some(self.revision), Some((ctx, proof)))
            .await;
        outcome(result, Some(self.review.plan.operation_id))
    }
}

pub(super) async fn review(State(s): State<App>, request: Request) -> Response {
    let tag = match owner_mutation(&s, request.headers()) {
        Ok(tag) => tag,
        Err(error) => return error.response(),
    };
    let Some(review) = crate::setup::credential::<PreviewInput>(request)
        .await
        .and_then(|input| Review::parse(&s, &input.code))
    else {
        return invalid();
    };
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return unavailable();
    }
    outcome(review.submit(&s, &tag, None, None).await, None)
}

pub(super) async fn options(State(s): State<App>, request: Request) -> Response {
    let tag = match owner_mutation(&s, request.headers()) {
        Ok(tag) => tag,
        Err(error) => return error.response(),
    };
    let Some(command) = crate::setup::credential::<ApprovalInput>(request)
        .await
        .and_then(|input| input.command(&s))
    else {
        return invalid();
    };
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return unavailable();
    }
    let result = command
        .review
        .submit(&s, &tag, Some(command.revision), None)
        .await;
    if matches!(&result, Ok(v) if v["state"] == "fresh_required") {
        let admin = s.auth.as_ref().expect("authenticated");
        return confirmation::start(
            &s,
            &admin.desktop_changes,
            tag,
            desktop_confirmation::Command::Copy(command),
        )
        .await;
    }
    outcome(result, Some(command.review.plan.operation_id))
}

fn outcome(result: Result<Value, sqlx::Error>, op: Option<Uuid>) -> Response {
    let Ok(data) = result else {
        return unavailable();
    };
    match (data["state"].as_str(), op) {
        (Some("applied"), Some(id)) => desktop_confirmation::receipt(id),
        (Some("awaiting_approval" | "approved" | "expired" | "committed"), None) => {
            http::success(data)
        }
        (Some("denied"), _) => denied(),
        (Some("invalid"), _) => invalid(),
        (Some("conflict"), _) => http::error(
            StatusCode::CONFLICT,
            "copy_changed",
            "Source copy already exists or the device is no longer eligible.",
        ),
        (Some("limit"), _) => http::error(
            StatusCode::TOO_MANY_REQUESTS,
            "copy_limit",
            "Pending copy limit reached.",
        ),
        (Some(other), _) => confirmation::error(other).unwrap_or_else(unavailable),
        _ => unavailable(),
    }
}

fn invalid() -> Response {
    http::error(
        StatusCode::BAD_REQUEST,
        "invalid_copy_review",
        "Invalid source copy review.",
    )
}
