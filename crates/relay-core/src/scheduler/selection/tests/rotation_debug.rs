use super::*;
use crate::scheduler::rotation::{CandidateBlockReason, RotationConfigurationBlock};

#[test]
fn capture_preserves_exact_admission_and_is_independent_of_later_changes() {
    let mut scheduler = PoolScheduler::new();
    scheduler.upsert(candidate("ready"));
    scheduler.upsert(RuntimeCandidate {
        enabled: false,
        ..candidate("disabled")
    });
    scheduler.upsert(candidate("tried"));
    let mut capture = None;
    let selected = scheduler
        .select_with_rotation_capture(
            SelectionRequest {
                model: "gpt-5",
                allowed_protocols: &[WireApi::Responses],
                scope: &CandidateScope::default(),
                tried: &HashSet::from(["tried".into()]),
                response_affinity_key: None,
                prompt_affinity_key: None,
                now_ms: 100,
            },
            RotationOperation::Text,
            Some(&mut capture),
        )
        .unwrap();
    let snapshot = capture.unwrap();
    assert!(!snapshot.preparation_blocked);
    scheduler.remove("ready");
    let trace = snapshot.into_trace();
    assert_eq!(
        trace.selected_candidate_id.as_deref(),
        Some(selected.candidate_id.as_str())
    );
    assert_eq!(trace.candidates[0].in_flight, 0);
    assert_eq!(trace.candidates.len(), 3);
    assert_eq!(
        trace
            .candidates
            .iter()
            .find(|c| c.candidate_id == "disabled")
            .unwrap()
            .configuration_block,
        Some(RotationConfigurationBlock::Disabled)
    );
    assert_eq!(
        trace
            .candidates
            .iter()
            .find(|c| c.candidate_id == "tried")
            .unwrap()
            .block_reason,
        Some(CandidateBlockReason::AlreadyTried)
    );
}

#[test]
fn blocked_preparation_cannot_invent_a_fallback_to_another_account() {
    let mut scheduler = PoolScheduler::new();
    scheduler.upsert(candidate("owner"));
    scheduler.upsert(candidate("other"));
    scheduler.bind_response_affinity("response", "owner", 100);
    scheduler.upsert(RuntimeCandidate {
        enabled: false,
        ..candidate("owner")
    });
    let mut capture = None;
    let selected = scheduler.select_with_rotation_capture(
        SelectionRequest {
            model: "gpt-5",
            allowed_protocols: &[WireApi::Responses],
            scope: &CandidateScope::default(),
            tried: &HashSet::new(),
            response_affinity_key: Some("response"),
            prompt_affinity_key: None,
            now_ms: 101,
        },
        RotationOperation::Text,
        Some(&mut capture),
    );
    assert!(selected.is_none());
    let snapshot = capture.unwrap();
    assert!(snapshot.preparation_blocked);
    let trace = snapshot.into_trace();
    assert!(trace.selected_candidate_id.is_none());
    assert_eq!(trace.owner_candidate_id.as_deref(), Some("owner"));
    assert!(trace.candidates.iter().all(|c| c.fallback_rank.is_none()));
    assert_eq!(
        trace
            .candidates
            .iter()
            .find(|c| c.candidate_id == "other")
            .unwrap()
            .configuration_block,
        Some(RotationConfigurationBlock::RequestPreparation)
    );
}
