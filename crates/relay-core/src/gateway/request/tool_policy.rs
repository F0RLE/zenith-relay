use super::*;

pub(in crate::gateway) fn tool_use_diagnostics(value: &Value) -> ToolUseDiagnostics {
    let stats = crate::tool_policy::catalog_stats(value);
    ToolUseDiagnostics {
        client_tool_count: stats.count,
        client_schema_bytes: Some(stats.bytes),
        tool_choice: tool_choice_mode(value),
        ..ToolUseDiagnostics::default()
    }
}

/// Internal request extension, never a wire header or persisted request body.
#[derive(Clone)]
pub(in crate::gateway) struct RequestToolPolicy {
    policy: crate::ToolPolicy,
    configured_mode: crate::ToolPolicyMode,
    pub(in crate::gateway) diagnostics: ToolUseDiagnostics,
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
        }
    }

    pub(in crate::gateway) fn apply(&mut self, request: &mut Value) -> Result<(), &'static str> {
        self.apply_value(request)
    }

    pub(in crate::gateway) fn apply_value(
        &mut self,
        request: &mut Value,
    ) -> Result<(), &'static str> {
        let result = crate::tool_policy::apply_tool_policy(request, &self.policy)?;
        self.record(result);
        Ok(())
    }

    pub(in crate::gateway) fn apply_adapter(
        &mut self,
        request: &mut crate::PreparedAdapterRequest,
    ) -> Result<(), &'static str> {
        let result = request.apply_tool_policy(&self.policy)?;
        self.record(result);
        Ok(())
    }

    fn record(&mut self, result: crate::tool_policy::ToolPolicyResult) {
        // Keep the configured mode visible on every attempt.
        self.diagnostics.policy_mode = Some(self.configured_mode);
        // Capture the exact post-policy Value that will be serialized. Do not
        // deserialize the entire conversation again just to count its catalog.
        // Usage rows describe one attempt, not the maximum across other routes.
        self.diagnostics.forwarded_tool_count = result.after.count;
        self.diagnostics.forwarded_schema_bytes = Some(result.after.bytes);
        self.diagnostics.filtered_tool_count =
            result.before.count.saturating_sub(result.after.count);
        self.diagnostics.policy_outcome = Some(result.outcome);
    }
}

fn tool_choice_mode(value: &Value) -> ToolChoiceMode {
    let choice = value.get("tool_choice").or_else(|| {
        value
            .get("response")
            .and_then(|response| response.get("tool_choice"))
    });
    match choice {
        None => ToolChoiceMode::Unspecified,
        Some(Value::String(value)) => tool_choice_mode_from_type(value),
        Some(Value::Object(object)) => object
            .get("type")
            .and_then(Value::as_str)
            .map_or(ToolChoiceMode::Specific, tool_choice_mode_from_type),
        Some(_) => ToolChoiceMode::Unspecified,
    }
}

fn tool_choice_mode_from_type(value: &str) -> ToolChoiceMode {
    match value.to_ascii_lowercase().as_str() {
        "auto" => ToolChoiceMode::Auto,
        "required" | "any" => ToolChoiceMode::Required,
        "none" => ToolChoiceMode::None,
        "allowed_tools" => ToolChoiceMode::AllowedTools,
        _ => ToolChoiceMode::Specific,
    }
}
