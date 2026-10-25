use serde_json::{json, Value};
use zenith_relay_core::ToolUseDiagnostics;

pub(super) fn document(tools: &ToolUseDiagnostics) -> Value {
    let catalog = tools.basis_points_catalog.as_ref().map(|catalog| json!({
        "representation": "prompt_text_not_native_tools_array",
        "mode": (["full", "compact", "discovery"].contains(&catalog.mode.as_str()).then_some(catalog.mode.as_str())),
        "fullSchemaBytes": catalog.full_schema_bytes, "renderedSchemaBytes": catalog.rendered_schema_bytes,
        "catalogPromptBytes": catalog.catalog_prompt_bytes, "loadedToolCount": catalog.loaded_tool_count,
        "discoveryRounds": catalog.discovery_rounds, "discoveryLatencyMs": catalog.discovery_latency_ms,
        "discoveryInputTokens": catalog.discovery_input_tokens, "discoveryOutputTokens": catalog.discovery_output_tokens,
    }));
    let policy_catalog = json!({
        "stage": "after_tool_policy_before_transport_encoding",
        "toolCount": tools.forwarded_tool_count,
        "schemaBytes": tools.forwarded_schema_bytes,
        "filteredToolCount": tools.filtered_tool_count,
    });
    json!({
        "clientToolCount": tools.client_tool_count, "policyCatalog": policy_catalog,
        "toolCallCount": tools.tool_call_count, "upstreamToolCallCount": tools.upstream_tool_call_count,
        "toolCallsReadyMs": tools.tool_calls_ready_ms, "terminalOutput": tools.terminal_output,
        "clientSchemaBytes": tools.client_schema_bytes,
        "clientContext": tools.client_context, "upstreamContext": tools.upstream_context,
        "toolChoice": tools.tool_choice, "requestedParallelToolCalls": tools.requested_parallel_tool_calls,
        "textOutput": tools.text_output,
        "policyMode": tools.policy_mode, "policyOutcome": tools.policy_outcome,
        "policyFallback": tools.policy_fallback, "deferredToolSearch": tools.deferred_tool_search,
        "catalog": catalog,
    })
}
