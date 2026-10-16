use super::*;
use crate::accounts::{
    AccountAuthMode, AccountAuthState, AccountHealthState, AccountIdentity, AccountRecord,
};
use crate::quota::{QuotaSnapshot, QuotaTransition, QuotaWindow, Subscription};

fn quota_window(
    kind: QuotaWindowKind,
    available_basis_points: Option<u16>,
    explicitly_full: Option<bool>,
    reset_at_ms: Option<u64>,
    observed_at_ms: u64,
) -> QuotaWindow {
    QuotaWindow {
        kind,
        provider_cycle_id: None,
        window_start_ms: None,
        available_basis_points,
        explicitly_full,
        reset_at_ms,
        window_minutes: Some(300),
        observed_at_ms,
        full_transition_fingerprint: Some("cycle-1".into()),
        exhaustion_transition_fingerprint: None,
    }
}

fn account() -> AccountRecord {
    AccountRecord {
        id: "account-1".into(),
        label: "Account".into(),
        identity: AccountIdentity::from_hashed_parts(
            "openai",
            "example.test",
            "identity-hash",
            "secret-hash",
            "default",
            None,
        )
        .unwrap(),
        auth_mode: AccountAuthMode::OAuth,
        auth_state: AccountAuthState::Active,
        health: AccountHealthState::Healthy,
        source_id: "openai".into(),
        secret_refs: vec!["account:account-1".into()],
        subscription: Subscription::default(),
        quota: QuotaSnapshot {
            primary: Some(quota_window(
                QuotaWindowKind::Primary,
                Some(10_000),
                Some(true),
                Some(10_000),
                100,
            )),
            ..QuotaSnapshot::default()
        },
        token_generation: 1,
        token_updated_at_ms: Some(1),
        tags: ["default".to_string()].into(),
        enabled: true,
        in_pool: true,
        draining: false,
        created_at_ms: 1,
        last_used_at_ms: None,
        last_error_code: None,
    }
}

fn task(max_attempts: u8, jitter_seconds: u32) -> WakeTask {
    WakeTask {
        id: "task-1".into(),
        name: "Primary wake".into(),
        enabled: true,
        account_selector: AccountSelector::AllEligible,
        window_kinds: [QuotaWindowKind::Primary].into(),
        model_policy: WakeModelPolicy::LightestSupported,
        trigger: WakeTrigger::QuotaFull,
        fallback_schedule: None,
        execution_policy: WakeExecutionPolicy::Automatic,
        jitter_seconds,
        max_attempts_per_cycle: max_attempts,
        created_at_ms: 1,
        updated_at_ms: 1,
    }
}

fn transition() -> QuotaTransition {
    transition_with_fingerprint("cycle-1")
}

fn transition_with_fingerprint(fingerprint: &str) -> QuotaTransition {
    QuotaTransition {
        window_kind: QuotaWindowKind::Primary,
        fingerprint: fingerprint.into(),
        transitioned_at_ms: 100,
    }
}

fn policy() -> WakeAdapterPolicy {
    WakeAdapterPolicy {
        windows_requiring_activity: [QuotaWindowKind::Primary].into(),
        models: vec![
            WakeModel {
                id: "large".into(),
                lightness_rank: 10,
                wake_capable: true,
            },
            WakeModel {
                id: "small".into(),
                lightness_rank: 1,
                wake_capable: true,
            },
        ],
        verification_delay_ms: 1_000,
        output_token_cap: 8,
    }
}

fn schedule(coordinator: &mut WakeCoordinator, task: &WakeTask, now_ms: u64) -> WakeSchedule {
    schedule_for(coordinator, task, &account(), &transition(), now_ms)
}

fn schedule_for(
    coordinator: &mut WakeCoordinator,
    task: &WakeTask,
    account: &AccountRecord,
    transition: &QuotaTransition,
    now_ms: u64,
) -> WakeSchedule {
    match coordinator.evaluate(task, account, transition, None, &policy(), now_ms) {
        WakeDecision::Scheduled(schedule) => schedule,
        decision => panic!("unexpected decision: {decision:?}"),
    }
}

