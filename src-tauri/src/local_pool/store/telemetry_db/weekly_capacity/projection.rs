//! Common reference workload for comparable subscription cards, not a balance.
use super::*;
use zenith_relay_core::protocol::{QuotaWindowUsage, WeeklyValueCalibration};

#[cfg(test)]
mod tests;

pub(in crate::local_pool::store::telemetry_db) const PROJECTION_SQL: &str =
    "SELECT a.payload_json, (SELECT h.payload_json FROM weekly_capacity_history h
     WHERE h.account_id = a.account_id AND h.observed_at_ms <= ?2
     ORDER BY h.observed_at_ms DESC LIMIT 1)
     FROM weekly_capacity_anchors a WHERE a.account_id = ?1";

impl TelemetryDb {
    pub(crate) fn weekly_value_projections(
        &self,
        observations: &[CapacityObservation<'_>],
    ) -> Result<BTreeMap<String, QuotaWindowUsage>> {
        if observations.is_empty() {
            return Ok(BTreeMap::new());
        }
        // At most one indexed history lookup per known ledger. Never scan request
        // telemetry or the full capacity history during a UI snapshot.
        let connection = self.lock_report_connection()?;
        let mut statement = connection.prepare(PROJECTION_SQL).map_err(db_error)?;
        let mut samples = BTreeMap::new();
        for observation in observations.iter().take(MAX_LEDGERS) {
            if !valid_window(observation) {
                continue;
            }
            let row = statement
                .query_row(
                    params![observation.ledger, sql_u64(observation.now_ms)],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
                )
                .optional()
                .map_err(db_error)?;
            let Some((anchor, Some(measurement))) = row else {
                continue;
            };
            let anchor: Anchor = serde_json::from_str(&anchor).map_err(json_error)?;
            let measurement: Measurement =
                serde_json::from_str(&measurement).map_err(json_error)?;
            if compatible_sample(&anchor, &measurement, observation) {
                samples.insert(observation.ledger, (observation, measurement));
            }
        }
        drop(statement);
        drop(connection);
        Ok(project(observations, &samples))
    }
}

fn valid_window(value: &CapacityObservation<'_>) -> bool {
    let window = value.window;
    !value.ledger.is_empty()
        && !value.account_ids.is_empty()
        && !value.plan.trim().is_empty()
        && !value.plan.trim().eq_ignore_ascii_case("unknown")
        && window
            .window_minutes
            .is_some_and(|minutes| (6 * 1440..=8 * 1440).contains(&minutes))
        && window.available_basis_points.is_some_and(|bp| bp <= 10_000)
        && window
            .window_start_ms
            .is_some_and(|start| start <= window.observed_at_ms)
        && window.reset_at_ms.is_some_and(|reset| reset > value.now_ms)
        && window.observed_at_ms <= value.now_ms
        && value.now_ms - window.observed_at_ms < MAX_ANCHOR_AGE
}

fn compatible_sample(
    anchor: &Anchor,
    sample: &Measurement,
    current: &CapacityObservation<'_>,
) -> bool {
    let next = Anchor {
        account_ids: current.account_ids.to_vec(),
        plan: current.plan.trim().to_ascii_lowercase(),
        pricing_revision: fingerprint(current.pricing_revision),
        members: fingerprint(&current.account_ids.join("\0")),
        window: current.window.clone(),
        from_ms: current.window.observed_at_ms,
        remaining: current.window.available_basis_points.unwrap_or_default(),
    };
    let unchanged = anchor.plan == next.plan
        && anchor.pricing_revision == next.pricing_revision
        && anchor.members == next.members
        && anchor.window == next.window;
    // Current inventory fences persisted samples even before the background
    // collector records a reset, plan change, unlink, or corrected observation.
    if current.window.observed_at_ms < anchor.window.observed_at_ms
        || (!unchanged && discontinuity(anchor, &next).is_some())
    {
        return false;
    }
    sample.event.is_none()
        && sample.ledger == current.ledger
        && sample.account_ids == current.account_ids
        && sample.plan == next.plan
        && sample.pricing_revision == next.pricing_revision
        && sample.workload.is_some()
        && (MIN_CONSUMED..=10_000).contains(&sample.consumed)
        && sample
            .micro_usd
            .is_some_and(|cost| (1..=9_007_199_254_740_991).contains(&cost))
        && sample.from_ms < sample.at_ms
        && sample.from_ms >= current.now_ms.saturating_sub(MAX_ANCHOR_AGE)
        && sample.from_ms
            >= current
                .window
                .window_start_ms
                .unwrap_or_default()
                .saturating_sub(120_000)
        && sample.at_ms <= anchor.window.observed_at_ms
        && sample.at_ms <= current.now_ms
        && current.now_ms - sample.at_ms < MAX_ANCHOR_AGE
        && sample.reset_at_ms > current.now_ms
        && sample
            .reset_at_ms
            .abs_diff(current.window.reset_at_ms.unwrap_or_default())
            <= 120_000
}

fn cohort(value: &CapacityObservation<'_>) -> (String, u32, String) {
    (
        value.plan.trim().to_ascii_lowercase(),
        value.window.window_minutes.unwrap_or_default(),
        fingerprint(value.pricing_revision),
    )
}

fn project(
    observations: &[CapacityObservation<'_>],
    samples: &BTreeMap<&str, (&CapacityObservation<'_>, Measurement)>,
) -> BTreeMap<String, QuotaWindowUsage> {
    let mut cohorts = BTreeMap::<_, BTreeMap<&str, Vec<&Measurement>>>::new();
    for (observation, sample) in samples.values() {
        if let Some(workload) = &sample.workload {
            cohorts
                .entry(cohort(observation))
                .or_default()
                .entry(workload.key.as_str())
                .or_default()
                .push(sample);
        }
    }
    let mut calibrations = BTreeMap::new();
    let now = observations.first().map_or(0, |value| value.now_ms);
    for (key, groups) in cohorts {
        let has_fresh = groups
            .values()
            .flatten()
            .any(|sample| sample.from_ms >= now.saturating_sub(DAY_MS));
        let mut selected = Vec::new();
        for values in groups.into_values() {
            let values = values
                .into_iter()
                .filter(|sample| !has_fresh || sample.from_ms >= now.saturating_sub(DAY_MS))
                .collect::<Vec<_>>();
            // Most represented exact workload; ties use the stable workload key,
            // never whichever account happened to refresh most recently.
            if values.len() > selected.len() {
                selected = values;
            }
        }
        let Some(point) = report::point(now, &selected) else {
            continue;
        };
        let Some(workload) = selected.first().and_then(|sample| sample.workload.as_ref()) else {
            continue;
        };
        calibrations.insert(
            key.clone(),
            WeeklyValueCalibration {
                profile_id: fingerprint(&format!(
                    "{}\0{}\0{}\0{}",
                    key.0, key.1, key.2, workload.key
                )),
                workload: workload.label.clone(),
                cache_band: workload.cache_band,
                output_band: workload.output_band,
                median_micro_usd: point.median_micro_usd,
                low_micro_usd: point.low_micro_usd,
                high_micro_usd: point.high_micro_usd,
                account_count: point.account_count,
                observed_at_ms: point.observed_at_ms,
                valid_until_ms: selected
                    .iter()
                    .map(|sample| {
                        sample
                            .reset_at_ms
                            .min(sample.from_ms.saturating_add(if has_fresh {
                                DAY_MS
                            } else {
                                MAX_ANCHOR_AGE
                            }))
                    })
                    .min()
                    .unwrap_or(point.valid_until_ms),
            },
        );
    }
    let mut result = BTreeMap::new();
    for observation in observations {
        if !valid_window(observation) {
            continue;
        }
        let Some(calibration) = calibrations.get(&cohort(observation)) else {
            continue;
        };
        let window = observation.window;
        let value = QuotaWindowUsage {
            ledger_key: observation.ledger.to_owned(),
            kind: window.kind,
            window_start_ms: window.window_start_ms.unwrap_or_default(),
            observed_at_ms: window.observed_at_ms,
            window_minutes: window.window_minutes.unwrap_or_default(),
            reset_at_ms: window.reset_at_ms.unwrap_or_default(),
            available_basis_points: window.available_basis_points.unwrap_or_default(),
            calibration: calibration.clone(),
        };
        for id in observation.account_ids {
            result.insert(id.clone(), value.clone());
        }
    }
    result
}
