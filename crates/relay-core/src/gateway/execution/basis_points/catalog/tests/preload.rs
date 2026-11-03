use super::*;
use crate::gateway::execution::basis_points::prepare::describe_tools;
use crate::gateway::execution::basis_points::presentation::{PRELOAD_MAX_BYTES, loaded_names};
use std::collections::BTreeSet;

// Test-only reference of eager selection before the rendering/sorting shortcut.
fn original_loaded_names(tools: &[&ClientTool], request: &Value) -> BTreeSet<String> {
    let available: BTreeSet<_> = tools.iter().map(|tool| tool.key()).collect();
    let mut loaded: BTreeSet<_> = request
        .get("input")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| {
            matches!(
                item["type"].as_str(),
                Some("function_call" | "custom_tool_call")
            )
        })
        .filter_map(|item| item.as_object())
        .map(client_tool_call_name)
        .filter(|name| available.contains(name))
        .collect();
    let core = |tool: &ClientTool| {
        tool.namespace.is_none()
            && matches!(
                tool.name.as_str(),
                "exec_command" | "apply_patch" | "write_stdin"
            )
    };
    let explicit = request["tool_choice"]["type"]
        .as_str()
        .is_some_and(|kind| matches!(kind, "function" | "custom" | "allowed_tools"));
    let mut ordered = tools.to_vec();
    ordered.sort_by_cached_key(|tool| (!core(tool), tool.key()));
    let mut count = 0;
    let mut bytes = 0;
    for tool in ordered {
        let deferred = tool.spec.get("defer_loading").and_then(Value::as_bool);
        if deferred == Some(true) || !(deferred == Some(false) || core(tool) || explicit) {
            continue;
        }
        let size = describe_tools(&[tool], false).len();
        if count < 8 && bytes + size <= PRELOAD_MAX_BYTES {
            loaded.insert(tool.key());
            bytes += size;
            count += 1;
        }
    }
    loaded
}

fn eager_request(count: usize) -> Value {
    json!({"tools":(0..count).rev().map(|index| json!({
        "type":"function","name":format!("a_eager_{index:04}"),"defer_loading":false,
        "parameters":{
            "type":"object",
            "properties":(0..8).map(|field| (
                format!("field_{field}"),
                json!({"type":"string","description":"Synthetic field.","minLength":1})
            )).collect::<Map<_, _>>(),
            "required":["field_0"],"additionalProperties":false
        }
    })).collect::<Vec<_>>()})
}

fn cold_tools(tools: &[ClientTool]) -> Vec<ClientTool> {
    tools
        .iter()
        .cloned()
        .map(|mut tool| {
            tool.presentation = Arc::default();
            tool
        })
        .collect()
}

fn compare_selection(request: &Value) -> BTreeSet<String> {
    let tools = selected_tools(request, &client_tools(request).unwrap());
    let original = cold_tools(&tools);
    let current = cold_tools(&tools);
    let expected = original_loaded_names(&original.iter().collect::<Vec<_>>(), request);
    let actual = loaded_names(&current.iter().collect::<Vec<_>>(), request);
    assert_eq!(actual, expected);
    let rendered = |tools: &[ClientTool], names: &BTreeSet<String>| {
        let mut selected = tools
            .iter()
            .filter(|tool| names.contains(&tool.key()))
            .collect::<Vec<_>>();
        selected.sort_by_cached_key(|tool| tool.key());
        describe_tools(&selected, false)
    };
    assert_eq!(rendered(&current, &actual), rendered(&original, &expected));
    actual
}

