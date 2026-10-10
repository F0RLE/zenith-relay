use super::*;

pub(in crate::gateway) fn tool_use_diagnostics(request_payload: &Value) -> ToolUseDiagnostics {
    let stats = crate::tool_policy::catalog_stats(request_payload);
    ToolUseDiagnostics {
        client_tool_count: stats.count,
        client_schema_bytes: Some(stats.bytes),
        tool_choice: tool_choice_mode(request_payload),
        ..ToolUseDiagnostics::default()
    }
}

/// Internal request extension, never a wire header or persisted request body.
#[derive(Clone)]
pub(in crate::gateway) struct RequestToolPolicy {
    policy: crate::ToolPolicy,
    configured_mode: crate::ToolPolicyMode,
    pub(in crate::gateway) diagnostics: ToolUseDiagnostics,
    cache_context: Option<crate::runtime::cache_context::CacheContextObservation>,
}

impl RequestToolPolicy {
    pub(in crate::gateway) fn new(runtime: &GatewayRuntime, request: &Value) -> Self {
        let mut policy = runtime.tool_policy();
        // The settings switch is gone. A saved automatic mode must not keep
        // adding defer_loading or tool_search to new requests.
        policy.mode = crate::ToolPolicyMode::PassThrough;
        Self {
            configured_mode: policy.mode,
            policy,
            diagnostics: tool_use_diagnostics(request),
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
        if self.cache_context.is_none() {
            self.cache_context =
                Some(runtime.begin_cache_context(request, key_id, request_id, client_context));
        }
    }

    pub(in crate::gateway) fn observe_cache_context(
        &self,
        runtime: &GatewayRuntime,
        route: &mut crate::runtime::ExecutorRoute,
        upstream: &Value,
    ) {
        if let Some(observation) = self.cache_context.as_ref() {
            runtime.observe_cache_context(observation, route, upstream);
        }
    }

    pub(in crate::gateway) fn apply(&mut self, request: &mut Value) -> Result<(), &'static str> {
        self.apply_value(request)
    }

    pub(in crate::gateway) fn apply_value(
        &mut self,
        request: &mut Value,
    ) -> Result<(), &'static str> {
        let policy_result = crate::tool_policy::apply_tool_policy(request, &self.policy)?;
        self.record(policy_result);
        Ok(())
    }

    pub(in crate::gateway) fn apply_adapter(
        &mut self,
        request: &mut crate::PreparedAdapterRequest,
    ) -> Result<(), &'static str> {
        let policy_result = request.apply_tool_policy(&self.policy)?;
        self.record(policy_result);
        Ok(())
    }

    fn record(&mut self, policy_result: crate::tool_policy::ToolPolicyResult) {
        // Keep the configured mode visible on every attempt.
        self.diagnostics.policy_mode = Some(self.configured_mode);
        // Capture the exact post-policy Value that will be serialized. Do not
        // deserialize the entire conversation again just to count its catalog.
        // Usage rows describe one attempt, not the maximum across other routes.
        self.diagnostics.forwarded_tool_count = policy_result.after.count;
        self.diagnostics.forwarded_schema_bytes = Some(policy_result.after.bytes);
        self.diagnostics.filtered_tool_count = policy_result
            .before
            .count
            .saturating_sub(policy_result.after.count);
        self.diagnostics.policy_outcome = Some(policy_result.outcome);
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