fn completion(outcome: WakeCompletionOutcome, completed_at_ms: u64) -> WakeCompletion {
    WakeCompletion {
        outcome,
        completed_at_ms,
        latency_ms: Some(10),
        input_tokens: Some(1),
        output_tokens: Some(1),
        error_code: None,
    }
}

#[test]
fn retry_cap_allows_only_one_explicit_retry() {
    let mut coordinator = WakeCoordinator::new(8, 8).unwrap();
    let schedule = schedule(&mut coordinator, &task(2, 0), 110);
    let first = coordinator.claim_due(schedule.due_at_ms, 1).pop().unwrap();
    assert_eq!(first.attempt, 1);
    assert!(coordinator.complete(first, completion(WakeCompletionOutcome::Unconfirmed, 120)));

    let retry = coordinator.pending().pop().unwrap();
    let second = coordinator.claim_due(retry.due_at_ms, 1).pop().unwrap();
    assert_eq!(second.attempt, 2);
    assert!(coordinator.complete(second, completion(WakeCompletionOutcome::Failed, 130)));
    assert!(coordinator.pending().is_empty());
    assert_eq!(
        coordinator.evaluate(&task(2, 0), &account(), &transition(), None, &policy(), 140),
        WakeDecision::Skipped(WakeOutcome::SkippedDuplicate)
    );
    assert_eq!(
        coordinator
            .state()
            .history()
            .iter()
            .map(|history_record| (history_record.attempt, history_record.outcome))
            .collect::<Vec<_>>(),
        vec![(1, WakeOutcome::Unconfirmed), (2, WakeOutcome::Failed)]
    );
}

#[test]
fn confirmed_attempt_permanently_completes_cycle() {
    let task = task(2, 0);
    let mut coordinator = WakeCoordinator::new(8, 8).unwrap();
    let schedule = schedule(&mut coordinator, &task, 110);
    let permit = coordinator.claim_due(schedule.due_at_ms, 1).pop().unwrap();
    assert!(coordinator.complete(permit, completion(WakeCompletionOutcome::Confirmed, 120)));
    assert!(coordinator.pending().is_empty());
    assert_eq!(
        coordinator.evaluate(&task, &account(), &transition(), None, &policy(), 130),
        WakeDecision::Skipped(WakeOutcome::SkippedDuplicate)
    );
    assert_eq!(
        coordinator.state().history().back().unwrap().outcome,
        WakeOutcome::Confirmed
    );
}

#[test]
fn pending_and_retry_due_times_survive_restart() {
    let task = task(2, 30);
    let mut coordinator = WakeCoordinator::new(8, 8).unwrap();
    let schedule = schedule(&mut coordinator, &task, 1_000);
    let serialized = serde_json::to_string(coordinator.state()).unwrap();
    let state: WakeAutomationState = serde_json::from_str(&serialized).unwrap();
    let mut coordinator = WakeCoordinator::from_state(state).unwrap();
    assert_eq!(coordinator.pending(), vec![schedule.clone()]);
    assert!(coordinator
        .claim_due(schedule.due_at_ms.saturating_sub(1), 1)
        .is_empty());

    let first = coordinator.claim_due(schedule.due_at_ms, 1).pop().unwrap();
    assert!(coordinator.complete(
        first,
        completion(WakeCompletionOutcome::Unconfirmed, schedule.due_at_ms + 10)
    ));
    let retry = coordinator.pending().pop().unwrap();
    let serialized = serde_json::to_string(coordinator.state()).unwrap();
    let state: WakeAutomationState = serde_json::from_str(&serialized).unwrap();
    let mut coordinator = WakeCoordinator::from_state(state).unwrap();
    assert_eq!(coordinator.pending(), vec![retry.clone()]);
    assert_eq!(coordinator.claim_due(retry.due_at_ms, 1)[0].attempt, 2);
}

#[test]
fn jitter_is_deterministic_and_bounded() {
    let task = task(1, 30);
    let mut first = WakeCoordinator::new(8, 8).unwrap();
    let mut second = WakeCoordinator::new(8, 8).unwrap();
    let first = schedule(&mut first, &task, 5_000);
    let second = schedule(&mut second, &task, 5_000);
    assert_eq!(first.due_at_ms, second.due_at_ms);
    assert!((5_000..=35_000).contains(&first.due_at_ms));
}

