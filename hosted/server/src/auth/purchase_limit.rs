use super::*;
use axum::extract::Path;
use serde::Deserialize;
use serde_json::{Value, json};
use subtle::ConstantTimeEq;
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(untagged)]
enum Limit {
    Capped(String),
    Unlimited(()),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Change {
    operation_id: Uuid,
    expected_revision: String,
    limit_micro_usd: Limit,
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
    let Some(cmd) = crate::setup::credential::<Change>(req).await else {
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
    let result = sqlx::query_scalar::<_, String>(
        "SELECT relay_hosted.change_purchase_limit($1,$2,$3,$4,$5,$6)",
    )
    .bind(&s.origin)
    .bind(admin.tag(b"session-v1", &token).as_ref())
    .bind(id)
    .bind(cmd.operation_id)
    .bind(revision)
    .bind(limit)
    .fetch_one(&s.pool)
    .await;
    match result.as_deref() {
        Ok("applied") => http::success(json!({"applied":true,"operation_id":cmd.operation_id})),
        Ok("denied") => denied(),
        Ok("missing") => http::not_found().await,
        Ok("invalid") => invalid(),
        Ok("fresh_required") => http::error(
            StatusCode::FORBIDDEN,
            "fresh_passkey_required",
            "Verify the owner passkey again.",
        ),
        Ok("conflict") => http::error(
            StatusCode::CONFLICT,
            "purchase_limit_changed",
            "Review the current purchase limit.",
        ),
        _ => unavailable(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unlimited_must_be_explicit_and_usd_is_exact() {
        let mut body = json!({"operation_id":Uuid::new_v4(),"expected_revision":"0"});
        assert!(serde_json::from_value::<Change>(body.clone()).is_err());
        for value in [json!(null), json!("0"), json!("9223372036854775807")] {
            body["limit_micro_usd"] = value;
            assert!(serde_json::from_value::<Change>(body.clone()).is_ok());
        }
        body["limit_micro_usd"] = json!(0);
        assert!(serde_json::from_value::<Change>(body).is_err());
        assert_eq!(nonnegative("9223372036854775807"), Some(i64::MAX));
        for invalid in ["", "01", " 1", "+1", "-0", "-1", "9223372036854775808"] {
            assert_eq!(nonnegative(invalid), None);
        }
    }
}
