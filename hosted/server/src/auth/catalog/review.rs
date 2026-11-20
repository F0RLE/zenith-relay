use super::{Prices, Rate, Uuid, Value, json};
use crate::accounting::{ApprovedPurchase, Component, Offer, PricingError, SaleRule};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Rule {
    ApiDefault {},
    ModelMarkup { percent: Rate },
}

impl Rule {
    pub(super) fn inherits(&self) -> bool {
        matches!(self, Self::ApiDefault {})
    }

    pub(super) fn valid(&self) -> bool {
        match self {
            Self::ApiDefault {} => true,
            Self::ModelMarkup { percent } => percent.fits_text_catalog(),
        }
    }
}

#[derive(Deserialize)]
struct SupplierInput {
    source: Uuid,
    purchase_revision: Option<i64>,
    rates: Option<Prices>,
    default_revision: Option<i64>,
    default_percent: Option<Rate>,
}

/// Compute the review from a private DB snapshot. Commit rechecks that exact
/// snapshot under the publication lock, never accepting prices from the browser.
pub(super) fn prepare(item: Uuid, rule: Rule, snapshot: Value) -> Option<Value> {
    use Component::*;
    let rows: Vec<SupplierInput> = serde_json::from_value(snapshot.clone()).ok()?;
    if rows.len() > 128 {
        return None;
    }
    let approved = rows
        .into_iter()
        .filter_map(|row| {
            let p = row.rates?;
            Some(ApprovedPurchase {
                variant: item,
                connection: row.source,
                purchase_revision: row.purchase_revision?,
                rates: [(Input, p.input), (CacheRead, p.cached), (Output, p.output)].into(),
                default_markup: row.default_revision.zip(row.default_percent),
            })
        })
        .collect::<Vec<_>>();
    let offer = Offer::text(item, [Input, CacheRead, Output].into()).ok()?;
    let mode = match rule {
        Rule::ApiDefault {} => SaleRule::ApiDefault,
        Rule::ModelMarkup { percent } => SaleRule::ModelMarkup(percent),
    };
    let problem = match offer.preview(&mode, &approved) {
        Ok(preview) => {
            let sale = Prices {
                input: preview.selling[&Input],
                cached: preview.selling[&CacheRead],
                output: preview.selling[&Output],
            };
            if sale.valid() {
                return Some(json!({"rule":rule,"rates":sale,"inputs":snapshot}));
            }
            "unrepresentable"
        }
        Err(PricingError::NoProposal) => "no_proposal",
        Err(PricingError::UnrepresentableRate) => "unrepresentable",
        Err(_) => return None,
    };
    Some(json!({"rule":rule,"problem":problem,"inputs":snapshot}))
}
