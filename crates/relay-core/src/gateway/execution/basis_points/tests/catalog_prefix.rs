use super::*;

fn tool(name: &str, minimum: i64) -> Value {
    json!({"type":"function","name":name,"parameters":{"type":"object",
        "properties":{"value":{"type":"integer","minimum":minimum}},"required":["value"],"additionalProperties":false}})
}

#[test]
fn changing_root_overrides_preserves_explicit_catalog_history() {
    let mut request = json!({"model":"synthetic","tools":[tool("first",1)],
        "input":[{"type":"additional_tools","tools":[tool("historical",1)]},
            {"role":"user","content":"Synthetic history"},{"role":"assistant","content":"Synthetic answer"}]});
    let baseline = prepare_request(&request).unwrap();
    request["tools"] = json!([tool("second", 9)]);
    let changed = prepare_request(&request).unwrap();
    assert_eq!(
        &baseline["input"].as_array().unwrap()[..4],
        &changed["input"].as_array().unwrap()[..4]
    );
    let current = changed["input"][4]["content"][0]["text"].as_str().unwrap();
    assert!(current.contains("second"));
    assert!(!current.contains("first"));
    let selected = super::super::catalog::client_tools(&request).unwrap();
    assert!(super::super::catalog::tool_spec(&selected, "first").is_none());
}

#[test]
fn empty_current_catalog_keeps_history_and_appends_a_no_tools_guard() {
    for mode in ["compact", "discovery", "full", "auto"] {
        let request = json!({"model":"synthetic","relay_bps_tool_catalog":mode,"tools":[],
            "input":[{"type":"additional_tools","tools":[tool("historical",1)]},
                {"role":"user","content":"Synthetic continuation"}]});
        let prepared = prepare_request(&request).unwrap();
        assert!(prompt_text(&prepared).contains("historical"));
        let input = prepared["input"].as_array().unwrap();
        let guard = input.last().unwrap()["content"][0]["text"]
            .as_str()
            .unwrap();
        assert!(guard.contains("No client tools are permitted this turn"));
        let mut forced = request;
        forced["tool_choice"] = json!({"type":"function","name":"historical"});
        assert_eq!(
            prepare_request(&forced).unwrap_err().parameter(),
            Some("tool_choice")
        );
    }
}

#[test]
fn appended_declarations_keep_their_history_position_and_current_schema_wins() {
    for mode in ["compact", "discovery", "full", "auto"] {
        for declaration in ["additional_tools", "tool_search_output"] {
            let mut request = json!({"model":"synthetic","relay_bps_tool_catalog":mode,"input":[
                {"type":declaration,"tools":[tool("first",1)]},
                {"role":"user","content":"Synthetic history"}]});
            let baseline = prepare_request(&request).unwrap();
            request["input"]
                .as_array_mut()
                .unwrap()
                .push(json!({"type":declaration,"tools":[tool("first",9),tool("second",1)]}));
            let changed = prepare_request(&request).unwrap();
            assert_eq!(
                &baseline["input"].as_array().unwrap()[..3],
                &changed["input"].as_array().unwrap()[..3],
                "{mode}/{declaration}"
            );
            assert!(
                changed["input"][3]["content"][0]["text"]
                    .as_str()
                    .unwrap()
                    .contains("second")
            );
            let tools = super::super::catalog::client_tools(&request).unwrap();
            let first = super::super::catalog::tool_spec(&tools, "first").unwrap();
            assert_eq!(
                first.spec["parameters"]["properties"]["value"]["minimum"],
                9
            );
            request["tool_choice"] = json!("none");
            assert!(super::super::catalog::selected_tools(&request, &tools).is_empty());
        }
    }
}
