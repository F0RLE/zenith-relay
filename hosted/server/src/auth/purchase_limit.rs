use super::*;
use axum::extract::Path;
use confirmation::{Approval, Context};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(untagged)]
enum Limit {
    Capped(String),
    Unlimited(()),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestChange {
    operation_id: Uuid,
    expected_revision: String,
    limit_micro_usd: Limit,
}

pub(super) struct Change {
    source: Uuid,
    operation_id: Uuid,
    revision: i64,
    limit: Option<i64>,
}

impl Change {
    async fn execute(
        &self,
        s: &App,
        tag: &hmac::Tag,
        proof: Option<(&Context, &Approval)>,
    ) -> sqlx::Result<String> {
        let query = sqlx::query_scalar(
            "SELECT relay_hosted.change_purchase_limit($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)",
        )
        .bind(&s.origin)
        .bind(tag.as_ref())
        .bind(self.source)
        .bind(self.operation_id)
        .bind(self.revision)
        .bind(self.limit);
        confirmation::bind_proof(query, proof)
            .fetch_one(&s.pool)
            .await
    }

    pub(super) async fn apply(&self, s: &App, ctx: &Context, proof: &Approval) -> Response {
        respond(
            self.execute(s, &ctx.tag, Some((ctx, proof))).await,
            self.operation_id,
        )
        .await
    }
}

fn nonnegative(value: &str) -> Option<i64> {
    let parsed: i64 = value.parse().ok()?;
    (parsed >= 0 && parsed.to_string() == value).then_some(parsed)
}

fn invalid() -> Response {
    http::error(
        StatusCode::BAD_REQUEST,
        "invalid_purchase_limit",
        "Invalid monthly purchase limit.",
    )
}

pub(super) async fn read(State(s): State<App>, Path(id): Path<Uuid>, h: HeaderMap) -> Response {
    let (Some(admin), Some(token)) = (&s.auth, browser::cookie(&h, SESSION)) else {
        return denied();
    };
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return unavailable();
    }
    match sqlx::query_scalar::<_, Option<Value>>(
        "SELECT relay_hosted.owner_purchase_limit($1,$2,$3)",
    )
    .bind(&s.origin)
    .bind(admin.tag(b"session-v1", &token).as_ref())
    .bind(id)
    .fetch_one(&s.pool)
    .await
    {
        Ok(Some(v)) if v.get("source_id").is_some() => http::success(v),
        Ok(Some(_)) => http::not_found().await,
        Ok(None) => denied(),
        Err(_) => unavailable(),
    }
}

pub(super) async fn change(State(s): State<App>, Path(id): Path<Uuid>, req: Request) -> Response {
    submit(s, id, req, false).await
}

pub(super) async fn options(State(s): State<App>, Path(id): Path<Uuid>, req: Request) -> Response {
    submit(s, id, req, true).await
}

async fn submit(s: App, source: Uuid, req: Request, prepare: bool) -> Response {
    let tag = match owner_mutation(&s, req.headers()) {
        Ok(tag) => tag,
        Err(e) => return e.response(),
    };
    let Some(cmd) = crate::setup::credential::<RequestChange>(req).await else {
        return invalid();
    };
    let Some(revision) = nonnegative(&cmd.expected_revision).filter(|&n| n < i64::MAX) else {
        return invalid();
    };
    if cmd.operation_id.is_nil() {
        return invalid();
    }
    let limit = match cmd.limit_micro_usd {
        Limit::Capped(raw) => {
            let Some(n) = nonnegative(&raw) else {
                return invalid();
            };
            Some(n)
        }
        Limit::Unlimited(()) => None,
    };
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return unavailable();
    }
    let change = Change {
        source,
        operation_id: cmd.operation_id,
        revision,
        limit,
    };
    let result = change.execute(&s, &tag, None).await;
    if prepare && matches!(result.as_deref(), Ok("fresh_required")) {
        let admin = s.auth.as_ref().expect("authenticated");
        let command = supply_confirmation::Command::Limit(change);
        return confirmation::start(&s, &admin.supply_changes, tag, command).await;
    }
    respond(result, change.operation_id).await
}

async fn respond(result: sqlx::Result<String>, operation_id: Uuid) -> Response {
    match result.as_deref() {
        Ok("applied") => http::success(json!({"applied":true,"operation_id":operation_id})),
        Ok("denied") => denied(),
        Ok("missing") => http::not_found().await,
        Ok("invalid") => invalid(),
        Ok("conflict") => http::error(
            StatusCode::CONFLICT,
            "purchase_limit_changed",
            "Review the current purchase limit.",
        ),
        Ok(code) => confirmation::error(code).unwrap_or_else(unavailable),
        Err(_) => unavailable(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unlimited_must_be_explicit_and_usd_is_exact() {
        let mut body = json!({"operation_id":Uuid::new_v4(),"expected_revision":"0"});
        assert!(serde_json::from_value::<RequestChange>(body.clone()).is_err());
        for value in [json!(null), json!("0"), json!("9223372036854775807")] {
            body["limit_micro_usd"] = value;
            assert!(serde_json::from_value::<RequestChange>(body.clone()).is_ok());
        }
        body["limit_micro_usd"] = json!(0);
        assert!(serde_json::from_value::<RequestChange>(body).is_err());
        assert_eq!(nonnegative("9223372036854775807"), Some(i64::MAX));
        for invalid in ["", "01", " 1", "+1", "-0", "-1", "9223372036854775808"] {
            assert_eq!(nonnegative(invalid), None);
        }
    }
}
