use super::*;

#[test]
#[ignore = "manual synthetic BPS preparation and stable-prefix measurement"]
fn measure_bps_turn_prefix_reuse() {
    use std::{hint::black_box, time::Instant};

    for mode in ["compact", "discovery"] {
        let mut request = json!({
            "model":"synthetic-model", "relay_bps_tool_catalog":mode,
            "input":[
                {"role":"user","content":"Synthetic stable history. ".repeat(1024)},
                {"role":"assistant","content":"Synthetic previous answer."},
                {"role":"user","content":"Synthetic next turn."}
            ],
            "tools":(0..48).map(|index| json!({
                "type":"function","name":format!("tool_{index:02}"),
                "description":"Synthetic tool contract.",
                "parameters":{"type":"object","properties":{"text":{"type":"string"}},
                    "required":["text"],"additionalProperties":false}
            })).collect::<Vec<_>>()
        });
        let baseline = prepare_request(&request).unwrap();
        let prefix = baseline["input"].as_array().unwrap();
        let choices = [
            ("auto", json!("auto")),
            ("none", json!("none")),
            ("named", json!({"type":"function","name":"tool_00"})),
            (
                "allowed",
                json!({"type":"allowed_tools","mode":"auto",
                "tools":[{"type":"function","name":"tool_00"}]}),
            ),
        ];
        for (choice_name, choice) in choices {
            request["tool_choice"] = choice;
            let prepared = prepare_request(&request).unwrap();
            let shared = prefix
                .iter()
                .zip(prepared["input"].as_array().unwrap())
                .take_while(|(before, after)| before == after)
                .count();
            let shared_bytes = if shared == 0 {
                0
            } else {
                serde_json::to_vec(&prefix[..shared]).unwrap().len()
            };
            for _ in 0..5 {
                black_box(prepare_request(black_box(&request)).unwrap());
            }
            let mut samples = Vec::new();
            for _ in 0..7 {
                let started = Instant::now();
                for _ in 0..25 {
                    black_box(prepare_request(black_box(&request)).unwrap());
                }
                samples.push(started.elapsed().as_nanos() / 25);
            }
            samples.sort_unstable();
            eprintln!(
                "bps mode={mode} choice={choice_name} warmup=5 trials=7 iterations=25 shared_items={shared}/{} shared_bytes={shared_bytes} median_ns={} min_ns={} max_ns={}",
                prefix.len(), samples[3], samples[0], samples[6]
            );
        }
    }
}
