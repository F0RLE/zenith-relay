use super::*;

pub(in crate::gateway) fn tool_use_diagnostics(
    request_payload: &Value,
    diagnostics_enabled: bool,
) -> ToolUseDiagnostics {
    let stats = crate::tool_policy::catalog_stats_with_sizes(request_payload, diagnostics_enabled);
    ToolUseDiagnostics {
        client_tool_count: stats.count,
        client_schema_bytes: diagnostics_enabled.then_some(stats.bytes),
        client_context: diagnostics_enabled.then(|| {
            crate::usage::RequestContextSize::capture_with_catalog_bytes(
                request_payload,
                stats.bytes,
            )
        }),
        tool_choice: tool_choice_mode(request_payload),
        requested_parallel_tool_calls: request_payload
            .get("parallel_tool_calls")
            .or_else(|| request_payload.get("response")?.get("parallel_tool_calls"))
            .and_then(Value::as_bool),
        ..ToolUseDiagnostics::default()
    }
}

/// Internal request extension, never a wire header or persisted request body.
#[derive(Clone)]
pub(in crate::gateway) struct RequestToolPolicy {
    pub(in crate::gateway) diagnostics: ToolUseDiagnostics,
    cache_context: Option<crate::runtime::cache_context::CacheContextObservation>,
}

impl RequestToolPolicy {
    pub(in crate::gateway) fn new(runtime: &GatewayRuntime, request: &Value) -> Self {
        Self {
            diagnostics: tool_use_diagnostics(request, runtime.diagnostics_enabled()),
            cache_context: None,
        }
    }

    pub(in crate::gateway) fn capture_cache_context(
        &mut self,
        runtime: &GatewayRuntime,
        request: &Value,
        key_id: &str,
        request_id: &str,
        client_context: Option<&str>,
    ) {
        // Keep the original capture on WS -> HTTP fallback and route repairs.
        if self.diagnostics.client_context.is_some() && self.cache_context.is_none() {
            self.cache_context =
                Some(runtime.begin_cache_context(request, key_id, request_id, client_context));
        }
    }

    pub(in crate::gateway) fn observe_cache_context(
        &self,
        runtime: &GatewayRuntime,
        route: &mut crate::runtime::ExecutorRoute,
        upstream: &mut Value,
    ) {
        if let Some(observation) = self.cache_context.as_ref() {
            runtime.observe_cache_context(observation, route, upstream);
        }
    }

    pub(in crate::gateway) fn capture_forwarded_catalog(&mut self, request: &Value) {
        let snapshot = crate::tool_policy::capture_tool_catalog(
            request,
            self.diagnostics.client_context.is_some(),
        );
        self.diagnostics.policy_mode = Some(crate::ToolPolicyMode::PassThrough);
        // Capture the forwarded catalog before transport encoding (BPS may
        // embed only loaded contracts as prompt text). Do not
        // deserialize the entire conversation again just to count its catalog.
        // Usage rows describe one attempt, not the maximum across other routes.
        self.diagnostics.forwarded_tool_count = snapshot.stats.count;
        self.diagnostics.forwarded_schema_bytes =
            snapshot.context.as_ref().map(|_| snapshot.stats.bytes);
        self.diagnostics.deferred_tool_search = snapshot.stats.deferred_search;
        self.diagnostics.upstream_context = snapshot.context;
        self.diagnostics.filtered_tool_count = 0;
        self.diagnostics.policy_outcome = Some(crate::ToolPolicyOutcome::PassThrough);
    }
}

fn tool_choice_mode(request_payload: &Value) -> ToolChoiceMode {
    let choice = request_payload.get("tool_choice").or_else(|| {
        request_payload
            .get("response")
            .and_then(|response| response.get("tool_choice"))
    });
    match choice {
        None => ToolChoiceMode::Unspecified,
        Some(Value::String(choice_type)) => tool_choice_mode_from_type(choice_type),
        Some(Value::Object(choice_object)) => choice_object
            .get("type")
            .and_then(Value::as_str)
            .map_or(ToolChoiceMode::Specific, tool_choice_mode_from_type),
        Some(_) => ToolChoiceMode::Unspecified,
    }
}

fn tool_choice_mode_from_type(choice_type: &str) -> ToolChoiceMode {
    match choice_type.to_ascii_lowercase().as_str() {
        "auto" => ToolChoiceMode::Auto,
        "required" | "any" => ToolChoiceMode::Required,
        "none" => ToolChoiceMode::None,
        "allowed_tools" => ToolChoiceMode::AllowedTools,
        _ => ToolChoiceMode::Specific,
    }
}
