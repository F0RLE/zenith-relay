use super::*;

// The pre-optimization membership scan, retained only as a measurement reference.
fn linear_required_properties<'a>(
    properties: Option<&'a Map<String, Value>>,
    required: Option<&'a [Value]>,
) -> Option<impl Iterator<Item = (&'a String, bool)>> {
    let mut names = properties
        .map(|properties| properties.keys().collect::<Vec<_>>())
        .unwrap_or_default();
    names.sort();
    if required.is_some_and(|required| {
        required.iter().any(|name| {
            name.as_str()
                .is_none_or(|name| !properties.is_some_and(|p| p.contains_key(name)))
        })
    }) {
        return None;
    }
    Some(names.into_iter().map(move |name| {
        let is_required = required
            .is_some_and(|required| required.iter().any(|value| value.as_str() == Some(name)));
        (name, is_required)
    }))
}

#[test]
#[ignore = "paired synthetic required-field lookup measurement; not request latency"]
fn measure_required_property_lookup_paired() {
    use std::{hint::black_box, time::Instant};

    for width in [8, 64, 256] {
        let properties = (0..width)
            .rev()
            .map(|index| (format!("field_{index:03}"), json!({"type":"string"})))
            .collect::<Map<_, _>>();
        for count in [1, width / 2, width] {
            let required = (0..count)
                .rev()
                .map(|index| json!(format!("field_{index:03}")))
                .collect::<Vec<_>>();
            let expected = linear_required_properties(Some(&properties), Some(&required))
                .unwrap()
                .collect::<Vec<_>>();
            assert_eq!(
                required_properties(Some(&properties), Some(&required)).unwrap(),
                expected
            );
            let measure = |optimized| {
                let started = Instant::now();
                for _ in 0..1000 {
                    let properties = Some(black_box(&properties));
                    let required = Some(black_box(required.as_slice()));
                    if optimized {
                        for entry in required_properties(properties, required).unwrap() {
                            black_box(entry);
                        }
                    } else {
                        for entry in linear_required_properties(properties, required).unwrap() {
                            black_box(entry);
                        }
                    }
                }
                started.elapsed()
            };
            let mut before = Vec::new();
            let mut after = Vec::new();
            for round in 0..9 {
                if round % 2 == 0 {
                    before.push(measure(false));
                    after.push(measure(true));
                } else {
                    after.push(measure(true));
                    before.push(measure(false));
                }
            }
            before.sort_unstable();
            after.sort_unstable();
            eprintln!(
                "required-lookup fields={width} required={count} iterations=1000 before_us={} after_us={}",
                before[4].as_micros(),
                after[4].as_micros()
            );
        }
    }
}

#[test]
#[ignore = "manual synthetic wide required-schema rendering measurement"]
fn measure_wide_required_schema_rendering() {
    use sha2::{Digest, Sha256};
    use std::{hint::black_box, time::Instant};

    for width in [8, 64, 256] {
        for required_count in [1, width / 2, width] {
            let schema = json!({
                "type":"object",
                "properties":(0..width).rev().map(|index| (
                    format!("field_{index:03}"),
                    json!({"type":"string","description":"Synthetic parameter","minLength":1})
                )).collect::<serde_json::Map<_, _>>(),
                "required":(0..required_count).rev()
                    .map(|index| format!("field_{index:03}")).collect::<Vec<_>>(),
                "additionalProperties":false
            });
            let expected = prompt_schema(&schema);
            let mut samples = Vec::new();
            for _ in 0..9 {
                let started = Instant::now();
                for _ in 0..100 {
                    assert_eq!(prompt_schema(black_box(&schema)), expected);
                }
                samples.push(started.elapsed());
            }
            samples.sort_unstable();
            eprintln!(
                "wide-schema fields={width} required={required_count} compact_bytes={} iterations=100 median_us={} sha256={}",
                expected.len(),
                samples[4].as_micros(),
                hex::encode(Sha256::digest(expected.as_bytes()))
            );
        }
    }
}

#[test]
#[ignore = "manual synthetic nested schema rendering measurement"]
fn measure_nested_schema_rendering() {
    use std::{hint::black_box, time::Instant};

    for depth in [1, 4, 8] {
        let mut schema = json!({
            "type":"string","description":"Synthetic parameter","minLength":1,"maxLength":4096
        });
        for _ in 0..depth {
            schema = json!({
                "type":"object","properties":{"left":schema.clone(),"right":schema},
                "required":["left"],"additionalProperties":false,"description":"Synthetic node"
            });
        }
        let expected = prompt_schema(&schema);
        let mut samples = Vec::new();
        for _ in 0..5 {
            let started = Instant::now();
            for _ in 0..10 {
                assert_eq!(prompt_schema(black_box(&schema)), expected);
            }
            samples.push(started.elapsed());
        }
        samples.sort_unstable();
        eprintln!(
            "schema depth={depth}, bytes={}, iterations=10: median={:?}",
            schema.to_string().len(),
            samples[2]
        );
    }
}
