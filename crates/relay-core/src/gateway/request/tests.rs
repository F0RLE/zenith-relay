use super::*;
use crate::{
    DefaultServiceTier, GatewayRuntimeOptions, LocalGatewayKey, ProviderSource, RuntimeLocalKey,
    RuntimeSource,
};
use axum::http::{HeaderMap, HeaderValue};
mod background_turns;
mod catalog_rows;
mod request_shape;
mod tool_controls;
mod tool_repair;

fn automatic_tool_policy_test_runtime() -> GatewayRuntime {
    let runtime = capability_test_runtime(&["synthetic"], GatewayRuntimeOptions::default());
    runtime
        .set_tool_policy(crate::ToolPolicy {
            mode: crate::ToolPolicyMode::Automatic,
        })
        .unwrap();
    runtime
}

fn two_function_tools() -> Value {
    json!({
        "tools": [
            {"type":"function","name":"lookup"},
            {"type":"function","name":"update"}
        ]
    })
}

fn capability_test_runtime(models: &[&str], options: GatewayRuntimeOptions) -> GatewayRuntime {
    GatewayRuntime::from_pool(
        vec![RuntimeSource::unrestricted(ProviderSource {
            id: "source".into(),
            name: "source".into(),
            base_url: "https://example.test/v1".into(),
            api_key: "upstream-secret".into(),
            wire_api: WireApi::Responses,
            models: models.iter().map(|id| (*id).to_string()).collect(),
        })],
        vec![RuntimeLocalKey::unrestricted(LocalGatewayKey {
            id: "key".into(),
            secret: "secret".into(),
        })],
        options,
        Arc::new(|_| {}),
    )
    .unwrap()
}
