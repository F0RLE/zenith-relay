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

pub(in crate::gateway) fn is_deferred_tool_search_compatibility_error(
    status: StatusCode,
    details: &crate::usage::UpstreamErrorDetails,
) -> bool {
    if !status.is_client_error() {
        return false;
    }
    let text = format!(
        "{} {} {}",
        details.code.as_deref().unwrap_or_default(),
        details.error_type.as_deref().unwrap_or_default(),
        details.message.as_deref().unwrap_or_default(),
    )
    .to_ascii_lowercase();
    text.contains("tool_search")
        || text.contains("tool search")
        || text.contains("defer_loading")
        || text.contains("deferred tool")
        || (text.contains("unsupported") && text.contains("tool"))
}

/// Internal request extension, never a wire header or persisted request body.
#[derive(Clone)]
pub(in crate::gateway) struct RequestToolPolicy {
    policy: crate::ToolPolicy,
    configured_mode: crate::ToolPolicyMode,
    policy_fallback: bool,
    deferred_disabled: bool,
    deferred_applied: bool,
    pub(in crate::gateway) diagnostics: ToolUseDiagnostics,
}

impl RequestToolPolicy {
    pub(in crate::gateway) fn new(runtime: &GatewayRuntime, request: &Value) -> Self {
        let policy = runtime.tool_policy();
        Self {
            configured_mode: policy.mode,
            policy,
            policy_fallback: false,
            deferred_disabled: false,
            deferred_applied: false,
            diagnostics: tool_use_diagnostics(request),
        }
    }

    pub(in crate::gateway) fn apply(&mut self, request: &mut Value) -> Result<(), &'static str> {
        self.apply_value(request, false)
    }

    pub(in crate::gateway) fn apply_value(
        &mut self,
        request: &mut Value,
        allow_deferred_tool_search: bool,
    ) -> Result<(), &'static str> {
        let result = crate::tool_policy::apply_tool_policy(request, &self.policy)?;
        let deferred = allow_deferred_tool_search
            && !self.deferred_disabled
            && matches!(
                self.diagnostics.tool_choice,
                crate::ToolChoiceMode::Auto | crate::ToolChoiceMode::Unspecified
            )
            && crate::tool_policy::enable_deferred_tool_search(request, &self.policy);
        self.deferred_applied |= deferred;
        self.record(result, deferred, request);
        Ok(())
    }

    pub(in crate::gateway) fn apply_adapter(
        &mut self,
        request: &mut crate::PreparedAdapterRequest,
        allow_deferred_tool_search: bool,
    ) -> Result<(), &'static str> {
        let result = request.apply_tool_policy(&self.policy)?;
        let deferred = allow_deferred_tool_search
            && !self.deferred_disabled
            && matches!(
                self.diagnostics.tool_choice,
                crate::ToolChoiceMode::Auto | crate::ToolChoiceMode::Unspecified
            )
            && crate::tool_policy::enable_deferred_tool_search(
                request.upstream_body_mut(),
                &self.policy,
            );
        self.deferred_applied |= deferred;
        self.record(result, deferred, request.upstream_body());
        Ok(())
    }

    fn record(
        &mut self,
        mut result: crate::tool_policy::ToolPolicyResult,
        deferred: bool,
        request: &Value,
    ) {
        if deferred {
            // Include the provider control tool and defer flags in the
            // serialized-catalog diagnostic for this attempt. These are wire
            // bytes, not a claim about model-context tokens.
            result.after = crate::tool_policy::catalog_stats(request);
            result.outcome = crate::ToolPolicyOutcome::Deferred;
        }
        // Keep the configured mode visible on every attempt.
        self.diagnostics.policy_mode = Some(self.configured_mode);
        self.diagnostics.policy_fallback = self.policy_fallback;
        self.diagnostics.deferred_tool_search = deferred;
        // Capture the exact post-policy Value that will be serialized. Do not
        // deserialize the entire conversation again just to count its catalog.
        // Usage rows describe one attempt, not the maximum across other routes.
        self.diagnostics.forwarded_tool_count = result.after.count;
        self.diagnostics.forwarded_schema_bytes = Some(result.after.bytes);
        self.diagnostics.filtered_tool_count =
            result.before.count.saturating_sub(result.after.count);
        self.diagnostics.policy_outcome = Some(result.outcome);
    }

    /// Allow one compatibility retry for a deferred request.
    pub(in crate::gateway) fn prepare_deferred_fallback(&mut self) -> bool {
        if self.deferred_applied && !self.deferred_disabled {
            self.deferred_disabled = true;
            self.policy_fallback = true;
            self.diagnostics.policy_fallback = true;
            return true;
        }
        false
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
