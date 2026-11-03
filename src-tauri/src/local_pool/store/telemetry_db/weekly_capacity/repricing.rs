//! Revalue retained evidence without treating catalog edits as quota resets.
use super::*;
use zenith_relay_core::pricing::{PricingCatalog, PricingContext};

#[cfg(test)]
mod tests;

fn latest_sample(
    connection: &Connection,
    ledger: &str,
    previous: &Anchor,
    now_ms: u64,
) -> Result<Option<Measurement>> {
    if let Some(calibration) = &previous.calibration {
        return Ok(calibration.sample.clone());
    }
    // Legacy versions ended every catalog change with an empty pricing event.
    // Skip only those events, never a reset, plan/membership change or correction.
    // A bounded lookup repairs existing installations without rewriting history.
    let mut statement = connection
        .prepare(
            "SELECT payload_json FROM weekly_capacity_history
         WHERE account_id = ?1 AND observed_at_ms >= ?2 AND observed_at_ms <= ?3
         ORDER BY observed_at_ms DESC LIMIT 64",
        )
        .map_err(db_error)?;
    let rows = statement
        .query_map(
            params![
                ledger,
                sql_u64(now_ms.saturating_sub(MAX_ANCHOR_AGE)),
                sql_u64(previous.window.observed_at_ms),
            ],
            |row| row.get::<_, String>(0),
        )
        .map_err(db_error)?;
    for row in rows {
        let sample: Measurement =
            serde_json::from_str(&row.map_err(db_error)?).map_err(json_error)?;
        match sample.event {
            Some(CapacityEventKind::PricingChanged)
                if sample.ledger == ledger
                    && sample.account_ids == previous.account_ids
                    && sample.plan == previous.plan
                    && sample
                        .reset_at_ms
                        .abs_diff(previous.window.reset_at_ms.unwrap_or_default())
                        <= 120_000 =>
            {
                continue;
            }
            Some(_) => return Ok(None),
            None => return Ok(Some(sample)),
        }
    }
    Ok(None)
}

pub(super) fn refresh(
    connection: &Connection,
    ledger: &str,
    previous: &Anchor,
    next: &Anchor,
    now_ms: u64,
    catalog: &PricingCatalog,
    pricing: &PricingContext,
) -> Result<Calibration> {
    let Some(original) = latest_sample(connection, ledger, previous, now_ms)? else {
        return Ok(Calibration::default());
    };
    let mut sample = original.clone();
    // Ignore the old price revision only while validating the non-price fences;
    // no projection is published until the actual interval is priced again.
    sample.pricing_revision.clone_from(&next.pricing_revision);
    let invalid = Calibration {
        sample: Some(original),
        valid: false,
    };
    if !projection::compatible_measurement(&sample, next, ledger, now_ms) {
        return Ok(invalid);
    }
    // Raw sends are retained for 30 days; eligible samples span at most 72 hours.
    // History/account clears also clear the anchor and fence the caller's commit.
    let from_ms = (sample.from_ms / 1000 + 1) * 1000;
    let Some(priced) = workload::measure(
        connection,
        &sample.account_ids,
        from_ms,
        sample.at_ms,
        catalog,
        pricing,
    )?
    else {
        return Ok(invalid);
    };
    if sample
        .usage_fingerprint
        .as_ref()
        .is_some_and(|expected| expected != &priced.usage_fingerprint)
        || sample
            .workload
            .as_ref()
            .is_none_or(|expected| expected.key != priced.workload.key)
    {
        return Ok(invalid);
    }
    let capacity = u128::from(priced.cost) * 10_000 / u128::from(sample.consumed);
    if !(1..=9_007_199_254_740_991).contains(&capacity) {
        return Ok(invalid);
    }
    sample.micro_usd = Some(capacity as u64);
    sample.pricing_coverage_basis_points = priced.pricing_coverage_basis_points;
    sample.workload = Some(priced.workload);
    sample.usage_fingerprint = Some(priced.usage_fingerprint);
    // Keep the original quota timestamps: repricing never renews freshness.
    Ok(Calibration {
        sample: Some(sample),
        valid: true,
    })
}
