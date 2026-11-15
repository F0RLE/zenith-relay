//! Opt-in rotation traces use the existing redacted, bounded operation log.

use super::{DebugSession, hash_identifier, persist};
use std::sync::{
    OnceLock,
    atomic::{AtomicU64, Ordering},
    mpsc::{self, SyncSender},
};
use zenith_relay_core::{RotationDebugCapture, RotationDebugSink, RotationOperation};

static WRITER: OnceLock<Option<SyncSender<QueuedCapture>>> = OnceLock::new();
static SKIPPED: AtomicU64 = AtomicU64::new(0);
static NEXT_TRACE: AtomicU64 = AtomicU64::new(1);

pub(crate) struct RotationLogger;

struct QueuedCapture {
    epoch: u64,
    capture: RotationDebugCapture,
}

impl RotationDebugSink for RotationLogger {
    fn epoch(&self) -> Option<u64> {
        DebugSession::capture().map(DebugSession::epoch)
    }

    fn submit(&self, epoch: u64, capture: RotationDebugCapture) {
        if self.epoch() != Some(epoch) {
            return;
        }
        let writer = WRITER.get_or_init(|| {
            let (sender, receiver) = mpsc::sync_channel::<QueuedCapture>(2);
            std::thread::Builder::new()
                .name("rotation-diagnostics".into())
                .spawn(move || {
                    while let Ok(queued) = receiver.recv() {
                        if RotationLogger.epoch() == Some(queued.epoch) {
                            write_capture(queued);
                        }
                    }
                })
                .ok()
                .map(|_| sender)
        });
        if writer
            .as_ref()
            .is_none_or(|sender| sender.try_send(QueuedCapture { epoch, capture }).is_err())
        {
            SKIPPED.fetch_add(1, Ordering::Relaxed);
        }
    }
}

fn write_capture(queued: QueuedCapture) {
    emit_capture(
        queued.capture,
        SKIPPED.swap(0, Ordering::Relaxed),
        |trace, message, row| write_row(queued.epoch, trace, message, row),
    );
}

fn emit_capture(
    capture: RotationDebugCapture,
    skipped: u64,
    mut emit: impl FnMut(&str, &str, &str) -> bool,
) {
    let request = hash_identifier(&capture.request_id().to_string());
    let model = hash_identifier(capture.model());
    let operation = match capture.operation() {
        RotationOperation::Text => "text",
        RotationOperation::Image => "image",
        RotationOperation::Compaction => "compaction",
        RotationOperation::Metadata => "metadata",
    };
    let reserved = capture.reserved;
    let preparation_blocked = capture.preparation_blocked;
    let mut trace = capture.into_trace();
    redact_trace(&mut trace);
    let trace_id = format!(
        "{}-{}",
        trace.observed_at_ms,
        NEXT_TRACE.fetch_add(1, Ordering::Relaxed)
    );
    let header = serde_json::json!({
        "observedAtMs": trace.observed_at_ms,
        "request": request,
        "model": model,
        "operation": operation,
        "mode": trace.mode,
        "selected": trace.selected_candidate_id,
        "owner": trace.owner_candidate_id,
        "reserved": reserved,
        "preparationBlocked": preparation_blocked,
        "candidateCount": trace.candidates.len(),
        "fallbackOrderComplete": trace.fallback_order_complete,
        "orderMeaning": "fixed_snapshot_fallback",
        "stages": trace.stages,
        "skippedSnapshots": skipped,
    });
    if !emit(&trace_id, "rotation_decision", &header.to_string()) {
        return;
    }
    for candidate in &trace.candidates {
        if let Ok(row) = serde_json::to_string(candidate)
            && !emit(&trace_id, "rotation_candidate", &row)
        {
            return;
        }
    }
    emit(
        &trace_id,
        "rotation_complete",
        &serde_json::json!({ "candidateCount": trace.candidates.len() }).to_string(),
    );
}

fn redact_trace(trace: &mut zenith_relay_core::RotationDecisionTrace) {
    trace.selected_candidate_id = trace.selected_candidate_id.as_deref().map(hash_identifier);
    trace.owner_candidate_id = trace.owner_candidate_id.as_deref().map(hash_identifier);
    for candidate in &mut trace.candidates {
        candidate.candidate_id = hash_identifier(&candidate.candidate_id);
    }
}

fn write_row(epoch: u64, trace: &str, message: &str, row: &str) -> bool {
    if RotationLogger.epoch() != Some(epoch) {
        return false;
    }
    persist::record_event(
        "operations",
        "info",
        "rotation",
        Some("pool-rotation"),
        None,
        message,
        &[("trace", trace.to_string()), ("row", row.to_string())],
    );
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_identity_is_redacted_and_candidate_rows_survive_existing_log_bounds() {
        use zenith_relay_core::scheduler::rotation::*;
        let identity = "synthetic-operator@example.test";
        let mut trace = RotationDecisionTrace {
            observed_at_ms: 100,
            mode: RotationMode::Automatic,
            selected_candidate_id: Some(identity.into()),
            owner_candidate_id: Some(identity.into()),
            fallback_order_complete: true,
            stages: Vec::new(),
            candidates: vec![RotationCandidateDecision {
                candidate_id: identity.into(),
                fallback_rank: Some(1),
                availability: RotationDecisionAvailability::Ready,
                block_reason: None,
                configuration_block: None,
                retry_at_ms: None,
                eliminated_at: None,
                priority: 1,
                configured_weight: 1,
                effective_weight: Some(8),
                rotation_score: Some(8),
                in_flight: 0,
                effective_capacity: 4,
                quota_remaining_basis_points: Some(9000),
                provider_credits_micro_units: None,
                provider_credits_unlimited: false,
                expiry: Some(RotationExpiryEvidence {
                    expires_at_ms: 1000,
                    valid_until_ms: 500,
                    band: 0,
                    reset_credits: Some(2),
                    urgency_multiplier: 8,
                }),
                preferred: true,
                warm_alternative: false,
            }],
        };
        redact_trace(&mut trace);
        let serialized = serde_json::to_string(&trace).unwrap();
        assert!(!serialized.contains(identity));
        let reference = hash_identifier(identity);
        assert_eq!(
            trace.selected_candidate_id.as_deref(),
            Some(reference.as_str())
        );
        assert_eq!(
            trace.owner_candidate_id.as_deref(),
            Some(reference.as_str())
        );
        assert_eq!(trace.candidates[0].candidate_id, reference);
        let row = serde_json::to_string(&trace.candidates[0]).unwrap();
        assert!(row.len() < super::super::MAX_TEXT_BYTES);
        assert_eq!(super::super::safe_detail(&row), row);
        let decoded: RotationCandidateDecision = serde_json::from_str(&row).unwrap();
        assert_eq!(decoded, trace.candidates[0]);
    }
}
