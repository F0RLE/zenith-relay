use super::*;

const NOW: u64 = 2_000_000_000_000;

fn window() -> QuotaWindow {
    serde_json::from_value(serde_json::json!({
        "kind":"secondary", "windowStartMs":NOW - 3 * DAY_MS,
        "resetAtMs":NOW + 4 * DAY_MS, "availableBasisPoints":8400,
        "windowMinutes":10080, "observedAtMs":NOW,
    }))
    .unwrap()
}

fn observation<'a>(
    ledger: &'a str,
    ids: &'a [String],
    window: &'a QuotaWindow,
) -> CapacityObservation<'a> {
    CapacityObservation {
        ledger,
        account_ids: ids,
        plan: "plus",
        window,
        pricing_revision: "v1",
        now_ms: NOW,
    }
}

fn sample(current: &CapacityObservation<'_>, cost: u64, profile: &str) -> Measurement {
    Measurement {
        ledger: current.ledger.into(),
        account_ids: current.account_ids.to_vec(),
        at_ms: NOW,
        from_ms: NOW - 60_000,
        reset_at_ms: current.window.reset_at_ms.unwrap(),
        plan: current.plan.into(),
        pricing_revision: fingerprint(current.pricing_revision),
        consumed: 500,
        micro_usd: Some(cost),
        pricing_coverage_basis_points: 10_000,
        event: None,
        workload: Some(workload::Workload {
            key: profile.into(),
            label: format!("model-{profile}"),
            models: vec![format!("model-{profile}")],
            cache_band: 75,
            output_band: 10,
        }),
    }
}

fn anchor(current: &CapacityObservation<'_>) -> Anchor {
    Anchor {
        account_ids: current.account_ids.to_vec(),
        plan: current.plan.into(),
        pricing_revision: fingerprint(current.pricing_revision),
        members: fingerprint(&current.account_ids.join("\0")),
        window: current.window.clone(),
        from_ms: NOW,
        remaining: 8400,
    }
}

#[test]
fn different_workloads_share_the_observed_median_not_a_range_midpoint() {
    let window = window();
    let a = vec!["a-codex".into(), "a-bps".into(), "a-siwc".into()];
    let b = vec!["b".into()];
    let c = vec!["c".into()];
    let d = vec!["different-workload".into()];
    let new = vec!["new-account".into()];
    let observations = vec![
        observation("a", &a, &window),
        observation("b", &b, &window),
        observation("c", &c, &window),
        observation("d", &d, &window),
        observation("new", &new, &window),
    ];
    let samples = observations[..4]
        .iter()
        .enumerate()
        .map(|(index, value)| {
            (
                value.ledger,
                (
                    value,
                    sample(
                        value,
                        [100, 200, 300, 10_000][index],
                        if index == 3 { "other" } else { "common" },
                    ),
                ),
            )
        })
        .collect();
    let result = project(&observations, &samples);
    assert_eq!(result.len(), 7);
    for value in result.values() {
        assert_eq!(value.calibration.median_micro_usd, Some(250));
        assert_eq!(value.calibration.account_count, 4);
        assert_eq!(value.calibration.profile_count, 2);
        assert_eq!(
            (
                value.calibration.low_micro_usd,
                value.calibration.high_micro_usd
            ),
            (100, 10_000)
        );
        assert_eq!(value.available_basis_points, 8400);
    }
    assert_eq!(result["a-codex"], result["a-siwc"]);
    let json = serde_json::to_value(&result["a-codex"]).unwrap();
    assert_eq!(json["ledgerKey"], "a");
    assert_eq!(json["availableBasisPoints"], 8400);
    assert_eq!(json["calibration"]["medianMicroUsd"], 250);
    assert!(json.get("apiEquivalent").is_none());
    assert!(json.get("recentApiEquivalent").is_none());
}

#[test]
fn old_persisted_measurements_keep_full_coverage_and_single_samples_stay_explicit() {
    let quota = window();
    let ids = vec!["a".into()];
    let current = observation("a", &ids, &quota);
    let mut legacy = serde_json::to_value(sample(&current, 100, "one")).unwrap();
    legacy
        .as_object_mut()
        .unwrap()
        .remove("pricing_coverage_basis_points");
    let restored: Measurement = serde_json::from_value(legacy).unwrap();
    assert_eq!(restored.pricing_coverage_basis_points, 10_000);
    let values = project(
        &[observation("a", &ids, &quota)],
        &BTreeMap::from([("a", (&current, restored))]),
    );
    assert_eq!(values["a"].calibration.account_count, 1);
    assert_eq!(values["a"].calibration.profile_count, 1);
    assert_eq!(values["a"].calibration.median_micro_usd, Some(100));
}

