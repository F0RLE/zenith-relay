use super::*;
use serde_json::json;

#[test]
fn basis_points_accepts_responses_lite_and_preserves_serial_tool_contract() {
    let request = json!({
        "model": "test-model",
        "input": "Find a record",
        "parallel_tool_calls": true,
        "tools": [{
            "type": "function",
            "name": "lookup",
            "description": "Look up a record",
            "parameters": {
                "type": "object",
                "properties": {"query": {"type": "string"}},
                "required": ["query"]
            }
        }]
    });
    let service_tier_policy = ServiceTierPolicy::pool_owned(&request);
    let mut last_adapter_error = None;

    assert!(reject_account_basis_points(
        &request,
        &service_tier_policy,
        DefaultServiceTier::Standard,
        AccountEndpoint::Wake,
        &mut last_adapter_error,
    )
    .is_none());
    assert!(last_adapter_error.is_none());

    let normalized = match prepare_account_upstream_body(
        &request,
        "test-model",
        false,
        AccountEndpoint::Wake,
        true,
        true,
    ) {
        Ok(body) => body,
        Err(_) => panic!("valid Responses Lite request should normalize"),
    };
    assert_eq!(normalized["parallel_tool_calls"], false);

    let basis_points = match rewrite_account_basis_points_body(
        &normalized,
        "test-request",
        &mut last_adapter_error,
    ) {
        Ok(body) => body,
        Err(_) => panic!("valid tool catalog should translate for Basis Points"),
    };
    assert!(basis_points.get("tools").is_none());
    let developer_instructions = basis_points["input"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["role"] == "developer")
        .filter_map(|item| item.pointer("/content/0/text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(developer_instructions.contains("run_officejs"));
    assert!(developer_instructions.contains("lookup"));
    assert!(developer_instructions.contains("Invoke at most one client tool"));
}
