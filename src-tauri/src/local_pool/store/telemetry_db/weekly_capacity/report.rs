use super::*;

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WeeklyCapacityQuery {
    pub from_ms: Option<u64>,
    pub to_ms: Option<u64>,
    pub bucket_ms: Option<u64>,
    pub account_id: Option<String>,
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
    pub pricing_revision: String,
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
        let rows = {
            // Use the existing WAL report reader, not the request writer during aggregation.
            let connection = self.lock_report_connection()?;
            let mut statement = connection.prepare(
                "SELECT payload_json FROM weekly_capacity_history WHERE observed_at_ms >= ?1 AND observed_at_ms <= ?2
                 ORDER BY observed_at_ms DESC LIMIT ?3",
            ).map_err(db_error)?;
            let rows = statement
                .query_map(
                    params![
                        sql_u64(from.min(now_ms.saturating_sub(MAX_ANCHOR_AGE))),
                        sql_u64(to),
                        MAX_HISTORY as i64
                    ],
                    |row| row.get::<_, String>(0),
                )
                .map_err(db_error)?;
            let mut values = Vec::new();
            for row in rows {
                let value: Measurement =
                    serde_json::from_str(&row.map_err(db_error)?).map_err(json_error)?;
                if query
                    .account_id
                    .as_ref()
                    .is_none_or(|id| value.account_ids.contains(id))
                {
                    values.push(value);
                }
            }
            values
        };
        Ok(build_report(rows, from, to, bucket, now_ms))
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
                    .min(value.at_ms.saturating_add(MAX_ANCHOR_AGE))
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

fn build_report(
    rows: Vec<Measurement>,
    from: u64,
    to: u64,
    bucket: u64,
    now: u64,
) -> WeeklyCapacityReport {
    let mut groups = BTreeMap::<String, Vec<&Measurement>>::new();
    let mut plans = BTreeMap::<(String, String), Vec<&Measurement>>::new();
    let mut models = BTreeMap::<(String, String, String), Vec<&Measurement>>::new();
    let mut events =
        BTreeMap::<(u64, String, CapacityEventKind), std::collections::BTreeSet<&str>>::new();
    let mut latest_event = HashMap::<&str, u64>::new();
    for value in &rows {
        if let Some(kind) = value.event {
            latest_event
                .entry(&value.ledger)
                .and_modify(|at| *at = (*at).max(value.at_ms))
                .or_insert(value.at_ms);
            if (from..=to).contains(&value.at_ms) {
                let at = from + (value.at_ms - from) / bucket * bucket;
                events
                    .entry((at, value.plan.clone(), kind))
                    .or_default()
                    .insert(&value.ledger);
            }
        }
        if let Some(workload) = &value.workload {
            if value.micro_usd.is_some() {
                let key = fingerprint(&format!(
                    "{}\0{}\0{}",
                    value.plan, workload.key, value.pricing_revision
                ));
                groups.entry(key).or_default().push(value);
                plans
                    .entry((value.plan.clone(), value.pricing_revision.clone()))
                    .or_default()
                    .push(value);
                if let Some(model) = workload.single_model() {
                    models
                        .entry((
                            value.plan.clone(),
                            model.to_owned(),
                            value.pricing_revision.clone(),
                        ))
                        .or_default()
                        .push(value);
                }
            }
        }
    }
    let mut result = Vec::new();
    for (id, values) in groups {
        let Some(first) = values.first() else {
            continue;
        };
        let Some(workload) = &first.workload else {
            continue;
        };
        let Some(summary) = summarize_measurements(&values, &latest_event, from, to, bucket, now)
        else {
            continue;
        };
        result.push(WeeklyCapacityGroup {
            id,
            plan: first.plan.clone(),
            workload: workload.label.clone(),
            cache_band: workload.cache_band,
            output_band: workload.output_band,
            pricing_revision: first.pricing_revision.clone(),
            points: summary.points,
            current: summary.current,
            stale: summary.stale,
        });
    }
    // Aggregate raw observations before truncating model choices. Averaging the
    // model medians would overweight small groups and count linked ledgers again.
    let mut summaries = plans
        .into_values()
        .filter_map(|values| {
            let latest = values.iter().map(|value| value.at_ms).max()?;
            let summary = summarize_measurements(&values, &latest_event, from, to, bucket, now)?;
            Some((latest, summary))
        })
        .collect::<Vec<_>>();
    summaries.sort_by_key(|(latest, _)| std::cmp::Reverse(*latest));
    summaries.truncate(32);
    // Combine raw single-model observations, not already-aggregated workload
    // medians. Mixed-model intervals remain in the all-model plan summary.
    let mut models = models
        .into_iter()
        .filter_map(|((_, model, _), values)| {
            let latest = values.iter().map(|value| value.at_ms).max()?;
            let summary = summarize_measurements(&values, &latest_event, from, to, bucket, now)?;
            Some((latest, WeeklyCapacityModel { model, summary }))
        })
        .collect::<Vec<_>>();
    models.sort_by_key(|(latest, _)| std::cmp::Reverse(*latest));
    models.truncate(32);
    // Bound renderer work; prefer groups with the newest actual measurements.
    result.sort_by_key(|group| {
        std::cmp::Reverse(group.points.last().map_or(0, |point| point.observed_at_ms))
    });
    result.truncate(32);
    WeeklyCapacityReport {
        generated_at_ms: now,
        summaries: summaries.into_iter().map(|(_, summary)| summary).collect(),
        models: models.into_iter().map(|(_, model)| model).collect(),
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

fn summarize_measurements(
    values: &[&Measurement],
    latest_event: &HashMap<&str, u64>,
    from: u64,
    to: u64,
    bucket: u64,
    now: u64,
) -> Option<WeeklyCapacitySummary> {
    let first = values.first()?;
    let mut buckets = BTreeMap::<u64, HashMap<&str, &Measurement>>::new();
    let mut current = HashMap::<&str, &Measurement>::new();
    for &value in values {
        if (from..=to).contains(&value.at_ms) {
            let at = from + (value.at_ms - from) / bucket * bucket;
            keep_latest(buckets.entry(at).or_default(), value);
        }
        if value.at_ms >= now.saturating_sub(MAX_ANCHOR_AGE)
            && value.reset_at_ms > now
            && latest_event
                .get(value.ledger.as_str())
                .is_none_or(|event| *event < value.at_ms)
        {
            keep_latest(&mut current, value);
        }
    }
    let fresh = current
        .values()
        .copied()
        .filter(|value| value.from_ms >= now.saturating_sub(DAY_MS))
        .collect::<Vec<_>>();
    let stale = fresh.is_empty();
    let current = if stale {
        current.values().copied().collect::<Vec<_>>()
    } else {
        fresh
    };
    Some(WeeklyCapacitySummary {
        plan: first.plan.clone(),
        pricing_revision: first.pricing_revision.clone(),
        points: buckets
            .into_iter()
            .filter_map(|(at, values)| point(at, &values.values().copied().collect::<Vec<_>>()))
            .collect(),
        current: point(now, &current),
        stale,
    })
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
            event: None,
            workload: Some(workload::Workload {
                key: "model-default".into(),
                label: "model".into(),
                models: vec!["model".into()],
                cache_band: 75,
                output_band: 10,
            }),
        }
    }

    #[test]
    fn medians_have_one_vote_per_ledger_not_one_vote_per_request() {
        let rows = vec![
            measurement("a", 900, NOW - 2000),
            measurement("a", 100, NOW - 1000),
            measurement("b", 200, NOW),
            measurement("c", 300, NOW),
        ];
        let report = build_report(rows, NOW - HOUR_MS, NOW, HOUR_MS, NOW);
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
        let report = build_report(rows, NOW - HOUR_MS, NOW, HOUR_MS, NOW);
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
        assert_eq!(json["summaries"][0]["pricingRevision"], "v1");
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
            .collect();
        let report = build_report(rows, NOW - HOUR_MS, NOW, HOUR_MS, NOW);
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
    fn separates_plans_workloads_and_price_revisions() {
        let original = measurement("a", 100, NOW);
        let mut plan = measurement("b", 200, NOW);
        plan.plan = "pro".into();
        let mut workload = measurement("c", 300, NOW);
        workload.workload.as_mut().unwrap().key = "other-model".into();
        let mut price = measurement("d", 400, NOW);
        price.pricing_revision = "v2".into();
        let report = build_report(
            vec![original, plan, workload, price],
            NOW - DAY_MS,
            NOW,
            HOUR_MS,
            NOW,
        );
        assert_eq!(report.groups.len(), 4);
        assert!(report
            .groups
            .iter()
            .all(|group| group.current.as_ref().unwrap().account_count == 1));
        assert_eq!(report.summaries.len(), 3);
        assert_eq!(report.models.len(), 3);
        let plus = report
            .summaries
            .iter()
            .find(|summary| summary.plan == "plus" && summary.pricing_revision == "v1")
            .unwrap();
        assert_eq!(plus.current.as_ref().unwrap().median_micro_usd, 200);
        assert_eq!(plus.current.as_ref().unwrap().account_count, 2);
        assert!(report
            .summaries
            .iter()
            .filter(|summary| summary.plan != "plus" || summary.pricing_revision != "v1")
            .all(|summary| summary.current.as_ref().unwrap().account_count == 1));
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
        let report = build_report(vec![single, mixed, other], NOW - HOUR_MS, NOW, HOUR_MS, NOW);
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
        let report = build_report(
            vec![stale.clone(), fresh.clone()],
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
        let report = build_report(vec![stale.clone()], NOW - 30 * DAY_MS, NOW, DAY_MS, NOW);
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
        let report = build_report(
            vec![stale],
            NOW - 30 * DAY_MS,
            NOW,
            DAY_MS,
            NOW + 2 * DAY_MS,
        );
        assert!(report.groups[0].current.is_none());
        assert!(report.summaries[0].current.is_none());
        let mut reset = fresh.clone();
        reset.at_ms += 1;
        reset.event = Some(CapacityEventKind::Reset);
        reset.micro_usd = None;
        reset.workload = None;
        let report = build_report(
            vec![fresh, reset.clone(), reset],
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
