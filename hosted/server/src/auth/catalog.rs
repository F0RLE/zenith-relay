use super::*;
use crate::accounting::Rate;
use axum::extract::{Path, RawQuery};
use confirmation::{Approval, Context};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

pub(super) mod display_order;
pub(super) mod member_models;
mod review;
pub(super) mod rotation;

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
#[serde(rename_all = "snake_case")]
enum Action {
    Admit,
    Save,
    Review,
    Publish,
    Suspend,
}

impl Action {
    fn requires_proof(&self) -> bool {
        matches!(self, Self::Admit | Self::Publish | Self::Suspend)
    }

    fn as_str(&self) -> &'static str {
        match self {
            Self::Admit => "admit",
            Self::Save => "save",
            Self::Review => "review",
            Self::Publish => "publish",
            Self::Suspend => "suspend",
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestChange {
    operation_id: Uuid,
    expected_revision: String,
    action: Action,
    rates: Option<Prices>,
    purchase_rates: Option<Prices>,
    source_markup: Option<SourceMarkup>,
    sale_rule: Option<review::Rule>,
}

pub(super) struct Change {
    item: Uuid,
    operation_id: Uuid,
    revision: i64,
    action: Action,
    payload: Value,
}

impl Change {
    async fn execute(
        &self,
        s: &App,
        tag: &hmac::Tag,
        proof: Option<(&Context, &Approval)>,
    ) -> sqlx::Result<String> {
        let query = sqlx::query_scalar(
            "SELECT relay_hosted.change_text_rule($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)",
        )
        .bind(&s.origin)
        .bind(tag.as_ref())
        .bind(self.item)
        .bind(self.operation_id)
        .bind(self.revision)
        .bind(self.action.as_str())
        .bind(&self.payload);
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

pub(super) async fn activity(
    State(s): State<App>,
    h: HeaderMap,
    RawQuery(query): RawQuery,
) -> Response {
    let (Some(admin), Some(token)) = (&s.auth, browser::cookie(&h, SESSION)) else {
        return denied();
    };
    if query.is_some() {
        return http::error(
            StatusCode::BAD_REQUEST,
            "invalid_filter",
            "Pool activity does not accept filters.",
        );
    }
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return unavailable();
    }
    match sqlx::query_scalar::<_, Option<Value>>("SELECT relay_hosted.owner_pool_activity($1,$2)")
        .bind(&s.origin)
        .bind(admin.tag(b"session-v1", &token).as_ref())
        .fetch_one(&s.pool)
        .await
    {
        Ok(Some(view)) => http::success(view),
        Ok(None) => denied(),
        Err(_) => unavailable(),
    }
}

pub(super) async fn change(State(s): State<App>, Path(id): Path<String>, req: Request) -> Response {
    submit(s, id, req, false).await
}

pub(super) async fn options(
    State(s): State<App>,
    Path(id): Path<String>,
    req: Request,
) -> Response {
    submit(s, id, req, true).await
}

fn invalid() -> Response {
    http::error(
        StatusCode::BAD_REQUEST,
        "invalid_model_rule",
        "Invalid model rule.",
    )
}

async fn submit(s: App, id: String, req: Request, prepare: bool) -> Response {
    let tag = match owner_mutation(&s, req.headers()) {
        Ok(tag) => tag,
        Err(e) => return e.response(),
    };
    let Ok(item) = id.parse::<Uuid>() else {
        return invalid();
    };
    let Some(cmd) = crate::setup::credential::<RequestChange>(req).await else {
        return invalid();
    };
    let Some(rev) = revision(&cmd.expected_revision) else {
        return invalid();
    };
    if cmd.operation_id.is_nil()
        || (prepare && !cmd.action.requires_proof())
        || matches!(cmd.action, Action::Save)
            != (cmd.rates.is_some() || cmd.purchase_rates.is_some() || cmd.source_markup.is_some())
        || matches!(cmd.action, Action::Review) != cmd.sale_rule.is_some()
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
    let change = Change {
        item,
        operation_id: cmd.operation_id,
        revision: rev,
        action: cmd.action,
        payload,
    };
    let result = change.execute(&s, &tag, None).await;
    if prepare && matches!(result.as_deref(), Ok("fresh_required")) {
        let admin = s.auth.as_ref().expect("authenticated");
        let command = supply_confirmation::Command::Rule(change);
        return confirmation::start(&s, &admin.supply_changes, tag, command).await;
    }
    respond(result, change.operation_id).await
}

async fn respond(result: sqlx::Result<String>, operation_id: Uuid) -> Response {
    match result.as_deref() {
        Ok("applied") => http::success(json!({"operation_id":operation_id,"applied":true})),
        Ok("missing") => http::not_found().await,
        Ok("invalid") => invalid(),
        Ok("denied") => denied(),
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
        Ok(code) => confirmation::error(code).unwrap_or_else(unavailable),
        Err(_) => unavailable(),
    }
}
