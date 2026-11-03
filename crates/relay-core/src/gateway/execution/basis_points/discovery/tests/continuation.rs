use super::*;
use crate::gateway::request_body::retained_request_bytes;

fn fixture(kind: &str) -> (Value, Value) {
    let mut request = request();
    let mut reply = response(LOOKUP_TOOL, json!({"names":["files.read"]}));
    match kind {
        "small" => {}
        "schema" => {
            request["tools"][0]["tools"][0]["parameters"]["description"] =
                json!("synthetic schema ".repeat(4096));
        }
        "fragments" => {
            let content = (0..512)
                .map(|index| {
                    json!({"type":"output_text","text":format!("synthetic-{index}:{}", "x".repeat(256))})
                })
                .collect::<Vec<_>>();
            reply["output"].as_array_mut().unwrap().insert(
                0,
                json!({"type":"message","role":"assistant","content":content}),
            );
            reply["output"].as_array_mut().unwrap().insert(
                0,
                json!({"type":"reasoning","id":"rs_synthetic","encrypted_content":"synthetic".repeat(16 * 1024)}),
            );
        }
        _ => panic!("unknown synthetic fixture"),
    }
    (request, reply)
}

#[test]
fn continuation_accounting_covers_retained_tool_results() {
    for kind in ["small", "schema", "fragments"] {
        let (request, reply) = fixture(kind);
        let mut upstream = prepare_request(&request).unwrap();
        let start = upstream["input"].as_array().unwrap().len();
        let mut session = DiscoverySession::new(&request, true).unwrap().unwrap();
        assert!(session.advance(&reply, &mut upstream).unwrap());
        // Move the appended values: cloning would shrink string capacities and
        // hide the allocation retained by the real request's tool-result text.
        let added = upstream["input"].as_array_mut().unwrap().split_off(start);
        assert_eq!(
            &added[..added.len() - 1],
            reply["output"].as_array().unwrap()
        );
        let result = added.last().unwrap();
        assert_eq!(
            result["call_id"],
            reply["output"].as_array().unwrap().last().unwrap()["call_id"]
        );
        let result: Value = serde_json::from_str(result["output"].as_str().unwrap()).unwrap();
        assert_eq!(
            result["tools"][0]["parameters"],
            request["tools"][0]["tools"][0]["parameters"]
        );
        let retained = retained_request_bytes(&Value::Array(added));
        assert!(
            session.continuation_bytes >= retained,
            "{kind}: charged {} bytes but retained estimate is {retained}",
            session.continuation_bytes
        );
    }
}

#[test]
fn continuation_budget_accepts_exact_limit_and_rejects_one_byte_more() {
    for kind in ["small", "schema", "fragments"] {
        let (request, reply) = fixture(kind);
        let prepared = prepare_request(&request).unwrap();
        let mut calibrated = DiscoverySession::new(&request, true).unwrap().unwrap();
        assert!(calibrated.advance(&reply, &mut prepared.clone()).unwrap());
        let cost = calibrated.continuation_bytes;
        assert!(cost > 0 && cost < MAX_CONTINUATION_BYTES);
        for excess in [0, 1] {
            let mut session = DiscoverySession::new(&request, true).unwrap().unwrap();
            let prior_bytes = MAX_CONTINUATION_BYTES - cost + excess;
            session.continuation_bytes = prior_bytes;
            let mut upstream = prepared.clone();
            let result = session.advance(&reply, &mut upstream);
            if excess == 0 {
                assert!(result.unwrap());
                assert_eq!(session.continuation_bytes, MAX_CONTINUATION_BYTES);
            } else {
                assert_eq!(
                    result.unwrap_err().parameter(),
                    Some("output.relay.catalog.bytes")
                );
                assert_eq!(upstream, prepared);
                assert_eq!(session.continuation_bytes, prior_bytes);
                assert_eq!(session.rounds, 0);
                assert_eq!(session.rendered_schema_bytes, Some(0));
                assert!(session.loaded.is_empty());
                assert!(session.lookup_contracts.is_empty());
                assert!(session.call_ids.is_empty());
                assert!(session.prior_messages.is_empty());
            }
        }
    }
}

#[test]
#[ignore = "local BPS continuation CPU probe; not provider latency or live memory measurement"]
fn measure_bps_discovery_continuation() {
    use std::{hint::black_box, time::Instant};

    for kind in ["small", "schema", "fragments"] {
        let (request, reply) = fixture(kind);
        let prepared = prepare_request(&request).unwrap();
        let mut samples = Vec::new();
        let mut charged = 0;
        for trial in 0..18 {
            let mut session = DiscoverySession::new(&request, true).unwrap().unwrap();
            let mut upstream = prepared.clone();
            let started = Instant::now();
            assert!(
                session
                    .advance(black_box(&reply), black_box(&mut upstream))
                    .unwrap()
            );
            let elapsed = started.elapsed().as_nanos();
            charged = session.continuation_bytes;
            black_box((&session, &upstream));
            if trial >= 3 {
                samples.push(elapsed);
            }
        }
        samples.sort_unstable();
        println!(
            "bps-discovery-continuation kind={kind} trials=15 median_ns={} p90_ns={} retained_estimate={charged}",
            samples[7], samples[13]
        );
    }
}
