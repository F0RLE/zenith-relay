use zenith_relay_core::UsageEvent;

/// Logical completion updates reference an existing attempt, never a new send.
pub fn physical_attempts(events: &[UsageEvent]) -> Vec<&UsageEvent> {
    let (completions, attempts): (Vec<_>, Vec<_>) = events
        .iter()
        .partition(|event| event.is_request_completion());
    let mut completed_requests = std::collections::HashSet::new();
    for completion in completions {
        assert!(!completion.success);
        assert!(completed_requests.insert(&completion.request_id));
        assert!(attempts.iter().any(|attempt| {
            attempt.request_id == completion.request_id
                && attempt.attempt == completion.attempt
                && attempt.candidate_id == completion.candidate_id
        }));
    }
    attempts
}
