use super::*;

#[test]
fn budget_allows_only_three_dispatches_and_respects_commit_boundaries() {
    let mut budget = RequestBudget::default_for(RequestId(1));
    assert_eq!(budget.remaining(), 3);
    assert_eq!(budget.start_dispatch(), Some(AttemptId(1)));
    assert_eq!(
        budget.retry_decision(ExecutionObservation::not_sent()),
        RetryDecision::Retry { next_dispatch: 2 }
    );
    assert_eq!(budget.start_dispatch(), Some(AttemptId(2)));
    assert_eq!(budget.start_dispatch(), Some(AttemptId(3)));
    assert_eq!(budget.start_dispatch(), None);
    assert_eq!(
        budget.retry_decision(ExecutionObservation::not_sent()),
        RetryDecision::Stop(RetryStopReason::BudgetExhausted)
    );
    assert_eq!(
        budget.retry_decision(ExecutionObservation::unknown()),
        RetryDecision::Stop(RetryStopReason::RemoteOutcomeUnknown)
    );
    assert_eq!(
        RequestBudget::default_for(RequestId(2)).retry_decision(ExecutionObservation::committed()),
        RetryDecision::Stop(RetryStopReason::ResponseCommitted)
    );
    assert_eq!(
        RequestBudget::default_for(RequestId(3)).retry_decision(ExecutionObservation::accepted()),
        RetryDecision::Stop(RetryStopReason::ReplayUnavailable)
    );
    assert_eq!(
        RequestBudget::default_for(RequestId(4)).retry_decision(ExecutionObservation::accepted()),
        RetryDecision::Stop(RetryStopReason::ReplayUnavailable)
    );
}

#[test]
fn configured_budget_is_not_silently_reduced_to_fixture_default() {
    let budget = RequestBudget::for_incoming_request(8);
    assert_eq!(budget.max_dispatches(), 8);
    assert_eq!(RequestBudget::for_incoming_request(0).max_dispatches(), 1);
}

#[test]
fn wire_and_work_budgets_are_independent_but_bounded() {
    let mut budget = RequestBudget::with_limits(RequestId(78), 2, 3).unwrap();
    assert_eq!(budget.start_wire_attempt(), Some(1));
    assert_eq!(budget.start_wire_attempt(), Some(2));
    assert_eq!(budget.start_dispatch(), Some(AttemptId(1)));
    assert_eq!(budget.start_wire_attempt(), Some(3));
    assert_eq!(budget.start_wire_attempt(), None);
    assert_eq!(budget.start_dispatch(), Some(AttemptId(2)));
    assert_eq!(budget.start_dispatch(), None);
}

#[test]
fn accepted_work_needs_all_four_retry_proofs() {
    let budget = RequestBudget::default_for(RequestId(77));
    let accepted = RetryEvidence {
        input_repeatable: true,
        execution: ExecutionEvidence::Accepted,
        target_portable: true,
        idempotency: IdempotencyContract::None,
    };
    assert_eq!(
        budget.retry_decision_with_evidence(ExecutionObservation::accepted(), accepted),
        RetryDecision::Stop(RetryStopReason::ReplayUnavailable)
    );
    let mut deduplicated = accepted;
    deduplicated.idempotency = IdempotencyContract::Proven;
    assert_eq!(
        budget.retry_decision_with_evidence(ExecutionObservation::accepted(), deduplicated,),
        RetryDecision::Retry { next_dispatch: 1 }
    );
    let mut non_repeatable = deduplicated;
    non_repeatable.input_repeatable = false;
    assert_eq!(
        budget.retry_decision_with_evidence(ExecutionObservation::accepted(), non_repeatable,),
        RetryDecision::Stop(RetryStopReason::ReplayUnavailable)
    );
}
