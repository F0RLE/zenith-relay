use serde_json::{Value, json};
use zenith_relay_core::usage::TransportDiagnostics;

pub(super) fn document(transport: &TransportDiagnostics) -> Value {
    let mut value = serde_json::to_value(transport).expect("typed transport diagnostics serialize");
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
