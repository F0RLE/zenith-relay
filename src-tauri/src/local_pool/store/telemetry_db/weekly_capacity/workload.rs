use super::*;
use zenith_relay_core::{
    pricing::{PricingCatalog, PricingContext},
    ApiEquivalentUsage, CatalogPriceResolver, ObservedUsageSums,
};

#[derive(Clone, Deserialize, Serialize)]
pub(super) struct Workload {
    pub key: String,
    pub label: String,
    #[serde(default)]
    pub models: Vec<String>,
    pub cache_band: u8,
    pub output_band: u8,
}

impl Workload {
    pub(super) fn single_model(&self) -> Option<&str> {
        match self.models.as_slice() {
            [model] => (!model.trim().is_empty()).then_some(model.as_str()),
            [] => {
                // Older stored observations only have the joined profile label.
                // Accept it only when every profile names the same model.
                let mut names = self.label.split(" + ").map(|profile| {
                    profile
                        .split_once(" · ")
                        .map_or(profile, |(model, _)| model)
                });
                let model = names.next().filter(|model| !model.trim().is_empty())?;
                names.all(|name| name == model).then_some(model)
            }
            _ => None,
        }
    }
}

pub(super) fn measure(
    connection: &Connection,
    accounts: &[String],
    from_ms: u64,
    to_ms: u64,
    catalog: &PricingCatalog,
    pricing: &PricingContext,
) -> Result<Option<(u64, Workload)>> {
    let sql = format!(
        "SELECT COALESCE(resolved_model, requested_model, ''), {class}, {context},
         COALESCE(effective_reasoning_effort, ''), {columns}, COUNT(*)
         FROM usage_sends WHERE account_id = ?1
         AND created_at >= datetime(?2 / 1000, 'unixepoch') AND created_at <= datetime(?3 / 1000, 'unixepoch')
         GROUP BY 1, 2, 3, 4",
        class = zenith_relay_core::usage::USAGE_PRICE_CLASS_SQL,
        context = zenith_relay_core::usage::USAGE_CONTEXT_BAND_SQL,
        columns = zenith_relay_core::usage::API_EQUIVALENT_AGGREGATE_SQL,
    );
    let resolver = CatalogPriceResolver::new(catalog, pricing);
    let mut statement = connection.prepare(&sql).map_err(db_error)?;
    let mut profiles = BTreeMap::<String, u64>::new();
    let mut models = std::collections::BTreeSet::new();
    let (mut cost, mut input, mut cached, mut output) = (0_u64, 0_u64, 0_u64, 0_u64);
    for account in accounts {
        let rows = statement
            .query_map(params![account, sql_u64(from_ms), sql_u64(to_ms)], |row| {
                let model: String = row.get(0)?;
                let class: String = row.get(1)?;
                let context: String = row.get(2)?;
                let effort: String = row.get(3)?;
                let usage = ApiEquivalentUsage::from_observed_sums(
                    ObservedUsageSums::from_priced_aggregate(
                        |index| row.get(4 + index),
                        |index| row.get(4 + index),
                    )?,
                )
                .with_aggregate_rates(&class, &context);
                let rows: i64 = row.get(17)?;
                let complete = [12, 13, 15]
                    .into_iter()
                    .all(|index| row.get::<_, i64>(index).ok() == Some(rows));
                Ok((model, class, context, effort, usage, complete))
            })
            .map_err(db_error)?;
        for row in rows {
            let (model, class, context, effort, usage, complete) = row.map_err(db_error)?;
            let value = resolver.estimate("account", account, Some(&model), usage);
            if !complete
                || model.is_empty()
                || value.unpriced_tokens > 0
                || usage.cached_input_tokens > usage.input_tokens
            {
                return Ok(None);
            }
            let row_input = usage.input_tokens.unwrap_or_default();
            let row_output = usage.output_tokens.unwrap_or_default();
            let tokens = row_input.saturating_add(row_output);
            if tokens == 0 {
                continue;
            }
            cost = cost.saturating_add(value.micro_usd);
            input = input.saturating_add(row_input);
            cached = cached.saturating_add(usage.cached_input_tokens.unwrap_or_default());
            output = output.saturating_add(row_output);
            models.insert(model.clone());
            // Keep joint model/tier/context/reasoning proportions, not just a
            // model name. Coarse token-mix bands reduce false policy-change signals.
            let profile = profiles
                .entry(format!("{model} · {class} · {context} · {effort}"))
                .or_default();
            *profile = profile.saturating_add(tokens);
        }
    }
    let total = input.saturating_add(output);
    if cost == 0 || total == 0 || profiles.is_empty() || profiles.len() > 8 {
        return Ok(None);
    }
    let band = |part: u64, whole: u64, size: u64| -> u8 {
        if whole == 0 {
            return 0;
        }
        ((u128::from(part) * 100 / u128::from(whole)).min(99) as u64 / size * size) as u8
    };
    let cache_band = band(cached, input, 25);
    let output_band = band(output, total, 10);
    let mix = profiles
        .iter()
        .map(|(profile, tokens)| format!("{profile}:{}", band(*tokens, total, 10)))
        .collect::<Vec<_>>()
        .join("|");
    let key = fingerprint(&format!("{mix}|cache:{cache_band}|output:{output_band}"));
    let label = profiles.keys().cloned().collect::<Vec<_>>().join(" + ");
    Ok(Some((
        cost,
        Workload {
            key,
            label,
            models: models.into_iter().collect(),
            cache_band,
            output_band,
        },
    )))
}
