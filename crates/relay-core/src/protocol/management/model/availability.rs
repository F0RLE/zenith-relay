use super::{AccountSummary, OperationalStatus, SourceSummary};
use crate::{CandidateKind, CandidateRuntimeSnapshot};

/// Counts pooled source and account candidates that are currently eligible
/// for rotation. This is a snapshot statistic, not scheduler admission.
pub fn pool_candidate_count(sources: &[SourceSummary], accounts: &[AccountSummary]) -> usize {
    sources
        .iter()
        .filter(|source| {
            source.in_pool
                && source.supports_any_wire_api()
                && source.operational_status == OperationalStatus::Rotation
        })
        .count()
        + accounts
            .iter()
            .filter(|account| {
                account.in_pool && account.operational_status == OperationalStatus::Rotation
            })
            .count()
}

/// Returns whether an enabled pooled API source has a route for this model.
/// Account membership is counted separately by callers.
pub fn model_has_api_source_route(sources: &[SourceSummary], model: &str) -> bool {
    sources.iter().any(|source| {
        source.enabled
            && source.in_pool
            && !source.draining
            && source.secret_available
            && source
                .models_for_any_wire_api()
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(model))
    })
}

pub fn source_runtime_available(
    routing_order: &[CandidateRuntimeSnapshot],
    source_id: &str,
) -> bool {
    routing_order.iter().any(|candidate| {
        candidate.kind == CandidateKind::ApiSource
            && candidate.available
            && (candidate.candidate_id == source_id
                || candidate
                    .candidate_id
                    .strip_prefix(source_id)
                    .is_some_and(|suffix| suffix.starts_with("::")))
    })
}

/// Returns whether an API source has any healthy runtime route exposed through
/// the pool's multi-protocol system key. Candidate ids may be the legacy source
/// id or a protocol-specific child such as `source::messages`.
pub fn pooled_source_runtime_available(
    routing_order: &[CandidateRuntimeSnapshot],
    source_id: &str,
) -> bool {
    source_runtime_available(routing_order, source_id)
}
