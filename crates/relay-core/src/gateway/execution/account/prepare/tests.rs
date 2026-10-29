use super::*;
use serde_json::json;

#[test]
fn bps_account_borrows_history_until_a_route_rewrite_is_needed() {
    let request = json!({"model":"synthetic", "input":"Synthetic history. ".repeat(65536),
        "context_management":[], "metadata":{"synthetic":"preserve"}});
    for rewrite in [false, true] {
        let body = match prepare_account_upstream_body(
            &request,
            "synthetic",
            rewrite,
            AccountEndpoint::Wake,
            true,
            true,
        ) {
            Ok(body) => body,
            Err(_) => panic!("valid BPS request was rejected"),
        };
        assert!(matches!(body, Cow::Borrowed(_)));
        assert!(std::ptr::eq(
            body["input"].as_str().unwrap().as_ptr(),
            request["input"].as_str().unwrap().as_ptr()
        ));
        let wire = super::super::super::basis_points::prepare_request(&body).unwrap();
        assert!(wire.get("context_management").is_none());
        assert_eq!(request["metadata"]["synthetic"], "preserve");
    }
    let mut body = match prepare_account_upstream_body(
        &request,
        "other",
        true,
        AccountEndpoint::Wake,
        false,
        true,
    ) {
        Ok(body) => body,
        Err(_) => panic!("valid model rewrite was rejected"),
    };
    assert!(matches!(body, Cow::Owned(_)));
    assert_eq!(body["model"], "other");
    body.to_mut()["input"] =
        json!("<environment_context><timezone>UTC</timezone></environment_context>");
    crate::request_timezone::rewrite_request_timezone(body.to_mut(), "Europe/Minsk");
    assert!(body["input"].as_str().unwrap().contains("Europe/Minsk"));
    assert_eq!(request["model"], "synthetic");
    assert!(
        request["input"]
            .as_str()
            .unwrap()
            .starts_with("Synthetic history.")
    );
}

#[test]
fn basis_points_accepts_lite_items_without_inheriting_serial_transport_policy() {
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

    assert!(
        reject_account_basis_points(
            &request,
            &service_tier_policy,
            DefaultServiceTier::Standard,
            AccountEndpoint::Wake,
            &mut last_adapter_error,
        )
        .is_none()
    );
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
    assert_eq!(normalized["parallel_tool_calls"], true);

    let mut schema_validation_ms = None;
    let basis_points = match rewrite_account_basis_points_body(
        &normalized,
        "test-request",
        &mut last_adapter_error,
        &mut schema_validation_ms,
        true,
        &mut super::super::super::basis_points::RequestCatalog::default(),
    ) {
        Ok(body) => body,
        Err(_) => panic!("valid tool catalog should translate for Basis Points"),
    };
    assert!(basis_points.get("tools").is_none());
    assert!(schema_validation_ms.is_some());
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
    assert!(!developer_instructions.contains("Invoke at most one client tool"));
}

#[test]
fn native_lite_rejects_explicit_parallel_calls_without_rewriting_the_request() {
    let request = json!({"model":"synthetic", "input":"hello", "parallel_tool_calls":true});
    assert!(matches!(
        prepare_account_upstream_body(
            &request,
            "synthetic",
            false,
            AccountEndpoint::Wake,
            true,
            false
        ),
        Err(AccountPrepare::Respond(_))
    ));
    assert_eq!(request["parallel_tool_calls"], true);
    let full = prepare_account_upstream_body(
        &request,
        "synthetic",
        false,
        AccountEndpoint::Wake,
        false,
        false,
    );
    assert!(matches!(full, Ok(ref body) if body["parallel_tool_calls"] == true));
}
