use super::*;
use serde_json::{Value, json};

fn event(id: &str, effort: Option<&str>, ttft: Option<u64>) -> UsageEvent {
    let mut event = aggregate_test_event(id, 1, 1000, 0, None, 20);
    event.requested_reasoning_effort = effort.map(str::to_owned);
    event.effective_reasoning_effort = effort.map(str::to_owned);
    event.ttft_ms = ttft;
    event.cached_input_tokens = Some(800);
    event.routing = Some(
        serde_json::from_value(json!({
            "reason":"only_eligible","eligibleCandidates":1,"inFlightBefore":0,"dispatchesBefore":0,
            "transport":{"upstream":"http","mode":"basis_points"}
        }))
        .unwrap(),
    );
    event
}

fn database() -> (std::path::PathBuf, TelemetryDb) {
    let root = crate::files::test_temp_root("relay-reasoning");
    let db = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
    (root, db)
}

fn group<'a>(report: &'a Value, effort: &str) -> &'a Value {
    report["reasoningLatencyComparison"]["groups"]
        .as_array()
        .unwrap()
        .iter()
        .find(|group| group["effectiveReasoningEffort"] == effort)
        .unwrap()
}

#[test]
fn reasoning_comparison_is_same_route_single_send_and_never_recommends_a_level() {
    let (root, db) = database();
    for variant in 0..11 {
        let mut other = event(
            &format!("PRIVATE_OTHER_{variant}"),
            Some("medium"),
            Some(99999),
        );
        match variant {
            0 => other.source_id = "PRIVATE_SOURCE".into(),
            1 => other.candidate_id = Some("PRIVATE_CANDIDATE".into()),
            2 => other.account_id = Some("PRIVATE_ACCOUNT".into()),
            3 => other.requested_model = Some("PRIVATE_REQUEST_MODEL".into()),
            4 => other.resolved_model = Some("PRIVATE_RESOLVED_MODEL".into()),
            5 => other.wire_api = WireApi::Messages,
            6 => other.transport = zenith_relay_core::UsageTransport::Websocket,
            7 => other.local_key_id = "PRIVATE_KEY".into(),
            8 => other.applied_service_tier = Some("priority".into()),
            9 => {
                other
                    .routing
                    .as_mut()
                    .unwrap()
                    .transport
                    .as_mut()
                    .unwrap()
                    .upstream = zenith_relay_core::UsageTransport::Websocket
            }
            _ => {
                other
                    .routing
                    .as_mut()
                    .unwrap()
                    .transport
                    .as_mut()
                    .unwrap()
                    .mode = zenith_relay_core::usage::TransportMode::Native
            }
        }
        db.record(&other).unwrap();
    }
    let mut failed = event("failed", Some("medium"), Some(99999));
    failed.success = false;
    db.record(&failed).unwrap();
    let mut discovery = event("multi_send", Some("medium"), Some(99999));
    discovery.attempt = 2;
    db.record(&discovery).unwrap();
    db.record(&event(
        "PRIVATE_UNKNOWN",
        Some("PRIVATE_EFFORT"),
        Some(99999),
    ))
    .unwrap();
    db.record(&event("missing_ttft", Some("medium"), None))
        .unwrap();
    for i in 0..5 {
        db.record(&event(&format!("low_{i}"), Some("low"), Some(i * 100)))
            .unwrap();
        db.record(&event(
            &format!("high_{i}"),
            Some("high"),
            Some((i + 1) * 1000),
        ))
        .unwrap();
    }
    let report = db.request_diagnostics("high_4").unwrap();
    let comparison = &report["reasoningLatencyComparison"];
    assert_eq!(comparison["status"], "multiple_levels_observed");
    assert_eq!(comparison["kind"], "observational_not_controlled");
    assert_eq!(comparison["matchedRouteRequests"], 14);
    assert_eq!(comparison["groups"].as_array().unwrap().len(), 2);
    assert_eq!(
        comparison["excluded"],
        json!({"unsuccessful":1,"multipleSends":1,"unknownEffectiveReasoning":1,"missingTtft":1})
    );
    assert_eq!(group(&report, "low")["samples"], 5);
    assert_eq!(group(&report, "low")["ttftP50Ms"], 200);
    assert_eq!(group(&report, "high")["ttftP50Ms"], 3000);
    assert!(group(&report, "low")["ttftP95Ms"].is_null());
    assert_eq!(
        group(&report, "low")["cachedInputFractionP50BasisPoints"],
        8000
    );
    assert_eq!(
        report["finalRequest"]["protocol"]["effectiveReasoningEffort"],
        "high"
    );
    assert!(!report.to_string().contains("PRIVATE_"));
    assert!(report.to_string().len() < 256 * 1024);
    drop(db);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn reasoning_comparison_keeps_unknowns_and_zero_and_excludes_future_and_old_rows() {
    let (root, db) = database();
    let mut old = event("old", Some("low"), Some(99999));
    db.record(&old).unwrap();
    db.lock_connection().unwrap().execute("UPDATE request_logs SET created_at = datetime('now','-8 days') WHERE request_id = 'old'", []).unwrap();
    old.request_id = "zero".into();
    old.ttft_ms = Some(0);
    old.input_tokens = None;
    old.cached_input_tokens = None;
    db.record(&old).unwrap();
    db.record(&event("future", Some("high"), Some(99999)))
        .unwrap();
    let report = db.request_diagnostics("zero").unwrap();
    assert_eq!(
        report["reasoningLatencyComparison"]["status"],
        "insufficient_samples"
    );
    assert_eq!(
        report["reasoningLatencyComparison"]["matchedRouteRequests"],
        1
    );
    let low = group(&report, "low");
    assert_eq!(low["ttftP50Ms"], 0);
    assert_eq!(low["inputTokenSamples"], 0);
    assert_eq!(low["cacheRateSamples"], 0);
    assert!(low["inputTokensP50"].is_null());
    assert!(low["cachedInputFractionP50BasisPoints"].is_null());
    let mut unknown = event("unknown_transport", Some("low"), Some(1));
    unknown.routing = None;
    db.record(&unknown).unwrap();
    let report = db.request_diagnostics("unknown_transport").unwrap();
    assert_eq!(
        report["reasoningLatencyComparison"]["unavailableReason"],
        "transport_observation_missing"
    );
    drop(db);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn reasoning_comparison_bounds_history_and_only_reports_supported_percentile_samples() {
    let (root, db) = database();
    db.record(&event("outside_window", Some("low"), Some(99999)))
        .unwrap();
    for i in 0..1000 {
        db.record(&event(&format!("sample_{i}"), Some("high"), Some(i)))
            .unwrap();
    }
    let report = db.request_diagnostics("sample_999").unwrap();
    let comparison = &report["reasoningLatencyComparison"];
    assert_eq!(comparison["matchedRouteRequests"], 1000);
    assert_eq!(comparison["groups"].as_array().unwrap().len(), 1);
    assert_eq!(comparison["status"], "insufficient_samples");
    assert_eq!(group(&report, "high")["ttftP50Ms"], 499);
    assert_eq!(group(&report, "high")["ttftP95Ms"], 949);
    drop(db);
    std::fs::remove_dir_all(root).unwrap();
}