#[test]
fn cohorts_never_mix_plans_prices_or_weekly_durations() {
    let window = window();
    let mut shorter = window.clone();
    shorter.window_minutes = Some(6 * 1440);
    let ids = (0..4)
        .map(|i| vec![format!("account-{i}")])
        .collect::<Vec<_>>();
    let mut observations = vec![
        observation("a", &ids[0], &window),
        observation("b", &ids[1], &window),
        observation("c", &ids[2], &window),
        observation("d", &ids[3], &shorter),
    ];
    observations[1].plan = "pro";
    observations[2].pricing_revision = "v2";
    let samples = observations
        .iter()
        .enumerate()
        .map(|(i, value)| {
            (
                value.ledger,
                (value, sample(value, (i as u64 + 1) * 100, "same")),
            )
        })
        .collect();
    let result = project(&observations, &samples);
    for (i, id) in ids.iter().enumerate() {
        assert_eq!(
            result[&id[0]].calibration.median_micro_usd,
            Some((i as u64 + 1) * 100)
        );
    }
}

#[test]
fn freshness_is_per_profile_and_equal_groups_do_not_have_a_hash_selected_winner() {
    let window = window();
    let a = vec!["a".into()];
    let b = vec!["b".into()];
    let observations = vec![observation("a", &a, &window), observation("b", &b, &window)];
    let mut old = sample(&observations[0], 900, "a");
    old.from_ms = NOW - 2 * DAY_MS;
    old.at_ms = old.from_ms + 60_000;
    let fresh = sample(&observations[1], 200, "b");
    let samples = BTreeMap::from([
        ("a", (&observations[0], old.clone())),
        ("b", (&observations[1], fresh)),
    ]);
    let result = project(&observations, &samples);
    assert_eq!(result["a"].calibration.median_micro_usd, Some(550));
    assert_eq!(result["a"].calibration.low_micro_usd, 200);
    assert_eq!(result["a"].calibration.high_micro_usd, 900);
    assert_eq!(
        result["a"].calibration.valid_until_ms,
        NOW - 60_000 + DAY_MS
    );
    let fallback = project(
        &observations,
        &BTreeMap::from([("a", (&observations[0], old))]),
    );
    assert_eq!(fallback["b"].calibration.median_micro_usd, Some(900));
    let samples = BTreeMap::from([
        ("a", (&observations[0], sample(&observations[0], 100, "a"))),
        ("b", (&observations[1], sample(&observations[1], 200, "b"))),
    ]);
    assert_eq!(
        project(&observations, &samples)["b"]
            .calibration
            .median_micro_usd,
        Some(150)
    );
    assert_eq!(
        project(&observations, &samples)["b"]
            .calibration
            .low_micro_usd,
        100
    );
    assert_eq!(
        project(&observations, &samples)["b"]
            .calibration
            .high_micro_usd,
        200
    );
    assert!(project(&observations, &BTreeMap::new()).is_empty());
}

#[test]
fn current_inventory_and_cycles_fence_persisted_measurements() {
    let mut window = window();
    let ids = vec!["a".into()];
    let current = observation("ledger", &ids, &window);
    let anchor = anchor(&current);
    let value = sample(&current, 100, "a");
    assert!(compatible_sample(&anchor, &value, &current));
    for change in 0..7 {
        let mut changed = value.clone();
        match change {
            0 => changed.pricing_revision = "old".into(),
            1 => changed.account_ids = vec!["unlinked".into()],
            2 => changed.event = Some(CapacityEventKind::Reset),
            3 => changed.consumed = 499,
            4 => changed.workload = None,
            5 => changed.from_ms = NOW - MAX_ANCHOR_AGE - 1,
            _ => changed.at_ms = NOW + 1,
        }
        assert!(
            !compatible_sample(&anchor, &changed, &current),
            "case {change}"
        );
    }
    let mut changed = observation("ledger", &ids, &window);
    changed.plan = "pro";
    assert!(!compatible_sample(&anchor, &value, &changed));
    window.available_basis_points = Some(9000); // same-timestamp correction
    assert!(!compatible_sample(
        &anchor,
        &value,
        &observation("ledger", &ids, &window)
    ));
    window.window_start_ms = anchor.window.reset_at_ms;
    window.observed_at_ms = NOW + 4 * DAY_MS;
    assert!(!compatible_sample(
        &anchor,
        &value,
        &observation("ledger", &ids, &window)
    ));
}

#[test]
fn missing_future_expired_and_nonweekly_quota_never_produce_a_target() {
    let mut window = window();
    let ids = vec!["a".into()];
    assert!(valid_window(&observation("ledger", &ids, &window)));
    window.observed_at_ms = NOW + 1;
    assert!(!valid_window(&observation("ledger", &ids, &window)));
    window.observed_at_ms = NOW - MAX_ANCHOR_AGE;
    assert!(!valid_window(&observation("ledger", &ids, &window)));
    window.observed_at_ms = NOW;
    window.reset_at_ms = Some(NOW);
    assert!(!valid_window(&observation("ledger", &ids, &window)));
    window.reset_at_ms = Some(NOW + DAY_MS);
    window.window_minutes = Some(300);
    assert!(!valid_window(&observation("ledger", &ids, &window)));
    window.window_minutes = Some(10080);
    window.available_basis_points = None;
    assert!(!valid_window(&observation("ledger", &ids, &window)));
}
