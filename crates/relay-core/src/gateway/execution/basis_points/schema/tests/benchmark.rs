use super::*;

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