#[test]
fn verification_confirms_only_consumption_or_advanced_countdown() {
    let before = quota_window(
        QuotaWindowKind::Primary,
        Some(10_000),
        Some(true),
        Some(10_000),
        100,
    );
    let consumed = quota_window(
        QuotaWindowKind::Primary,
        Some(9_000),
        Some(false),
        Some(10_000),
        200,
    );
    assert_eq!(
        verify_wake_countdown(Some(&before), Some(&consumed)),
        WakeVerificationOutcome::ConfirmedQuotaConsumed
    );
    let advanced = quota_window(
        QuotaWindowKind::Primary,
        Some(10_000),
        Some(true),
        Some(20_000),
        200,
    );
    assert_eq!(
        verify_wake_countdown(Some(&before), Some(&advanced)),
        WakeVerificationOutcome::ConfirmedCountdownAdvanced
    );
    let unchanged = quota_window(
        QuotaWindowKind::Primary,
        Some(10_000),
        Some(true),
        Some(10_000),
        200,
    );
    assert_eq!(
        verify_wake_countdown(Some(&before), Some(&unchanged)),
        WakeVerificationOutcome::Unconfirmed
    );
    let unknown = quota_window(QuotaWindowKind::Primary, None, None, Some(20_000), 200);
    assert_eq!(
        verify_wake_countdown(Some(&before), Some(&unknown)),
        WakeVerificationOutcome::Unconfirmed
    );
    assert_eq!(
        verify_wake_countdown(None, Some(&consumed)),
        WakeVerificationOutcome::Unconfirmed
    );
    let wrong_kind = quota_window(
        QuotaWindowKind::Secondary,
        Some(9_000),
        Some(false),
        Some(20_000),
        200,
    );
    assert_eq!(
        verify_wake_countdown(Some(&before), Some(&wrong_kind)),
        WakeVerificationOutcome::Unconfirmed
    );
}

#[test]
fn natural_use_permanently_completes_pending_cycle() {
    let task = task(2, 30);
    let mut coordinator = WakeCoordinator::new(8, 8).unwrap();
    schedule(&mut coordinator, &task, 110);
    assert_eq!(
        coordinator.evaluate(&task, &account(), &transition(), Some(100), &policy(), 120,),
        WakeDecision::Skipped(WakeOutcome::SkippedAlreadyStarted)
    );
    assert!(coordinator.pending().is_empty());
    assert!(coordinator.claim_due(u64::MAX, 1).is_empty());
    assert_eq!(
        coordinator.evaluate(&task, &account(), &transition(), None, &policy(), 130),
        WakeDecision::Skipped(WakeOutcome::SkippedDuplicate)
    );
    assert_eq!(
        coordinator.state().history().back().unwrap().outcome,
        WakeOutcome::SkippedAlreadyStarted
    );
}

#[test]
fn automatic_and_confirmation_claims_do_not_steal_each_other() {
    let automatic = task(1, 0);
    let mut confirmation = task(1, 0);
    confirmation.id = "task-confirm".into();
    confirmation.execution_policy = WakeExecutionPolicy::RequireConfirmation;
    let mut coordinator = WakeCoordinator::new(8, 8).unwrap();
    schedule(&mut coordinator, &automatic, 110);
    schedule_for(
        &mut coordinator,
        &confirmation,
        &account(),
        &transition_with_fingerprint("cycle-2"),
        110,
    );

    assert_eq!(coordinator.next_automatic_due(), Some(110));
    let confirmations = coordinator.claim_due_confirmations(110, 8);
    assert_eq!(confirmations.len(), 1);
    assert_eq!(confirmations[0].task_id, confirmation.id);
    assert_eq!(coordinator.next_automatic_due(), Some(110));
    let automatic = coordinator.claim_due_automatic(110, 8);
    assert_eq!(automatic.len(), 1);
    assert_eq!(automatic[0].task_id, "task-1");
    assert_eq!(coordinator.next_automatic_due(), None);
    assert!(coordinator.pending().is_empty());
}

