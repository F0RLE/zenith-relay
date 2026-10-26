use super::*;
use crate::scheduler::rotation::SharedRequestBudget;

fn request() -> Value {
    json!({"model":"synthetic","tools":(0..30).map(|i| json!({"type":"function","name":format!("tool_{i}"),
        "description":"Synthetic searchable description. ".repeat(40),
        "parameters":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false}
    })).collect::<Vec<_>>(),"input":[{"role":"user","content":"Synthetic input"}]})
}

fn prepare(request: &Value, budget: usize) -> (Value, Option<DiscoverySession>) {
    let mut upstream = request.clone();
    crate::providers::chatgpt::prepare_siwc_request(&mut upstream).unwrap();
    prepare_dispatch(
        request,
        &upstream,
        &SharedRequestBudget::for_incoming_request(budget),
        &mut ToolUseDiagnostics::default(),
    )
    .unwrap()
}

fn lookup_response(round: usize, arguments: Value) -> Value {
    json!({"id":format!("resp_{round}"),"status":"completed","output":[{
        "type":"function_call","id":format!("fc_{round}"),"call_id":format!("call_{round}"),
        "name":NATIVE_LOOKUP,"arguments":arguments.to_string(),"status":"completed"
    }]})
}

#[test]
fn native_catalog_never_changes_forced_or_restricted_tool_choice() {
    let request = request();
    assert!(eligible(&request));
    for choice in [
        json!("none"),
        json!("required"),
        json!({"type":"function","name":"tool_0"}),
        json!({"type":"allowed_tools","mode":"auto","tools":[{"type":"function","name":"tool_0"}]}),
    ] {
        let mut request = request.clone();
        request["tool_choice"] = choice;
        assert!(!eligible(&request));
    }
    let mut collision = request.clone();
    collision["tools"][0]["name"] = json!(NATIVE_LOOKUP);
    assert!(!eligible(&collision));
    let mut small = request;
    small["tools"].as_array_mut().unwrap().truncate(2);
    assert!(!eligible(&small));
}

#[test]
fn native_catalog_falls_back_without_budget_and_keeps_warmup_out_of_discovery() {
    let request = request();
    let (body, session) = prepare(&request, 1);
    assert!(session.is_none());
    assert_eq!(client_tools(&body).unwrap().len(), 30);
    assert!(!body.to_string().contains(NATIVE_LOOKUP));
    let mut warmup = request;
    warmup["generate"] = json!(false);
    let (body, session) = prepare(&warmup, 8);
    assert!(session.is_none());
    assert_eq!(body["prompt_cache_options"]["prewarm"], true);
    assert!(body.get("generate").is_none());
    assert_eq!(client_tools(&body).unwrap().len(), 1);
}

#[test]
fn native_catalog_recovers_bad_lookups_but_bounds_rounds_and_bytes() {
    let (mut body, session) = prepare(&request(), 16);
    let mut session = session.unwrap();
    for round in 0..8 {
        let reply = lookup_response(round, json!({"names":["not_available"]}));
        assert!(session.advance(&reply, &mut body).unwrap());
        assert!(
            body["input"].as_array().unwrap().last().unwrap()["output"]
                .as_str()
                .unwrap()
                .contains("invalid_catalog_lookup")
        );
    }
    assert!(
        session
            .advance(&lookup_response(8, json!({"names":["tool_0"]})), &mut body)
            .is_err()
    );
    let (mut body, session) = prepare(&request(), 16);
    let mut session = session.unwrap();
    session.continuation_bytes = MAX_CONTINUATION_BYTES;
    assert!(
        session
            .advance(&lookup_response(0, json!({"names":["tool_0"]})), &mut body)
            .is_err()
    );
}

#[test]
fn native_catalog_preserves_prefix_media_and_loads_history_before_calls() {
    let mut request = request();
    request["input"].as_array_mut().unwrap().extend([
        json!({"type":"function_call","name":"tool_2","call_id":"historical","arguments":"{\"path\":\"synthetic\"}"}),
        json!({"type":"function_call_output","call_id":"historical","output":"synthetic"}),
        json!({"role":"user","content":[{"type":"input_image","image_url":"data:image/png;base64,c3ludGhldGlj"}]})
    ]);
    let (first, _) = prepare(&request, 8);
    let input = first["input"].as_array().unwrap();
    let call = input
        .iter()
        .position(|item| item["call_id"] == "historical")
        .unwrap();
    assert_eq!(input[call - 1]["tools"][0]["name"], "tool_2");
    assert_eq!(input.last(), request["input"].as_array().unwrap().last());
    request["input"]
        .as_array_mut()
        .unwrap()
        .push(json!({"role":"user","content":"Another synthetic turn"}));
    let (second, _) = prepare(&request, 8);
    assert_eq!(
        &second["input"].as_array().unwrap()[..input.len()],
        input.as_slice()
    );
}

#[test]
fn native_catalog_rejects_duplicate_call_ids_and_hides_unfinished_calls() {
    let (mut body, session) = prepare(&request(), 8);
    let mut session = session.unwrap();
    let reply = lookup_response(0, json!({"query":"tool_0","limit":1}));
    assert!(session.advance(&reply, &mut body).unwrap());
    assert!(session.advance(&reply, &mut body).is_err());
    let mut incomplete = lookup_response(1, json!({"names":["tool_1"]}));
    incomplete["status"] = json!("incomplete");
    assert!(!session.advance(&incomplete, &mut body).unwrap());
    session.finish(&mut incomplete);
    assert!(incomplete["output"].as_array().unwrap().is_empty());
}

#[test]
fn native_catalog_does_not_turn_an_empty_generation_into_a_success() {
    let (mut body, session) = prepare(&request(), 8);
    let mut session = session.unwrap();
    let empty = json!({"id":"resp_empty","status":"completed","output":[],
        "usage":{"input_tokens":24,"output_tokens":0}});
    assert!(session.advance(&empty, &mut body).is_err());
}
