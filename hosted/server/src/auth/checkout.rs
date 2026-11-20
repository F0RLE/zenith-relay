use super::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashSet;
use uuid::Uuid;

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum FeePayer {
    Merchant,
    Customer,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Selection {
    binding_id: Uuid,
    code: String,
    fee_payer: FeePayer,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Draft {
    connections: Vec<Uuid>,
    methods: Vec<Selection>,
    minimum_credit_minor: u32,
    quick_amounts: Vec<u32>,
    custom_amount: bool,
    open_limit: u8,
}

impl Draft {
    fn valid(&self) -> bool {
        let mut connections = HashSet::new();
        let mut methods = HashSet::new();
        let mut amounts = HashSet::new();
        (1..=1_000_000).contains(&self.minimum_credit_minor)
            && (1..=10).contains(&self.open_limit)
            && self.quick_amounts.len() <= 8
            && (self.custom_amount || !self.quick_amounts.is_empty())
            && self
                .quick_amounts
                .iter()
                .all(|&n| (self.minimum_credit_minor..=1_000_000).contains(&n) && amounts.insert(n))
            && self.connections.len() <= 8
            && self
                .connections
                .iter()
                .all(|id| !id.is_nil() && connections.insert(*id))
            && self.methods.len() <= 16
            && self.methods.iter().all(|m| {
                connections.contains(&m.binding_id)
                    && valid_code(&m.code)
                    && methods.insert((m.binding_id, m.code.as_str()))
            })
    }
}

fn valid_code(s: &str) -> bool {
    (1..=64).contains(&s.len())
        && s.bytes().enumerate().all(|(i, b)| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || i > 0 && matches!(b, b'-' | b'_')
        })
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Action {
    Save { draft: Draft },
    Check {},
    Publish {},
    Disable { method_id: Uuid },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Change {
    operation_id: Uuid,
    expected_revision: String,
    command: Action,
}

fn invalid() -> Response {
    http::error(
        StatusCode::BAD_REQUEST,
        "invalid_checkout_configuration",
        "Invalid checkout configuration.",
    )
}

pub(super) async fn read(State(s): State<App>, headers: HeaderMap) -> Response {
    let (Some(admin), Some(token)) = (&s.auth, browser::cookie(&headers, SESSION)) else {
        return denied();
    };
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return unavailable();
    }
    match sqlx::query_scalar::<_, Option<Value>>(
        "SELECT relay_hosted.owner_checkout_builder($1,$2)",
    )
    .bind(&s.origin)
    .bind(admin.tag(b"session-v1", &token).as_ref())
    .fetch_one(&s.pool)
    .await
    {
        Ok(Some(data)) => http::success(data),
        Ok(None) => denied(),
        Err(_) => unavailable(),
    }
}

pub(super) async fn change(State(s): State<App>, req: Request) -> Response {
    let tag = match owner_mutation(&s, req.headers()) {
        Ok(tag) => tag,
        Err(e) => return e.response(),
    };
    let Some(cmd) = crate::setup::credential::<Change>(req).await else {
        return invalid();
    };
    let Ok(revision) = cmd.expected_revision.parse::<i64>() else {
        return invalid();
    };
    if revision < 1 || revision.to_string() != cmd.expected_revision || cmd.operation_id.is_nil() {
        return invalid();
    }
    let (action, draft, target) = match &cmd.command {
        Action::Save { draft } if draft.valid() => ("save", Some(sqlx::types::Json(draft)), None),
        Action::Check {} => ("check", None, None),
        Action::Publish {} => ("publish", None, None),
        Action::Disable { method_id } if !method_id.is_nil() => ("disable", None, Some(*method_id)),
        _ => return invalid(),
    };
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return unavailable();
    }
    let result = sqlx::query_scalar::<_, String>(
        "SELECT relay_hosted.change_checkout_builder($1,$2,$3,$4,$5,$6,$7)",
    )
    .bind(&s.origin)
    .bind(tag.as_ref())
    .bind(cmd.operation_id)
    .bind(revision)
    .bind(action)
    .bind(draft)
    .bind(target)
    .fetch_one(&s.pool)
    .await;
    match result.as_deref() {
        Ok("applied") => http::success(json!({"applied":true,"operation_id":cmd.operation_id})),
        Ok("denied") => denied(),
        Ok("invalid") => invalid(),
        Ok("fresh_required") => http::error(
            StatusCode::FORBIDDEN,
            "fresh_passkey_required",
            "Verify the owner passkey again.",
        ),
        Ok("conflict") => http::error(
            StatusCode::CONFLICT,
            "revision_conflict",
            "The checkout configuration changed.",
        ),
        Ok("check_required") => http::error(
            StatusCode::CONFLICT,
            "checkout_check_required",
            "Check the saved draft again.",
        ),
        Ok("unavailable") => http::error(
            StatusCode::FORBIDDEN,
            "checkout_closed",
            "Checkout configuration is limited to local qualification.",
        ),
        _ => unavailable(),
    }
}