#[test]
fn only_primary_recovery_can_schedule_or_survive_restart() {
    let mut coordinator = WakeCoordinator::new(8, 8).unwrap();
    schedule(&mut coordinator, &task(1, 0), 110);
    coordinator.state.cycles[0].window_kind = QuotaWindowKind::Secondary;

    let restored = WakeCoordinator::from_state(coordinator.into_state()).unwrap();
    assert!(restored.pending().is_empty());
    assert_eq!(restored.state().history().len(), 1);
    assert_eq!(
        restored.state().history()[0].error_code.as_deref(),
        Some("wake_window_redundant")
    );

    let mut secondary = transition();
    secondary.window_kind = QuotaWindowKind::Secondary;
    let mut policy = policy();
    policy
        .windows_requiring_activity
        .insert(QuotaWindowKind::Secondary);
    let mut coordinator = WakeCoordinator::new(8, 8).unwrap();
    assert_eq!(
        coordinator.evaluate(&task(1, 0), &account(), &secondary, None, &policy, 110),
        WakeDecision::Skipped(WakeOutcome::SkippedIneligible)
    );
    assert!(coordinator.pending().is_empty());
}

#[test]
fn tasks_share_one_global_account_window_cycle() {
    let first = task(1, 0);
    let mut second = task(1, 0);
    second.id = "task-2".into();
    let mut coordinator = WakeCoordinator::new(8, 8).unwrap();
    schedule(&mut coordinator, &first, 110);

    assert_eq!(
        coordinator.evaluate(&second, &account(), &transition(), None, &policy(), 110),
        WakeDecision::Skipped(WakeOutcome::SkippedDuplicate)
    );
    assert_eq!(coordinator.pending().len(), 1);
    assert_eq!(coordinator.claim_due_automatic(110, 8)[0].task_id, first.id);
}

#[test]
fn task_and_account_cancellation_finish_pending_and_in_flight_cycles() {
    let first = task(1, 0);
    let mut second = task(1, 0);
    second.id = "task-2".into();
    let mut other_account = account();
    other_account.id = "account-2".into();
    let mut coordinator = WakeCoordinator::new(8, 8).unwrap();
    schedule(&mut coordinator, &first, 110);
    schedule_for(
        &mut coordinator,
        &second,
        &account(),
        &transition_with_fingerprint("cycle-2"),
        110,
    );
    schedule_for(
        &mut coordinator,
        &second,
        &other_account,
        &transition(),
        110,
    );

    let first_permit = coordinator.claim_due_automatic(110, 1).pop().unwrap();
    assert!(coordinator.is_permit_active(&first_permit));
    assert_eq!(coordinator.remove_pending_for_task(&first.id, 120), 1);
    assert!(!coordinator.is_permit_active(&first_permit));
    assert!(!coordinator.complete(
        first_permit,
        completion(WakeCompletionOutcome::Unconfirmed, 121)
    ));
    assert_eq!(coordinator.remove_pending_for_account("account-1", 122), 1);
    assert_eq!(coordinator.pending().len(), 1);
    let permit = coordinator.claim_due_automatic(110, 1).pop().unwrap();
    assert_eq!(permit.account_id, other_account.id);
    assert_eq!(coordinator.remove_pending_for_account("account-2", 123), 1);
    assert!(!coordinator.is_permit_active(&permit));
    assert_eq!(coordinator.state().history().len(), 3);
    assert!(coordinator.state().history().iter().all(|history_record| {
        history_record.outcome == WakeOutcome::SkippedIneligible
            && history_record.model_id.is_none()
            && history_record.input_tokens.is_none()
            && history_record.output_tokens.is_none()
            && history_record
                .error_code
                .as_deref()
                .is_some_and(|code| code.starts_with("wake_"))
    }));
}

