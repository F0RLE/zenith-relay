use crate::{runtime::DefaultServiceTier, GatewayRuntime, WireApi};
use serde_json::{Map, Value};

mod schema;
mod shape;
pub(in crate::gateway) use shape::coerce_responses_input_array;
use shape::normalize_account_request_common;

const CODEX_TOOL_CONST_UNION_THRESHOLD: usize = 8;

/// Owns the service-tier field for one routed request.
///
/// Managed Codex requests prefer an explicit native speed selection over the
/// pool default. Every retry preserves that selection, independently of source
/// declarations. Generic client values such as `flex` remain opaque.
#[derive(Clone, Debug)]
pub(in crate::gateway) struct ServiceTierPolicy {
    owner: ServiceTierOwner,
    client_tier: Option<Value>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ServiceTierOwner {
    Client,
    Pool,
}

impl ServiceTierPolicy {
    pub(in crate::gateway) fn client_owned(request: &Value) -> Self {
        Self {
            owner: ServiceTierOwner::Client,
            client_tier: request
                .as_object()
                .and_then(|object| object.get("service_tier"))
                .cloned(),
        }
    }

    pub(in crate::gateway) fn pool_owned(request: &Value) -> Self {
        Self {
            owner: ServiceTierOwner::Pool,
            ..Self::client_owned(request)
        }
    }

    pub(in crate::gateway) fn select_for_model(
        &self,
        runtime: &GatewayRuntime,
        model: &str,
    ) -> DefaultServiceTier {
        self.client_tier.as_ref().map_or_else(
            || runtime.model_effective_service_tier(model),
            |tier| DefaultServiceTier::from_storage_value(tier.as_str().unwrap_or_default()),
        )
    }

    /// Basis Points accepts an omitted tier and the ordinary `auto`, `default`,
    /// and `standard` labels. Any other explicit client value, including Fast,
    /// is a speed the transport cannot carry.
    pub(in crate::gateway) fn rejects_basis_points_client_speed(&self) -> bool {
        let Some(tier) = self.client_tier.as_ref() else {
            return false;
        };
        let Some(name) = tier.as_str() else {
            return true;
        };
        !matches!(
            name.trim().to_ascii_lowercase().as_str(),
            "auto" | "default" | "standard"
        )
    }

    pub(in crate::gateway) fn prepare_for_candidate(
        &self,
        request: &mut Value,
        default: DefaultServiceTier,
        wire_api: WireApi,
    ) {
        // Restore the original client selection before each attempt. Pool
        // defaults apply only when the client did not choose a tier.
        request
            .as_object_mut()
            .expect("request object was validated before routing")
            .remove("service_tier");
        if let Some(client_tier) = self.client_tier.as_ref() {
            request
                .as_object_mut()
                .expect("request object was validated before routing")
                .insert("service_tier".to_string(), client_tier.clone());
        } else if self.owner == ServiceTierOwner::Pool && wire_api != WireApi::Messages {
            apply_default_service_tier_if_missing(request, default);
        }
    }

    pub(in crate::gateway) fn effective_tier(
        &self,
        request: &Value,
        default: DefaultServiceTier,
        wire_api: WireApi,
    ) -> DefaultServiceTier {
        if self.owner == ServiceTierOwner::Pool && wire_api == WireApi::Messages {
            return default;
        }
        if wire_api != WireApi::Messages {
            request_service_tier(request)
        } else {
            DefaultServiceTier::Standard
        }
    }
}

pub(in crate::gateway) fn request_service_tier(request: &Value) -> DefaultServiceTier {
    match request.get("service_tier").and_then(Value::as_str) {
        Some(tier) if tier.eq_ignore_ascii_case("ultrafast") => DefaultServiceTier::Ultrafast,
        Some(tier)
            if tier.eq_ignore_ascii_case("priority") || tier.eq_ignore_ascii_case("fast") =>
        {
            DefaultServiceTier::Fast
        }
        _ => DefaultServiceTier::Standard,
    }
}

/// Apply the pool's speed setting after the request owner has removed any tier
/// it does not control.
///
/// `priority` is the upstream OpenAI spelling for Fast. Standard deliberately
/// remains implicit, matching the Codex/Cockpit behavior and preserving
/// arbitrary client-owned values such as `flex`.
pub(in crate::gateway) fn apply_default_service_tier_if_missing(
    request: &mut Value,
    default: DefaultServiceTier,
) {
    let Some(object) = request.as_object_mut() else {
        return;
    };
    if object.contains_key("service_tier") {
        return;
    }
    let value = match default {
        DefaultServiceTier::Standard => return,
        DefaultServiceTier::Fast => "priority",
        DefaultServiceTier::Ultrafast => "ultrafast",
    };
    object.insert("service_tier".to_string(), Value::String(value.to_string()));
}

pub(in crate::gateway) fn normalize_account_request(
    object: &mut Map<String, Value>,
    responses_lite: bool,
) {
    // This transport normalization preserves native account settings. The
    // request execution layer applies Relay's pool speed policy later, while
    // Responses Lite alone requires `context=all_turns` here.
    object.insert("store".to_string(), Value::Bool(false));
    object.insert("stream".to_string(), Value::Bool(true));
    normalize_account_request_common(object, responses_lite);
}

/// Normalize the legacy non-streaming account compaction contract.
///
/// Unlike a regular Responses request, `/responses/compact` does not accept
/// the streaming transport controls that Relay adds for the normal account
/// path. Remove them even when a client supplied them explicitly; otherwise a
/// request can pass local validation and still be rejected by the account
/// endpoint. All other request fields remain client-owned so newly introduced
/// Codex options are not silently discarded.
pub(in crate::gateway) fn normalize_compact_account_request(
    object: &mut Map<String, Value>,
    responses_lite: bool,
) {
    object.remove("store");
    object.remove("stream");
    normalize_account_request_common(object, responses_lite);
}

pub(in crate::gateway) use shape::{
    normalize_basis_points_request, normalize_responses_lite_request,
    responses_lite_parallel_tool_calls_valid,
};

#[cfg(test)]
mod tests;
