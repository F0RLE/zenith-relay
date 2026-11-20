use super::*;
use crate::accounting::Rate;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use subtle::ConstantTimeEq;
use uuid::Uuid;

mod review;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Prices {
    input: Rate,
    cached: Rate,
    output: Rate,
}
impl Prices {
    fn valid(&self) -> bool {
        [self.input, self.cached, self.output]
            .into_iter()
            .all(Rate::fits_text_catalog)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum OptionalMarkup {
    Percent(Rate),
    Unset(()),
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceMarkup {
    expected_revision: String,
    percent: OptionalMarkup,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Change {
    operation_id: Uuid,
    expected_revision: String,
    action: String,
    rates: Option<Prices>,
    purchase_rates: Option<Prices>,
    source_markup: Option<SourceMarkup>,
    sale_rule: Option<review::Rule>,
}

fn revision(raw: &str) -> Option<i64> {
    let n: i64 = raw.parse().ok()?;
    ((0..i64::MAX).contains(&n) && n.to_string() == raw).then_some(n)
}

pub(super) async fn read(State(s): State<App>, h: HeaderMap) -> Response {
    let (Some(admin), Some(token)) = (&s.auth, browser::cookie(&h, SESSION)) else {
        return denied();
    };
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return unavailable();
    }
    match sqlx::query_scalar::<_, Option<Value>>("SELECT relay_hosted.owner_text_rules($1,$2)")
        .bind(&s.origin)
        .bind(admin.tag(b"session-v1", &token).as_ref())
        .fetch_one(&s.pool)
        .await
    {
        Ok(Some(items)) => http::success(json!({"items":items})),
        Ok(None) => denied(),
        Err(_) => unavailable(),
    }
}
pub(super) async fn change(
    State(s): State<App>,
    axum::extract::Path(id): axum::extract::Path<String>,
    req: Request,
) -> Response {
    let invalid = || {
        http::error(
            StatusCode::BAD_REQUEST,
            "invalid_model_rule",
            "Invalid model rule.",
        )
    };
    let (Some(admin), Some(token)) = (&s.auth, browser::cookie(req.headers(), SESSION)) else {
        return denied();
    };
    if !browser::same_origin(req.headers(), &s.origin)
        || !browser::one(req.headers(), "x-relay-csrf")
            .and_then(browser::decode)
            .is_some_and(|v| bool::from(admin.tag(b"csrf-v1", &token).as_ref().ct_eq(v.as_ref())))
    {
        return forbidden();
    }
    let Ok(item) = id.parse::<Uuid>() else {
        return invalid();
    };
    let Some(cmd) = crate::setup::credential::<Change>(req).await else {
        return invalid();
    };
    let Some(rev) = revision(&cmd.expected_revision) else {
        return invalid();
    };
    if cmd.operation_id.is_nil()
        || !matches!(
            cmd.action.as_str(),
            "admit" | "save" | "review" | "publish" | "suspend"
        )
        || (cmd.action == "save")
            != (cmd.rates.is_some() || cmd.purchase_rates.is_some() || cmd.source_markup.is_some())
        || (cmd.action == "review") != cmd.sale_rule.is_some()
        || [&cmd.rates, &cmd.purchase_rates]
            .into_iter()
            .flatten()
            .any(|p| !p.valid())
        || cmd.source_markup.as_ref().is_some_and(|source| {
            revision(&source.expected_revision).is_none()
                || matches!(source.percent, OptionalMarkup::Percent(p) if !p.fits_text_catalog())
        })
        || cmd.sale_rule.as_ref().is_some_and(|r| !r.valid())
    {
        return invalid();
    }
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return unavailable();
    }
    let tag = admin.tag(b"session-v1", &token);
    let payload = if let Some(rule) = cmd.sale_rule {
        let result = sqlx::query_scalar::<_, Option<Value>>(
            "SELECT relay_hosted.owner_text_price_inputs($1,$2,$3,$4)",
        )
        .bind(&s.origin)
        .bind(tag.as_ref())
        .bind(item)
        .bind(rule.inherits())
        .fetch_one(&s.pool)
        .await;
        let inputs = match result {
            Ok(Some(inputs)) => inputs,
            Ok(None) => return denied(),
            Err(_) => return unavailable(),
        };
        let Some(prepared) = review::prepare(item, rule, inputs) else {
            return unavailable();
        };
        prepared
    } else if cmd.purchase_rates.is_none() && cmd.source_markup.is_none() {
        cmd.rates.map_or_else(|| json!({}), |sale| json!(sale))
    } else {
        let mut payload = serde_json::Map::new();
        if let Some(sale) = cmd.rates {
            payload.insert("sale".into(), json!(sale));
        }
        if let Some(cost) = cmd.purchase_rates {
            payload.insert("purchase".into(), json!(cost));
        }
        if let Some(markup) = cmd.source_markup {
            payload.insert("source_markup".into(), json!(markup));
        }
        Value::Object(payload)
    };
    let result = sqlx::query_scalar::<_, String>(
        "SELECT relay_hosted.change_text_rule($1,$2,$3,$4,$5,$6,$7)",
    )
    .bind(&s.origin)
    .bind(tag.as_ref())
    .bind(item)
    .bind(cmd.operation_id)
    .bind(rev)
    .bind(cmd.action)
    .bind(payload)
    .fetch_one(&s.pool)
    .await;
    match result.as_deref() {
        Ok("applied") => http::success(json!({"operation_id":cmd.operation_id,"applied":true})),
        Ok("missing") => http::not_found().await,
        Ok("invalid") => invalid(),
        Ok("denied") => denied(),
        Ok("fresh_required") => http::error(
            StatusCode::FORBIDDEN,
            "fresh_passkey_required",
            "Verify the owner passkey again.",
        ),
        Ok("notice_required") => http::error(
            StatusCode::CONFLICT,
            "price_notice_required",
            "A price increase needs advance notice.",
        ),
        Ok("purchase_required") => http::error(
            StatusCode::CONFLICT,
            "purchase_prices_required",
            "Set purchase prices before publishing.",
        ),
        Ok("review_stale") => http::error(
            StatusCode::CONFLICT,
            "price_review_stale",
            "Review prices again after the purchase inputs changed.",
        ),
        Ok("no_proposal") => http::error(
            StatusCode::CONFLICT,
            "no_price_proposal",
            "No admitted source can propose a selling price under that rule.",
        ),
        Ok("unrepresentable") => http::error(
            StatusCode::CONFLICT,
            "unrepresentable_markup",
            "The calculated price exceeds the supported range or precision.",
        ),
        Ok("conflict" | "inactive") => http::error(
            StatusCode::CONFLICT,
            "rule_changed",
            "Review the current model rule.",
        ),
        _ => unavailable(),
    }
}
