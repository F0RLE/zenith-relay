use super::*;
use serde_json::{json, Value};

#[test]
fn sizing_matches_buffered_json_and_enforces_exact_byte_limits() {
    for value in [
        Value::Null,
        json!({"text":"Synthetic \"quotes\", \\paths,\r\n\tкириллица\u{0}","array":[true,1.25,null]}),
        serde_json::from_str::<Value>(r#"{"large":90071992547409931234567,"small":1e-100}"#)
            .unwrap(),
        json!({"synthetic_image":"abc/xyz+0123=".repeat(100_000)}),
    ] {
        let expected = serde_json::to_vec(&value).unwrap().len();
        assert_eq!(serialized_len(&value, usize::MAX), Some(expected));
        assert_eq!(serialized_len(&value, expected), Some(expected));
        assert_eq!(serialized_len(&value, expected - 1), None);
    }
    assert_eq!(serialized_len(&Value::Null, 0), None);
}

#[test]
fn serialization_failure_does_not_return_a_partial_size() {
    struct Invalid;
    impl Serialize for Invalid {
        fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom("synthetic serialization failure"))
        }
    }
    assert_eq!(serialized_len(&Invalid, usize::MAX), None);
}

#[test]
fn oversized_writes_leave_the_counter_within_its_limit() {
    let mut writer = SizeWriter {
        bytes: 0,
        max_bytes: 3,
    };
    assert_eq!(writer.write(b"ab").unwrap(), 2);
    assert!(writer.write(b"cd").is_err());
    assert_eq!(writer.bytes, 2);
    assert_eq!(writer.write(b"c").unwrap(), 1);
    assert_eq!(writer.bytes, 3);
}

#[test]
#[ignore = "manual release-profile throughput probe; no hardware-dependent pass threshold"]
fn sizing_throughput_probe() {
    use std::{hint::black_box, time::Instant};

    for (name, value, iterations) in [
        ("small", json!({"text":"synthetic".repeat(64)}), 10_000usize),
        (
            "catalog",
            json!({"tools":(0..64).map(|index| json!({
                "name":format!("synthetic_{index}"),"parameters":{"type":"object",
                "properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false}
            })).collect::<Vec<_>>()}),
            1_000,
        ),
        (
            "large",
            json!({"synthetic_history":"abc/xyz+0123=".repeat(100_000)}),
            100,
        ),
    ] {
        let expected = serde_json::to_vec(&value).unwrap().len();
        let mut buffered = Vec::new();
        let mut counted = Vec::new();
        for round in 0..6 {
            for streaming in if round % 2 == 0 {
                [false, true]
            } else {
                [true, false]
            } {
                let started = Instant::now();
                for _ in 0..iterations {
                    let size = if streaming {
                        serialized_len(black_box(&value), expected)
                    } else {
                        Some(serde_json::to_vec(black_box(&value)).unwrap().len())
                    };
                    assert_eq!(black_box(size), Some(expected));
                }
                let ns_per_call = started.elapsed().as_nanos() / iterations as u128;
                if streaming {
                    counted.push(ns_per_call);
                } else {
                    buffered.push(ns_per_call);
                }
            }
        }
        buffered.sort_unstable();
        counted.sort_unstable();
        eprintln!("sizing {name}: encoded_bytes={expected}, buffered_median_ns={}, counted_median_ns={}; local CPU probe, not provider latency or process RAM", buffered[3], counted[3]);
    }
}
