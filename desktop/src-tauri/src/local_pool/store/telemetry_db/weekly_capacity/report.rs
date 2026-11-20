use super::*;

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WeeklyCapacityQuery {
    pub from_ms: Option<u64>,
    pub to_ms: Option<u64>,
    pub bucket_ms: Option<u64>,
    pub account_id: Option<String>,
}

impl WeeklyCapacityQuery {
    fn includes_account(&self, ids: &[String]) -> bool {
        self.account_id.as_ref().is_none_or(|id| ids.contains(id))
    }
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WeeklyCapacityReport {
    pub generated_at_ms: u64,
    pub summaries: Vec<WeeklyCapacitySummary>,
    pub models: Vec<WeeklyCapacityModel>,
    pub groups: Vec<WeeklyCapacityGroup>,
    pub events: Vec<WeeklyCapacityEvent>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WeeklyCapacityPoint {
    pub at_ms: u64,
    pub observed_at_ms: u64,
    pub sample_start_ms: u64,
    pub valid_until_ms: u64,
    pub median_micro_usd: u64,
    pub low_micro_usd: u64,
    pub high_micro_usd: u64,
    pub account_count: usize,
    pub consumed_basis_points: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WeeklyCapacityGroup {
    pub id: String,
    pub plan: String,
    pub workload: String,
    pub cache_band: u8,
    pub output_band: u8,
    pub pricing_revision: String,
    pub points: Vec<WeeklyCapacityPoint>,
    pub current: Option<WeeklyCapacityPoint>,
    pub stale: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WeeklyCapacitySummary {
    pub plan: String,
    pub points: Vec<WeeklyCapacityPoint>,
    pub current: Option<WeeklyCapacityPoint>,
    pub stale: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WeeklyCapacityModel {
    pub model: String,
    #[serde(flatten)]
    pub summary: WeeklyCapacitySummary,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WeeklyCapacityEvent {
    pub at_ms: u64,
    pub plan: String,
    pub kind: CapacityEventKind,
    pub count: usize,
}

impl TelemetryDb {
    pub(crate) fn weekly_capacity_report(
        &self,
        query: WeeklyCapacityQuery,
        now_ms: u64,
        observations: &[CapacityObservation<'_>],
    ) -> Result<WeeklyCapacityReport> {
        let from = query
            .from_ms
            .unwrap_or(now_ms.saturating_sub(30 * DAY_MS))
            .clamp(now_ms.saturating_sub(30 * DAY_MS), now_ms);
        let to = query.to_ms.unwrap_or(now_ms).clamp(from, now_ms);
        let bucket = if query.bucket_ms == Some(DAY_MS) {
            DAY_MS
        } else {
            HOUR_MS
        };
        let (rows, current) = {
            // Persisted capacity shares the WAL reader; usage-revision and
            // in-memory totals synchronization belongs to usage reports.
            let connection = self.lock_snapshot_connection()?;
            let mut statement = connection.prepare(
                "SELECT payload_json FROM weekly_capacity_history WHERE observed_at_ms >= ?1 AND observed_at_ms <= ?2
                 ORDER BY observed_at_ms DESC LIMIT ?3",
            ).map_err(db_error)?;
            let rows = statement
                .query_map(
                    params![sql_u64(from), sql_u64(to), MAX_HISTORY as i64],
                    |row| row.get::<_, String>(0),
                )
                .map_err(db_error)?;
            let mut values = Vec::new();
            for row in rows {
                let value: Measurement =
                    serde_json::from_str(&row.map_err(db_error)?).map_err(json_error)?;
                if query.includes_account(&value.account_ids) {
                    values.push(value);
                }
            }
            let current = if to == now_ms && !observations.is_empty() {
                let scoped = observations
                    .iter()
                    .filter(|obs| query.includes_account(obs.account_ids));
                projection::calibrated_samples(&connection, scoped)?
                    .into_values()
                    .map(|(_, sample)| sample)
                    .collect()
            } else {
                Vec::new()
            };
            (values, current)
        };
        Ok(build_report(&rows, &current, from, to, bucket, now_ms))
    }
}

pub(super) fn point(at_ms: u64, values: &[&Measurement]) -> Option<WeeklyCapacityPoint> {
    if values.is_empty() {
        return None;
    }
    let mut costs = values
        .iter()
        .filter_map(|value| value.micro_usd)
        .collect::<Vec<_>>();
    if costs.is_empty() {
        return None;
    }
    costs.sort_unstable();
    let middle = costs.len() / 2;
    let median = if costs.len() % 2 == 0 {
        ((u128::from(costs[middle - 1]) + u128::from(costs[middle])) / 2) as u64
    } else {
        costs[middle]
    };
    Some(WeeklyCapacityPoint {
        at_ms,
        observed_at_ms: values
            .iter()
            .map(|value| value.at_ms)
            .max()
            .unwrap_or(at_ms),
        sample_start_ms: values
            .iter()
            .map(|value| value.from_ms)
            .min()
            .unwrap_or(at_ms),
        valid_until_ms: values
            .iter()
            .map(|value| {
                value
                    .reset_at_ms
                    .min(value.from_ms.saturating_add(MAX_ANCHOR_AGE))
            })
            .min()
            .unwrap_or(at_ms),
        median_micro_usd: median,
        low_micro_usd: costs[(costs.len() - 1) / 10],
        high_micro_usd: costs[((costs.len() - 1) * 9).div_ceil(10)],
        account_count: costs.len(),
        consumed_basis_points: values.iter().map(|value| u64::from(value.consumed)).sum(),
    })
}

#[derive(Default)]
struct SeriesSamples<'a> {
    history: Vec<&'a Measurement>,
    current: Vec<&'a Measurement>,
}

impl<'a> SeriesSamples<'a> {
    fn add(&mut self, value: &'a Measurement, current: bool) {
        if current {
            self.current.push(value);
        } else {
            self.history.push(value);
        }
    }
}

fn build_report(
    rows: &[Measurement],
    calibrated: &[Measurement],
    from: u64,
    to: u64,
    bucket: u64,
    now: u64,
) -> WeeklyCapacityReport {
    let mut groups = BTreeMap::<_, SeriesSamples<'_>>::new();
    let mut plans = BTreeMap::<_, SeriesSamples<'_>>::new();
    let mut models = BTreeMap::<_, SeriesSamples<'_>>::new();
    let mut events =
        BTreeMap::<(u64, String, CapacityEventKind), std::collections::BTreeSet<&str>>::new();
    for (value, current) in rows
        .iter()
        .map(|v| (v, false))
        .chain(calibrated.iter().map(|v| (v, true)))
    {
        if let Some(kind) = value.event {
            if !current && (from..=to).contains(&value.at_ms) {
                let at = from + (value.at_ms - from) / bucket * bucket;
                events
                    .entry((at, value.plan.clone(), kind))
                    .or_default()
                    .insert(&value.ledger);
            }
            continue;
        }
        if value.pricing_coverage_basis_points != 10_000
            || value.micro_usd.is_none()
            || (!current && !(from..=to).contains(&value.at_ms))
        {
            continue;
        }
        let Some(workload) = &value.workload else {
            continue;
        };
        let plan = value.plan.as_str();
        plans.entry(plan).or_default().add(value, current);
        groups
            .entry((plan, workload.key.as_str(), value.pricing_revision.as_str()))
            .or_default()
            .add(value, current);
        if let Some(model) = workload.single_model() {
            models.entry((plan, model)).or_default().add(value, current);
        }
    }
    let mut result = Vec::new();
    for ((plan, key, revision), samples) in groups {
        let Some(workload) = samples
            .history
            .first()
            .or(samples.current.first())
            .and_then(|v| v.workload.as_ref())
        else {
            continue;
        };
        let summary = summarize_measurements(plan, &samples, from, bucket, now);
        result.push(WeeklyCapacityGroup {
            id: fingerprint(&format!("{plan}\0{key}\0{revision}")),
            plan: plan.to_owned(),
            workload: workload.label.clone(),
            cache_band: workload.cache_band,
            output_band: workload.output_band,
            pricing_revision: revision.to_owned(),
            points: summary.points,
            current: summary.current,
            stale: summary.stale,
        });
    }
    // Bound actual plan/model choices, not the number of price changes that fit in history.
    let mut summaries = plans
        .into_iter()
        .map(|(plan, samples)| summarize_measurements(plan, &samples, from, bucket, now))
        .collect::<Vec<_>>();
    summaries.sort_by_key(|s| report_order(s.current.as_ref(), &s.points));
    summaries.truncate(32);
    // Combine raw single-model observations, not already-aggregated workload
    // medians. Mixed-model intervals remain in the all-model plan summary.
    let mut models = models
        .into_iter()
        .map(|((plan, model), samples)| WeeklyCapacityModel {
            model: model.to_owned(),
            summary: summarize_measurements(plan, &samples, from, bucket, now),
        })
        .collect::<Vec<_>>();
    models.sort_by_key(|m| report_order(m.summary.current.as_ref(), &m.summary.points));
    models.truncate(32);
    result.sort_by_key(|g| report_order(g.current.as_ref(), &g.points));
    result.truncate(32);
    WeeklyCapacityReport {
        generated_at_ms: now,
        summaries,
        models,
        groups: result,
        events: events
            .into_iter()
            .map(|((at_ms, plan, kind), ledgers)| WeeklyCapacityEvent {
                at_ms,
                plan,
                kind,
                count: ledgers.len(),
            })
            .collect(),
    }
}

fn report_order(
    current: Option<&WeeklyCapacityPoint>,
    history: &[WeeklyCapacityPoint],
) -> std::cmp::Reverse<(bool, u64)> {
    std::cmp::Reverse((
        current.is_some(),
        current
            .into_iter()
            .chain(history.last())
            .map(|point| point.observed_at_ms)
            .max()
            .unwrap_or(0),
    ))
}

fn summarize_measurements(
    plan: &str,
    samples: &SeriesSamples<'_>,
    from: u64,
    bucket: u64,
    now: u64,
) -> WeeklyCapacitySummary {
    let mut buckets = BTreeMap::<u64, Vec<&Measurement>>::new();
    for &value in &samples.history {
        let at = from + (value.at_ms - from) / bucket * bucket;
        buckets.entry(at).or_default().push(value);
    }
    let mut current: Vec<_> = samples
        .current
        .iter()
        .copied()
        .filter(|v| {
            v.at_ms <= now && v.reset_at_ms > now && v.from_ms.saturating_add(MAX_ANCHOR_AGE) > now
        })
        .collect();
    let fresh = |v: &&Measurement| v.from_ms.saturating_add(DAY_MS) > now;
    let stale = !current.iter().any(fresh);
    if !stale {
        current.retain(fresh);
    }
    WeeklyCapacitySummary {
        plan: plan.to_owned(),
        points: buckets
            .into_iter()
            .filter_map(|(at, values)| comparable_point(at, &values))
            .collect(),
        current: comparable_point(now, &current),
        stale,
    }
}

/// A day/hour retains its last observed price revision, never a median of incompatible prices.
fn comparable_point(at: u64, values: &[&Measurement]) -> Option<WeeklyCapacityPoint> {
    let latest = values
        .iter()
        .max_by_key(|v| (v.at_ms, v.pricing_revision.as_str()))?;
    let mut ledgers = HashMap::new();
    for &value in values
        .iter()
        .filter(|v| v.pricing_revision == latest.pricing_revision)
    {
        keep_latest(&mut ledgers, value);
    }
    point(at, &ledgers.into_values().collect::<Vec<_>>())
}

fn keep_latest<'a>(values: &mut HashMap<&'a str, &'a Measurement>, value: &'a Measurement) {
    let entry = values.entry(&value.ledger).or_insert(value);
    if value.at_ms > entry.at_ms {
        *entry = value;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 2_000_000_000_000;

    fn measurement(ledger: &str, cost: u64, at: u64) -> Measurement {
        Measurement {
            ledger: ledger.into(),
            account_ids: vec![ledger.into()],
            at_ms: at,
            from_ms: at - 60_000,
            reset_at_ms: NOW + 6 * DAY_MS,
            plan: "plus".into(),
            pricing_revision: "v1".into(),
            consumed: 500,
            micro_usd: Some(cost),
            pricing_coverage_basis_points: 10_000,
            event: None,
            usage_fingerprint: None,
            workload: Some(workload::Workload {
                key: "model-default".into(),
                label: "model".into(),
                models: vec!["model".into()],
                cache_band: 75,
                output_band: 10,
            }),
        }
    }

    fn observed(
        rows: &[Measurement],
        from: u64,
        to: u64,
        bucket: u64,
        now: u64,
    ) -> WeeklyCapacityReport {
        build_report(rows, rows, from, to, bucket, now)
    }

    #[test]
    fn medians_have_one_vote_per_ledger_not_one_vote_per_request() {
        let rows = vec![
            measurement("a", 900, NOW - 2000),
            measurement("a", 100, NOW - 1000),
            measurement("b", 200, NOW),
            measurement("c", 300, NOW),
        ];
        let report = observed(&rows, NOW - HOUR_MS, NOW, HOUR_MS, NOW);
        let point = report.groups[0].current.as_ref().unwrap();
        assert_eq!(point.account_count, 3);
        assert_eq!(point.median_micro_usd, 200);
        assert_eq!((point.low_micro_usd, point.high_micro_usd), (100, 300));
        assert_eq!(point.consumed_basis_points, 1500);
        assert_eq!(report.groups[0].points[0].account_count, 1);
        assert_eq!(report.groups[0].points[0].median_micro_usd, 100);
    }

    #[test]
    fn all_models_deduplicates_ledgers_before_taking_the_median() {
        let mut rows = vec![
            measurement("a", 1_000, NOW - 5_000),
            measurement("d", 400, NOW - 1_000),
        ];
        for (ledger, cost, at) in [
            ("a", 100, NOW - 4_000),
            ("b", 200, NOW - 3_000),
            ("c", 300, NOW - 2_000),
        ] {
            let mut value = measurement(ledger, cost, at);
            value.workload.as_mut().unwrap().key = "another-model".into();
            rows.push(value);
        }
        let report = observed(&rows, NOW - HOUR_MS, NOW, HOUR_MS, NOW);
        assert_eq!(report.groups.len(), 2);
        assert_eq!(report.summaries.len(), 1);
        let summary = &report.summaries[0];
        let point = summary.current.as_ref().unwrap();
        assert_eq!(point.account_count, 4);
        assert_eq!(point.consumed_basis_points, 2_000);
        assert_eq!(point.median_micro_usd, 250);
        assert_eq!(summary.points[0].account_count, 4);
        assert_eq!(summary.points[0].median_micro_usd, 250);
        // A mean of the two model medians would be 450, not this ledger median.
        let json = serde_json::to_value(report).unwrap();
        assert!(json["summaries"][0].get("pricingRevision").is_none());
        assert_eq!(json["groups"][0]["pricingRevision"], "v1");
        assert_eq!(json["summaries"][0]["current"]["medianMicroUsd"], 250);
        assert_eq!(json["models"][0]["model"], "model");
        assert_eq!(json["models"][0]["plan"], "plus");
        assert_eq!(json["models"][0]["current"]["medianMicroUsd"], 250);
        assert_eq!(json["models"][0]["current"]["accountCount"], 4);
        assert!(json["models"][0].get("summary").is_none());
    }

    #[test]
    fn model_choice_limit_does_not_truncate_all_model_evidence() {
        let rows = (1..=33)
            .map(|index| {
                let mut value = measurement(&format!("ledger-{index}"), index, NOW - 1_000);
                value.workload.as_mut().unwrap().key = format!("model-{index}");
                value
            })
            .collect::<Vec<_>>();
        let report = observed(&rows, NOW - HOUR_MS, NOW, HOUR_MS, NOW);
        assert_eq!(report.groups.len(), 32);
        let point = report.summaries[0].current.as_ref().unwrap();
        assert_eq!(point.account_count, 33);
        assert_eq!(point.median_micro_usd, 17);
        assert_eq!(report.models.len(), 1);
        assert_eq!(
            report.models[0]
                .summary
                .current
                .as_ref()
                .unwrap()
                .account_count,
            33
        );
    }

    #[test]
    fn plan_and_model_history_use_each_buckets_own_prices() {
        let original = measurement("a", 100, NOW - DAY_MS);
        let mut plan = measurement("b", 200, NOW - 1_000);
        plan.plan = "pro".into();
        let mut workload = measurement("c", 300, NOW - 2_000);
        workload.workload.as_mut().unwrap().key = "other-model".into();
        let mut price = measurement("d", 400, NOW - 1_000);
        price.pricing_revision = "v2".into();
        let current = [price.clone(), plan.clone()];
        let report = build_report(
            &[original, plan, workload, price],
            &current,
            NOW - 2 * DAY_MS,
            NOW,
            HOUR_MS,
            NOW,
        );
        assert_eq!(report.groups.len(), 4);
        assert_eq!(report.summaries.len(), 2);
        assert_eq!(report.models.len(), 2);
        let plus = report.summaries.iter().find(|s| s.plan == "plus").unwrap();
        let model = report
            .models
            .iter()
            .find(|m| m.summary.plan == "plus")
            .unwrap();
        for series in [plus, &model.summary] {
            assert_eq!(series.points.len(), 2);
            assert_eq!(series.points[0].median_micro_usd, 100);
            assert_eq!(series.points[1].median_micro_usd, 400);
            assert_eq!(series.points[1].account_count, 1);
            assert_eq!(series.current.as_ref().unwrap().median_micro_usd, 400);
        }
        let pro = report.summaries.iter().find(|s| s.plan == "pro").unwrap();
        assert_eq!(pro.points.len(), 1);
        assert_eq!(pro.points[0].median_micro_usd, 200);
    }

    #[test]
    fn month_keeps_daily_history_and_gaps_beyond_the_detail_limit() {
        let from = NOW - 30 * DAY_MS;
        let mut rows = Vec::new();
        for day in (0..30).filter(|&day| day != 13) {
            for change in 1..=3 {
                let cost = 100 + day * 3 + change;
                let mut value = measurement("a", cost, from + day * DAY_MS + change * HOUR_MS);
                value.pricing_revision = format!("price-{cost}");
                rows.push(value);
            }
        }
        rows.reverse();
        let report = build_report(&rows, &[], from, NOW, DAY_MS, NOW);
        assert_eq!(report.groups.len(), 32);
        assert_eq!(report.summaries.len(), 1);
        assert_eq!(report.models.len(), 1);
        for series in [&report.summaries[0], &report.models[0].summary] {
            let points: Vec<_> = series
                .points
                .iter()
                .map(|p| (p.at_ms, p.median_micro_usd))
                .collect();
            let expected: Vec<_> = (0..30)
                .filter(|&d| d != 13)
                .map(|d| (from + d * DAY_MS, 103 + d * 3))
                .collect();
            assert_eq!(points, expected);
            assert!(series.current.is_none());
        }
    }

    #[test]
    fn simultaneous_price_revisions_choose_one_regardless_of_row_order() {
        let a = measurement("a", 100, NOW);
        let mut b = measurement("b", 900, NOW);
        b.pricing_revision = "v2".into();
        for rows in [[&a, &b], [&b, &a]] {
            let point = comparable_point(NOW, &rows).unwrap();
            assert_eq!(point.account_count, 1);
            assert_eq!(point.median_micro_usd, 900);
        }
    }

    #[test]
    fn model_choices_do_not_attribute_mixed_intervals_to_one_model() {
        let single = measurement("a", 100, NOW - 2_000);
        let mut mixed = measurement("b", 9_000, NOW - 1_000);
        let workload = mixed.workload.as_mut().unwrap();
        workload.key = "mixed".into();
        workload.label = "model · standard · short · high + other · standard · short · high".into();
        workload.models = vec!["model".into(), "other".into()];
        let mut other = measurement("c", 300, NOW);
        let workload = other.workload.as_mut().unwrap();
        workload.key = "other".into();
        workload.label = "other · standard · short · high".into();
        workload.models = vec!["other".into()];
        let report = observed(&[single, mixed, other], NOW - HOUR_MS, NOW, HOUR_MS, NOW);
        assert_eq!(
            report.summaries[0].current.as_ref().unwrap().account_count,
            3
        );
        assert_eq!(report.models.len(), 2);
        assert_eq!(report.models[0].model, "other");
        for model in &report.models {
            let current = model.summary.current.as_ref().unwrap();
            assert_eq!(current.account_count, 1);
            assert_eq!(
                current.median_micro_usd,
                if model.model == "model" { 100 } else { 300 }
            );
        }
    }

    #[test]
    fn legacy_workloads_keep_single_model_history_without_inventing_mixed_splits() {
        for (label, expected) in [
            ("model · standard · short · high", Some("model")),
            (
                "model · standard · short · high + model · priority · long · low",
                Some("model"),
            ),
            (
                "model · standard · short · high + other · priority · long · low",
                None,
            ),
            ("", None),
        ] {
            let workload: workload::Workload = serde_json::from_value(serde_json::json!({
                "key":"legacy", "label":label, "cache_band":75, "output_band":10
            }))
            .unwrap();
            assert_eq!(workload.single_model(), expected);
        }
    }

    #[test]
    fn prefers_fresh_evidence_then_expires_stale_values_and_resets() {
        let stale = measurement("a", 900, NOW - 2 * DAY_MS);
        let fresh = measurement("b", 100, NOW);
        let report = observed(
            &[stale.clone(), fresh.clone()],
            NOW - 30 * DAY_MS,
            NOW,
            DAY_MS,
            NOW,
        );
        assert_eq!(
            report.groups[0].current.as_ref().unwrap().median_micro_usd,
            100
        );
        assert!(!report.groups[0].stale);
        assert_eq!(
            report.summaries[0]
                .current
                .as_ref()
                .unwrap()
                .median_micro_usd,
            100
        );
        assert!(!report.summaries[0].stale);
        let report = observed(
            std::slice::from_ref(&stale),
            NOW - 30 * DAY_MS,
            NOW,
            DAY_MS,
            NOW,
        );
        assert!(report.groups[0].stale);
        assert_eq!(
            report.groups[0].current.as_ref().unwrap().median_micro_usd,
            900
        );
        assert!(report.summaries[0].stale);
        assert_eq!(
            report.summaries[0]
                .current
                .as_ref()
                .unwrap()
                .median_micro_usd,
            900
        );
        let report = observed(&[stale], NOW - 30 * DAY_MS, NOW, DAY_MS, NOW + 2 * DAY_MS);
        assert!(report.groups[0].current.is_none());
        assert!(report.summaries[0].current.is_none());
        let mut reset = fresh.clone();
        reset.at_ms += 1;
        reset.event = Some(CapacityEventKind::Reset);
        reset.micro_usd = None;
        reset.workload = None;
        let report = build_report(
            &[fresh, reset.clone(), reset],
            &[],
            NOW - DAY_MS,
            NOW + 1,
            HOUR_MS,
            NOW + 1,
        );
        assert!(report.groups[0].current.is_none());
        assert_eq!(report.events[0].count, 1);
        assert_eq!(report.groups[0].points.len(), 1);
        assert!(report.summaries[0].current.is_none());
        assert_eq!(report.summaries[0].points.len(), 1);
    }
}
