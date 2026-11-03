use super::super::super::{TRANSPORT_TOOL, response::parse_transport_envelope};
use serde_json::{Map, Value, json};

fn fixture(
    bytes: usize,
    kind: &str,
    stringified: bool,
) -> (Map<String, Value>, Map<String, Value>) {
    let raw = "  synthetic \\\"quoted\\\" C:\\synthetic\\file\r\nстрока\t\n".repeat(bytes / 32 + 1);
    let inner = json!({"tool":"synthetic.exec","args":{"cmd":raw,"count":3}});
    let (mut outer, expected) = match kind {
        "referenced" => (
            json!({"references":["synthetic.exec"],"code":raw}),
            json!({"tool":"synthetic.exec","args":raw}),
        ),
        "legacy" => (json!({"code":inner.to_string()}), inner),
        "nested" => (
            json!({"code":{"name":TRANSPORT_TOOL,
                "code":{"name":TRANSPORT_TOOL,"code":inner}}}),
            inner,
        ),
        _ => unreachable!(),
    };
    if stringified {
        outer = Value::String(outer.to_string());
    }
    let item = json!({"name":TRANSPORT_TOOL,"arguments":outer});
    let Value::Object(item) = item else {
        unreachable!()
    };
    let Value::Object(expected) = expected else {
        unreachable!()
    };
    (item, expected)
}

#[test]
fn bps_envelope_preserves_payload_and_input_across_representations() {
    for kind in ["referenced", "legacy", "nested"] {
        for stringified in [false, true] {
            let (item, expected) = fixture(4096, kind, stringified);
            let original = item.clone();
            let (parsed, direct) = parse_transport_envelope(&item).unwrap();
            assert!(!direct);
            assert_eq!(
                serde_json::to_vec(&parsed).unwrap(),
                serde_json::to_vec(&expected).unwrap()
            );
            assert_eq!(item, original);
        }
    }
}

#[test]
#[ignore = "manual local BPS envelope decoding benchmark; no provider or TTFT measurement"]
fn measure_bps_envelope_decoding() {
    use std::{hint::black_box, time::Instant};

    for bytes in [4096, 1024 * 1024] {
        for kind in ["referenced", "legacy", "nested"] {
            for stringified in [false, true] {
                let (item, expected) = fixture(bytes, kind, stringified);
                let wire_bytes = serde_json::to_vec(&item).unwrap().len();
                let mut samples = Vec::new();
                for trial in 0..16 {
                    let started = Instant::now();
                    let (parsed, direct) = parse_transport_envelope(black_box(&item)).unwrap();
                    let elapsed = started.elapsed().as_nanos();
                    assert!(!direct);
                    assert_eq!(parsed, expected);
                    if trial > 0 {
                        samples.push(elapsed);
                    }
                }
                samples.sort_unstable();
                eprintln!(
                    "bps-envelope kind={kind} stringified={stringified} wire_bytes={wire_bytes} trials=15 median_ns={} p90_ns={}",
                    samples[7], samples[13]
                );
            }
        }
    }
}
