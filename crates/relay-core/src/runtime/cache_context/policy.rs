//! Allowlisted observations only. Never normalize a request or infer support.
use crate::usage::{
    CacheBreakpointDiagnostics, CacheKeyPresence, CachePolicyDiagnostics, CachePolicyMode,
    CachePolicyTtl, CachePrewarm,
};
use serde_json::Value;

const MAX_INSPECTED_BLOCKS: usize = 4_096;

pub(super) fn observe(request: &Value) -> CachePolicyDiagnostics {
    let options = request.get("prompt_cache_options");
    let invalid = Value::Null;
    let field = |name| match options {
        Some(Value::Object(fields)) => fields.get(name),
        // A supplied non-object must not look like omitted controls.
        Some(_) => Some(&invalid),
        None => None,
    };
    CachePolicyDiagnostics {
        mode: match field("mode") {
            None => CachePolicyMode::Omitted,
            Some(Value::String(value)) if value == "implicit" => CachePolicyMode::Implicit,
            Some(Value::String(value)) if value == "explicit" => CachePolicyMode::Explicit,
            Some(_) => CachePolicyMode::Other,
        },
        ttl: match field("ttl") {
            None => CachePolicyTtl::Omitted,
            Some(Value::String(value)) if value == "30m" => CachePolicyTtl::ThirtyMinutes,
            Some(Value::String(value)) if value == "12h" => CachePolicyTtl::TwelveHours,
            Some(_) => CachePolicyTtl::Other,
        },
        prewarm: match field("prewarm") {
            None => CachePrewarm::Omitted,
            Some(Value::Bool(true)) => CachePrewarm::Enabled,
            Some(Value::Bool(false)) => CachePrewarm::Disabled,
            Some(_) => CachePrewarm::Other,
        },
        key: match request.get("prompt_cache_key") {
            None => CacheKeyPresence::Omitted,
            Some(Value::String(value)) if value.trim().is_empty() => CacheKeyPresence::Empty,
            Some(Value::String(_)) => CacheKeyPresence::Present,
            Some(_) => CacheKeyPresence::Other,
        },
        breakpoints: breakpoints(request),
    }
}

fn breakpoints(request: &Value) -> CacheBreakpointDiagnostics {
    let mut observed = CacheBreakpointDiagnostics {
        complete: true,
        ..CacheBreakpointDiagnostics::default()
    };
    let mut inspected = 0;
    for (index, item) in request
        .get("input")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        let blocks = match item.get("type").and_then(Value::as_str) {
            None | Some("message") => item.get("content"),
            Some("function_call_output" | "custom_tool_call_output") => item.get("output"),
            // Tool schemas, configuration updates and opaque state are not blocks.
            _ => None,
        };
        for block in blocks.and_then(Value::as_array).into_iter().flatten() {
            if inspected == MAX_INSPECTED_BLOCKS {
                observed.complete = false;
                return observed;
            }
            inspected += 1;
            let Some(marker) = block.get("prompt_cache_breakpoint") else {
                continue;
            };
            if marker.get("mode").and_then(Value::as_str) == Some("explicit") {
                observed.explicit += 1;
                observed.ttl_overrides += u32::from(marker.get("ttl").is_some());
                observed.last_input_item = Some((index + 1) as u32);
            } else {
                observed.other += 1;
            }
        }
    }
    observed
}

#[cfg(test)]
mod tests;