#[test]
fn preload_preserves_order_history_current_permissions_and_explicit_deferral() {
    for count in [0, 1, 8, 9, 24] {
        let request = eager_request(count);
        assert_eq!(compare_selection(&request).len(), count.min(8));
    }
    let mut request = eager_request(24);
    request["tools"][0]["defer_loading"] = json!(true);
    request["tools"].as_array_mut().unwrap().extend([
        json!({"type":"function","name":"exec_command"}),
        json!({"type":"function","name":"write_stdin"}),
        json!({"type":"custom","name":"apply_patch","defer_loading":true,
            "format":{"type":"grammar","syntax":"lark","definition":"start: \"synthetic\""}}),
        json!({"type":"namespace","name":"scoped","tools":[
            {"type":"function","name":"exec_command"}
        ]}),
    ]);
    request["input"] = json!([
        {"type":"function_call","name":"a_eager_0023"},
        {"type":"function_call","name":"a_eager_0001"},
        {"type":"custom_tool_call","name":"apply_patch"},
        {"type":"function_call","name":"deleted_tool"}
    ]);
    for choice in [
        json!("auto"),
        json!("none"),
        json!({"type":"function","name":"a_eager_0023"}),
        json!({"type":"custom","name":"apply_patch"}),
        json!({"type":"allowed_tools","mode":"auto","tools":[
            {"type":"function","name":"a_eager_0001"},
            {"type":"function","name":"scoped.exec_command"}
        ]}),
    ] {
        request["tool_choice"] = choice;
        let original = request.clone();
        let expected = compare_selection(&request);
        assert!(!expected.contains("deleted_tool"));
        assert_eq!(request, original);
        request["tools"].as_array_mut().unwrap().reverse();
        assert_eq!(compare_selection(&request), expected);
    }
}

#[test]
fn preload_leaves_candidates_beyond_the_count_limit_unrendered() {
    let request = eager_request(24);
    let tools = cold_tools(&client_tools(&request).unwrap());
    let loaded = loaded_names(&tools.iter().collect::<Vec<_>>(), &request);
    assert_eq!(loaded.len(), 8);
    for tool in &tools {
        assert_eq!(
            tool.presentation.get().is_some(),
            loaded.contains(&tool.key()),
            "{}",
            tool.key()
        );
    }
}

#[test]
fn preload_keeps_exact_byte_boundary_and_skips_oversized_candidates() {
    for extra in [0, 1] {
        let mut request = json!({"tools":[
            {"type":"function","name":"a_budget","defer_loading":false,"description":"x"},
            {"type":"function","name":"z_small","defer_loading":false}
        ]});
        let initial = client_tools(&request).unwrap()[0].description(false).len();
        request["tools"][0]["description"] =
            json!("x".repeat(PRELOAD_MAX_BYTES - initial + 1 + extra));
        assert_eq!(
            client_tools(&request).unwrap()[0].description(false).len(),
            PRELOAD_MAX_BYTES + extra
        );
        assert_eq!(
            compare_selection(&request),
            BTreeSet::from([if extra == 0 { "a_budget" } else { "z_small" }.to_string()])
        );
    }
}

#[test]
#[ignore = "paired synthetic eager catalog preparation; not provider latency or process RSS"]
fn measure_eager_preload_paired() {
    use sha2::{Digest, Sha256};
    use std::{hint::black_box, time::Instant};

    for count in [24, 256, 1024] {
        let request = eager_request(count);
        let template = client_tools(&request).unwrap();
        let expected = compare_selection(&request);
        for warm in [false, true] {
            let iterations = if warm { 10 } else { 1 };
            let measure = |current| {
                let tools = cold_tools(&template);
                if warm {
                    for tool in &tools {
                        black_box(tool.description(false));
                    }
                }
                let refs = tools.iter().collect::<Vec<_>>();
                let started = Instant::now();
                for _ in 0..iterations {
                    if current {
                        black_box(loaded_names(black_box(&refs), black_box(&request)));
                    } else {
                        black_box(original_loaded_names(black_box(&refs), black_box(&request)));
                    }
                }
                let elapsed = started.elapsed();
                let rendered = tools
                    .iter()
                    .filter(|tool| tool.presentation.get().is_some())
                    .count();
                (elapsed, rendered)
            };
            let mut before = Vec::new();
            let mut after = Vec::new();
            let mut rendered = (0, 0);
            for round in 0..9 {
                let (reference, current) = if round % 2 == 0 {
                    (measure(false), measure(true))
                } else {
                    let current = measure(true);
                    (measure(false), current)
                };
                before.push(reference.0);
                after.push(current.0);
                rendered = (reference.1, current.1);
            }
            before.sort_unstable();
            after.sort_unstable();
            eprintln!(
                "eager-preload tools={count} warm={warm} iterations={iterations} reference_us={} current_us={} reference_rendered={} current_rendered={} loaded={} result_sha256={}",
                before[4].as_micros(),
                after[4].as_micros(),
                rendered.0,
                rendered.1,
                expected.len(),
                hex::encode(Sha256::digest(serde_json::to_vec(&expected).unwrap()))
            );
        }
    }
}
