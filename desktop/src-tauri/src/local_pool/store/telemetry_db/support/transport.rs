use serde_json::{Value, json};
use zenith_relay_core::usage::{
    CacheContextDiagnostics, ProviderCacheDiagnostics, TransportDiagnostics,
};

pub(super) fn cache_context(context: &CacheContextDiagnostics) -> Value {
    let mut value = serde_json::to_value(context).expect("typed cache diagnostics serialize");
    value["providerAvailability"] = json!(match context.provider {
        Some(ProviderCacheDiagnostics::Unavailable) => "reported_unavailable",
        Some(_) => "reported",
        None => "not_observed",
    });
    value
}

pub(super) fn document(transport: &TransportDiagnostics) -> Value {
    let mut value = serde_json::to_value(transport).expect("typed transport diagnostics serialize");
    value["codexServerTimingAvailability"] = json!(if transport.codex_server_timing.is_some() {
        "reported"
    } else if transport.upstream == zenith_relay_core::UsageTransport::Http {
        "unsupported_for_transport"
    } else {
        "not_observed"
    });
    let phases = transport.request_preparation.as_ref().map(|preparation| {
        // Schema validation is nested inside body_ms, never an extra phase.
        [
            preparation.body_ms,
            preparation.catalog_ms,
            preparation.serialization_ms,
            preparation.attachments_ms,
            preparation.context_accounting_ms,
        ]
    });
    let observed = phases.iter().flatten().flatten().count();
    let measured = phases
        .iter()
        .flatten()
        .flatten()
        .try_fold(0_u64, |sum, phase| sum.checked_add(*phase));
    let measured = (observed > 0).then_some(measured).flatten();
    let remainder = transport
        .request_preparation_ms
        .zip(measured)
        .and_then(|(total, measured)| total.checked_sub(measured));
    value["preparationAccounting"] = json!({
        "measuredNonOverlappingPhasesMs": measured,
        "unattributedMs": remainder,
        "observedPhaseCount": observed,
        "status": if observed == 0 || transport.request_preparation_ms.is_none() {
            "not_observed"
        } else if remainder.is_none() { "inconsistent" } else { "consistent" },
    });
    value
}