#[test]
fn natural_use_completes_pending_and_in_flight_cycles_with_redacted_history() {
    let first = task(1, 0);
    let mut second = task(1, 0);
    second.id = "task-2".into();
    let mut other_account = account();
    other_account.id = "account-2".into();
    let mut coordinator = WakeCoordinator::new(8, 8).unwrap();
    schedule(&mut coordinator, &first, 110);
    schedule_for(
        &mut coordinator,
        &second,
        &account(),
        &transition_with_fingerprint("cycle-2"),
        110,
    );
    schedule_for(&mut coordinator, &first, &other_account, &transition(), 110);
    let in_flight = coordinator.claim_due_automatic(110, 1).pop().unwrap();
    assert!(coordinator.is_permit_active(&in_flight));

    assert_eq!(
        coordinator.mark_natural_use_for_account("account-1", 120),
        2
    );
    assert!(!coordinator.is_permit_active(&in_flight));
    assert!(!coordinator.complete(
        in_flight,
        completion(WakeCompletionOutcome::Unconfirmed, 121)
    ));
    assert_eq!(coordinator.pending().len(), 1);
    assert_eq!(coordinator.pending()[0].request.account_id, "account-2");
    let history = coordinator.state().history();
    assert_eq!(history.len(), 2);
    assert!(history.iter().all(|history_record| {
        history_record.account_id == "account-1"
            && history_record.outcome == WakeOutcome::SkippedAlreadyStarted
            && history_record.model_id.is_none()
            && history_record.latency_ms.is_none()
            && history_record.input_tokens.is_none()
            && history_record.output_tokens.is_none()
            && history_record.error_code.is_none()
    }));
    let serialized = serde_json::to_string(history).unwrap();
    for secret in ["small", "Bearer", "prompt", "response body"] {
        assert!(!serialized.contains(secret));
    }
}

#[test]
fn state_and_history_never_serialize_request_or_response_content() {
    let mut coordinator = WakeCoordinator::new(8, 8).unwrap();
    let schedule = schedule(&mut coordinator, &task(1, 0), 110);
    let scheduled = serde_json::to_string(coordinator.state()).unwrap();
    assert!(scheduled.contains("outputTokenCap"));
    assert!(!scheduled.contains("prompt"));
    assert!(!scheduled.contains("response body"));
    let permit = coordinator.claim_due(schedule.due_at_ms, 1).pop().unwrap();
    assert!(coordinator.complete(
        permit,
        WakeCompletion {
            outcome: WakeCompletionOutcome::Failed,
            completed_at_ms: 120,
            latency_ms: None,
            input_tokens: None,
            output_tokens: None,
            error_code: Some("Bearer secret prompt response body".into()),
        }
    ));
    let serialized = serde_json::to_string(coordinator.state()).unwrap();
    assert!(!serialized.contains("Bearer"));
    assert!(!serialized.contains("secret"));
    assert!(!serialized.contains("prompt"));
    assert!(!serialized.contains("response body"));
    assert!(serialized.contains("redacted"));
}

#[test]
fn unsupported_schedules_and_attempt_limits_are_rejected() {
    let mut invalid = task(0, 0);
    assert_eq!(
        invalid.validate(),
        Err(WakeTaskValidationError::InvalidAttemptLimit)
    );
    invalid.max_attempts_per_cycle = MAX_WAKE_ATTEMPTS + 1;
    assert_eq!(
        invalid.validate(),
        Err(WakeTaskValidationError::InvalidAttemptLimit)
    );
    invalid.max_attempts_per_cycle = 1;
    invalid.trigger = WakeTrigger::Daily;
    assert_eq!(
        invalid.validate(),
        Err(WakeTaskValidationError::UnsupportedSchedule)
    );
    invalid.trigger = WakeTrigger::QuotaFull;
    invalid.fallback_schedule = Some(WakeTrigger::Interval(60));
    assert_eq!(
        invalid.validate(),
        Err(WakeTaskValidationError::UnsupportedSchedule)
    );
}

#[test]
fn lightest_model_rank_prefers_nano_then_mini() {
    assert!(model_lightness_rank("gpt-nano", 9) < model_lightness_rank("gpt-mini", 1));
    assert!(model_lightness_rank("gpt-mini", 9) < model_lightness_rank("gpt-large", 0));
}
