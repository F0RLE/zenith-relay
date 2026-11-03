use super::messages_tool_search::try_prepare_discovery;
use super::*;

pub(super) fn fixture() -> Value {
    json!({"model":"test","max_tokens":64,"tools":[
        {"name":"old","input_schema":{"type":"object"}},
        {"name":"later","defer_loading":true,"input_schema":{"type":"object"}}
    ],"messages":[
        {"role":"user","content":"Start"},
        {"role":"assistant","content":[{"type":"tool_use","id":"old_call","name":"old","input":{}}]},
        {"role":"user","content":[{"type":"tool_result","tool_use_id":"old_call","content":"synthetic result"},{"type":"text","text":"Change tools"}]},
        {"role":"system","content":[
            {"type":"text","text":"New turn policy"},
            {"type":"tool_removal","tool":{"type":"tool_reference","name":"old"}},
            {"type":"tool_addition","tool":{"type":"tool_reference","name":"later"}},
            {"type":"tool_addition","tool":{"type":"tool_definition","definition":{
                "name":"new","strict":true,"input_schema":{"type":"object","properties":{"value":{"type":"string"}},"required":["value"],"additionalProperties":false}
            }}}
        ]}
    ]})
}

#[test]
fn inline_tools_preserve_history_order_and_offer_only_the_current_exact_catalog() {
    let request = fixture();
    for upstream in [WireApi::Responses, WireApi::ChatCompletions] {
        let prepared = try_prepare_discovery(upstream, &request, false).unwrap();
        let decoded = decode::request(upstream, prepared.upstream_body()).unwrap();
        assert_eq!(
            decoded
                .tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            ["later", "new"]
        );
        assert_eq!(decoded.tools[1].strict, Some(true));
        assert_eq!(decoded.tools[1].parameters["additionalProperties"], false);
        assert_eq!(decoded.messages[1].role, Role::Assistant);
        let (system_index, system) = decoded
            .messages
            .iter()
            .enumerate()
            .find(|(_, message)| message.role == Role::System)
            .unwrap();
        // Chat separates user text from tool results; the system update must
        // follow both, rather than depend on a protocol-specific message index.
        assert!(system_index >= 3);
        assert_eq!(system.blocks[0], Block::Text("New turn policy".into()));
        assert!(matches!(&system.blocks[1], Block::Text(text) if text.contains("tool_removal")));
    }
    assert!(try_prepare_discovery(WireApi::Gemini, &request, false).is_err());
    let native = prepare(WireApi::Messages, WireApi::Messages, &request, false);
    assert!(native.is_passthrough());
    assert_eq!(native.upstream_body()["messages"], request["messages"]);
    assert_eq!(native.upstream_body()["tools"], request["tools"]);
}

#[test]
fn inline_tools_reject_bad_references_placement_and_forced_removed_tools() {
    let original = fixture();
    for change in ["unknown", "placement", "forced", "schema"] {
        let mut request = original.clone();
        match change {
            "unknown" => request["messages"][3]["content"][1]["tool"]["name"] = "missing".into(),
            "placement" => request["messages"][3]["role"] = "user".into(),
            "forced" => request["tool_choice"] = json!({"type":"tool","name":"old"}),
            "schema" => {
                request["messages"][3]["content"][3]["tool"]["definition"]["input_schema"] =
                    json!(7)
            }
            _ => unreachable!(),
        }
        assert!(
            try_prepare_discovery(WireApi::Responses, &request, false).is_err(),
            "{change}"
        );
    }
}

#[test]
fn inline_redefinition_and_reoffering_do_not_rewrite_earlier_turns() {
    let mut request = fixture();
    request["messages"][3]["content"].as_array_mut().unwrap().extend([
        json!({"type":"tool_addition","tool":{"type":"tool_definition","definition":{
            "name":"new","input_schema":{"type":"object","properties":{"changed":{"type":"integer"}},"additionalProperties":false}
        }}}),
        json!({"type":"tool_addition","tool":{"type":"tool_reference","name":"old"}}),
    ]);
    let before = request.clone();
    let prepared = try_prepare_discovery(WireApi::Responses, &request, true).unwrap();
    let decoded = decode::request(WireApi::Responses, prepared.upstream_body()).unwrap();
    assert_eq!(decoded.tools.len(), 3);
    assert!(
        decoded
            .tools
            .iter()
            .find(|tool| tool.name == "new")
            .unwrap()
            .parameters["properties"]
            .get("changed")
            .is_some()
    );
    assert_eq!(request, before);
}
