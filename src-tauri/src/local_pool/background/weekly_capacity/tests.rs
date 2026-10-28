use super::*;
use crate::local_pool::store::telemetry_db::WeeklyCapacityQuery;

const NOW: u64 = 2_000_000_000_000;

fn facts(kind: OAuthClientKind) -> Option<AccountCredentialFacts> {
    Some(AccountCredentialFacts {
        credit_balance_key: Some([7; 32]),
        oauth_client_kind: kind,
        has_oauth: true,
        has_refresh_token: true,
        plan_use_enabled: true,
        expires_at_ms: None,
        agent_identity: false,
        has_provider_account_id: true,
        has_account_proxy: false,
        account_proxy_valid: false,
        bypass_common_proxy: false,
    })
}

#[test]
fn collector_uses_shared_weekly_evidence_not_siwc_ceiling_without_ui_snapshot() {
    let root = crate::files::test_temp_root("capacity-collector");
    let telemetry = TelemetryDb::open(&root.join("test.sqlite")).unwrap();
    let mut codex = super::super::tests::account_record();
    codex.account.id = "codex".into();
    codex.account.subscription.plan_type = Some("plus".into());
    codex.account.quota.primary = Some(
        serde_json::from_value(serde_json::json!({
            "kind":"primary", "windowStartMs":NOW - 17_970_000,
            "resetAtMs":NOW + 30_000, "availableBasisPoints":0,
            "windowMinutes":300, "observedAtMs":NOW,
        }))
        .unwrap(),
    );
    codex.account.quota.secondary = Some(
        serde_json::from_value(serde_json::json!({
            "kind":"secondary", "windowStartMs":NOW - 86_400_000, "resetAtMs":NOW + 6 * 86_400_000,
            "availableBasisPoints":9000, "windowMinutes":10080, "observedAtMs":NOW,
        }))
        .unwrap(),
    );
    let mut bps = codex.clone();
    bps.account.id = "bps".into();
    bps.account.quota.secondary.as_mut().unwrap().observed_at_ms += 1000;
    let mut siwc = codex.clone();
    siwc.account.id = "siwc".into();
    siwc.account.subscription.plan_type = Some("not-shared".into());
    siwc.account
        .quota
        .secondary
        .as_mut()
        .unwrap()
        .observed_at_ms += 2000;
    let mut inventory = vec![codex, bps, siwc];
    let facts = HashMap::from([
        ("codex".into(), facts(OAuthClientKind::Codex)),
        ("bps".into(), facts(OAuthClientKind::ExcelBps)),
        ("siwc".into(), facts(OAuthClientKind::Siwc)),
    ]);
    let ledgers = subscription_ledgers(&inventory, &facts);
    assert_eq!(ledgers.len(), 1);
    assert_eq!(ledgers[0].account_ids, ["bps", "codex", "siwc"]);
    assert_eq!(ledgers[0].plan, "plus");
    assert_eq!(ledgers[0].window.observed_at_ms, NOW + 1000);
    assert!(subscription_ledgers(&inventory[2..], &facts).is_empty());
    let catalog = PricingCatalog::from_litellm_payload(&serde_json::json!({"model":{
        "litellm_provider":"openai", "input_cost_per_token":0.000002, "output_cost_per_token":0.00001,
    }}), Some("synthetic".into()), Some(1), false).unwrap();
    let observe = |inventory: &[LocalAccountRecord], now| {
        record_capacity(
            inventory,
            &facts,
            &telemetry,
            &catalog,
            &PricingContext::default(),
            now,
        )
        .unwrap()
    };
    observe(&inventory, NOW + 2000);
    let revision_before_reset = telemetry.weekly_capacity_revision();
    let weekly_before_reset = subscription_ledgers(&inventory, &facts)[0].window.clone();
    let burst = inventory[1].account.quota.primary.as_mut().unwrap();
    burst.window_start_ms = Some(NOW + 30_000);
    burst.reset_at_ms = Some(NOW + 18_030_000);
    burst.available_basis_points = Some(10_000);
    burst.explicitly_full = Some(true);
    burst.observed_at_ms = NOW + 30_000;
    observe(&inventory, NOW + 30_000);
    assert_eq!(
        subscription_ledgers(&inventory, &facts)[0].window,
        &weekly_before_reset
    );
    assert_eq!(telemetry.weekly_capacity_revision(), revision_before_reset);
    inventory[1].account.subscription.plan_type = Some("pro".into());
    inventory[1]
        .account
        .quota
        .secondary
        .as_mut()
        .unwrap()
        .observed_at_ms = NOW + 60_000;
    observe(&inventory, NOW + 60_000);
    let report = telemetry
        .weekly_capacity_report(WeeklyCapacityQuery::default(), NOW + 60_000)
        .unwrap();
    assert_eq!(report.events.len(), 1);
    assert_eq!(report.events[0].plan, "pro");
    assert_eq!(report.events[0].count, 1);
    let mut credits_only = facts.clone();
    credits_only
        .get_mut("siwc")
        .unwrap()
        .as_mut()
        .unwrap()
        .plan_use_enabled = false;
    assert_eq!(
        subscription_ledgers(&inventory, &credits_only)[0].account_ids,
        ["bps", "codex"]
    );
    record_capacity(
        &inventory,
        &credits_only,
        &telemetry,
        &catalog,
        &PricingContext::default(),
        NOW + 60_000,
    )
    .unwrap();
    let scoped = telemetry
        .weekly_capacity_report(
            WeeklyCapacityQuery {
                account_id: Some("siwc".into()),
                ..Default::default()
            },
            NOW + 60_000,
        )
        .unwrap();
    assert!(
        scoped.events.is_empty(),
        "credits-only use must not enter subscription evidence"
    );
    telemetry.clear().unwrap();
    observe(&inventory[2..], NOW + 60_000);
    inventory[2].account.subscription.plan_type = Some("pro".into());
    inventory[2]
        .account
        .quota
        .secondary
        .as_mut()
        .unwrap()
        .observed_at_ms = NOW + 120_000;
    observe(&inventory[2..], NOW + 120_000);
    assert!(
        telemetry
            .weekly_capacity_report(WeeklyCapacityQuery::default(), NOW + 120_000)
            .unwrap()
            .events
            .is_empty()
    );
    drop(telemetry);
    std::fs::remove_dir_all(root).unwrap();
}
