//! Private seller review and valuation. Neither operation publishes a tariff or posts money.
mod decimal;
pub use decimal::Rate;
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Component {
    Input,
    Output,
    CacheRead,
    CacheWrite,
    CacheWrite5m,
    CacheWrite1h,
}

pub type Rates = BTreeMap<Component, Rate>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PricingError {
    InvalidComponents,
    InvalidSource,
    NoProposal,
    MissingSalePrice,
    UnrepresentableRate,
    InvalidUsage,
    ChargeOverflow,
}

/// The catalog owns this identity for one model, operation, context band and service tier.
pub struct Offer {
    id: Uuid,
    required: BTreeSet<Component>,
}

pub enum SaleRule {
    ApiDefault,
    ModelMarkup(Rate),
    Fixed(Rates),
}

/// Only approved inventory is an input. Runtime health and Desktop price estimates are not.
pub struct ApprovedPurchase {
    pub variant: Uuid,
    pub connection: Uuid,
    pub purchase_revision: i64,
    pub rates: Rates,
    pub default_markup: Option<(i64, Rate)>,
}

#[derive(Debug, Eq, PartialEq)]
pub struct SalePreview {
    pub selling: Rates,
    /// Connection -> (purchase revision, default revision actually used).
    pub inputs: BTreeMap<Uuid, (i64, Option<i64>)>,
    pub below_cost: BTreeMap<Uuid, BTreeSet<Component>>,
}

impl Offer {
    pub fn text(id: Uuid, required: BTreeSet<Component>) -> Result<Self, PricingError> {
        use Component::*;
        if id.is_nil()
            || !required.contains(&Input)
            || !required.contains(&Output)
            || (required.contains(&CacheWrite)
                && (required.contains(&CacheWrite5m) || required.contains(&CacheWrite1h)))
        {
            return Err(PricingError::InvalidComponents);
        }
        Ok(Self { id, required })
    }

    /// Highest complete marked-up proposal per component, or the seller's exact fixed price.
    pub fn preview(
        &self,
        rule: &SaleRule,
        approved: &[ApprovedPurchase],
    ) -> Result<SalePreview, PricingError> {
        use PricingError::*;
        let mut selling = match rule {
            SaleRule::Fixed(rates) if rates.keys().eq(&self.required) => rates.clone(),
            SaleRule::Fixed(_) => return Err(MissingSalePrice),
            _ => Rates::new(),
        };
        let mut inputs = BTreeMap::new();
        let mut comparable = BTreeMap::new();
        let mut seen = BTreeSet::new();
        for source in approved.iter().filter(|s| s.variant == self.id) {
            if source.connection.is_nil()
                || source.purchase_revision < 1
                || !seen.insert(source.connection)
            {
                return Err(InvalidSource);
            }
            if !source.rates.keys().eq(&self.required) {
                continue;
            }
            comparable.insert(source.connection, &source.rates);
            let (revision, percent) = match rule {
                SaleRule::Fixed(_) => continue,
                SaleRule::ModelMarkup(p) => (None, *p),
                SaleRule::ApiDefault => match source.default_markup {
                    Some((r, p)) if r > 0 => (Some(r), p),
                    Some(_) => return Err(InvalidSource),
                    None => continue,
                },
            };
            inputs.insert(source.connection, (source.purchase_revision, revision));
            for (&component, &cost) in &source.rates {
                let rate = cost.with_markup(percent).ok_or(UnrepresentableRate)?;
                selling
                    .entry(component)
                    .and_modify(|best| *best = (*best).max(rate))
                    .or_insert(rate);
            }
        }
        if selling.is_empty() {
            return Err(NoProposal);
        }
        let below_cost = comparable
            .into_iter()
            .filter_map(|(source, rates)| {
                let below: BTreeSet<_> = rates
                    .iter()
                    .filter_map(|(c, rate)| (selling[c] < *rate).then_some(*c))
                    .collect();
                (!below.is_empty()).then_some((source, below))
            })
            .collect();
        Ok(SalePreview {
            selling,
            inputs,
            below_cost,
        })
    }

    /// Price complete, disjoint counters. USD/million tokens × tokens equals microUSD.
    /// Input is ordinary input only; a provider's inclusive total must be normalized upstream.
    pub fn value_micro_usd(
        &self,
        rates: &Rates,
        counts: &BTreeMap<Component, i64>,
    ) -> Result<i64, PricingError> {
        use PricingError::*;
        if !rates.keys().eq(&self.required) {
            return Err(MissingSalePrice);
        }
        if !counts.keys().eq(&self.required) {
            return Err(InvalidUsage);
        }
        let mut sum = 0u128;
        for (c, rate) in rates {
            let count = u128::try_from(counts[c]).map_err(|_| InvalidUsage)?;
            let term = rate.0.checked_mul(count).ok_or(ChargeOverflow)?;
            sum = sum.checked_add(term).ok_or(ChargeOverflow)?;
        }
        i64::try_from(sum.div_ceil(decimal::SCALE)).map_err(|_| ChargeOverflow)
    }
}
