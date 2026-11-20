//! Quota job finalization. Only the owning read schedules wake/reset work;
//! followers and canceled UI requests cannot repeat side effects.
use crate::local_pool::{
    accounts::{
        pool_resources::{weekly_reset_member, weekly_reset_rule_matches},
        quota_refresh::{
            AccountQuotaOutcome, AccountQuotaRefreshResponse, read_account_quota_once,
        },
        refresh_observations::AccountRefreshScope,
        reset_credits::{ResetAttempt, ResetOrigin, consume_reset_credit_when},
    },
    background::codex_wake_policy,
    error::{ErrorCode, LocalPoolError, Result},
    models::LocalAccountRecord,
    runtime::current_time_ms,
    state::DesktopState,
    store::AccountRefreshFence,
};
use zenith_relay_core::{
    automations::{
        WakeTask, reset_policy,
        reset_resources::{
            fresh_weekly_reset_window, subscription_allows_reset, weekly_reset_window,
        },
    },
    providers::chatgpt::CodexQuotaClient,
    quota::{QuotaTransition, QuotaWindow, QuotaWindowKind},
    scheduler::refresh::{RefreshIdentity, RefreshKind, recently_active},
};

#[cfg(test)]
mod reserve_tests;
mod weekly_threshold;

pub(super) fn evaluate_updated_transitions(
    state: &DesktopState,
    response: &AccountQuotaRefreshResponse,
) -> Result<()> {
    let transitions = quota_full_candidates(response);
    if transitions.is_empty() {
        return Ok(());
    }
    let capabilities = CodexQuotaClient::new()
        .map_err(|failure| LocalPoolError::new(ErrorCode::InvalidState, failure.code))?
        .capabilities();
    let policy = codex_wake_policy(&response.account, &capabilities);
    let tasks = {
        let store = state.store()?;
        if store
            .account(&response.account.account.id)
            .is_none_or(|latest| latest.account.quota != response.account.account.quota)
        {
            return Ok(());
        }
        store.automations().tasks.clone()
    };
    let now_ms = current_time_ms();
    for transition in &transitions {
        for task in &tasks {
            state.evaluate_wake_transition(
                task,
                &response.account.account,
                transition,
                &policy,
                now_ms,
            )?;
        }
    }
    Ok(())
}

fn quota_full_candidates(response: &AccountQuotaRefreshResponse) -> Vec<QuotaTransition> {
    let AccountQuotaOutcome::Updated { transitions, .. } = &response.quota else {
        return Vec::new();
    };
    let mut transitions = transitions.clone();
    if !transitions
        .iter()
        .any(|transition| transition.window_kind == QuotaWindowKind::Primary)
        && let Some(window) = response
            .account
            .account
            .quota
            .primary
            .as_ref()
            .filter(|window| window.kind == QuotaWindowKind::Primary && window.is_fully_available())
    {
        // A rule can be enabled after the quota became full, or Relay can
        // miss the edge while stopped. Reuse the persisted cycle identity:
        // repeated reads/restarts must never create another wake request.
        if let Some(fingerprint) = window
            .full_transition_fingerprint
            .as_ref()
            .filter(|fingerprint| !fingerprint.trim().is_empty())
        {
            transitions.push(QuotaTransition {
                window_kind: QuotaWindowKind::Primary,
                fingerprint: fingerprint.clone(),
                transitioned_at_ms: window
                    .window_start_ms
                    .filter(|start| *start <= window.observed_at_ms)
                    .unwrap_or(window.observed_at_ms),
            });
        }
    }
    transitions
}

pub(super) async fn evaluate_weekly_exhaustions(
    state: &DesktopState,
    response: &AccountQuotaRefreshResponse,
    fence: &AccountRefreshFence,
) -> Result<Option<AccountQuotaRefreshResponse>> {
    let now_ms = current_time_ms();
    let account = &response.account.account;
    if !matches!(response.quota, AccountQuotaOutcome::Updated { .. })
        || fresh_weekly_reset_window(&account.quota, now_ms).is_none()
    {
        return Ok(None);
    }
    weekly_threshold::confirm_redemption(state, response, fence)?;
    if weekly_threshold::has_matching_rule(state, response)? {
        return weekly_threshold::evaluate(state, response, fence).await;
    }
    if response.account.recovery.is_some()
        || account.quota.reset_credits_available == Some(0)
        || !subscription_allows_reset(&account.subscription, now_ms)
    {
        return Ok(None);
    }
    let Some(fingerprint) = weekly_exhaustion_fingerprint(response, now_ms) else {
        return Ok(None);
    };
    let member_fence = {
        let store = state.store()?;
        store.ensure_account_refresh_current(fence)?;
        if store
            .account(&fence.account_id)
            .is_none_or(|latest| latest.account.quota != response.account.account.quota)
        {
            return Ok(None);
        }
        let credentials = state.cached_account_credential_facts(store.accounts())?;
        let Some(selected) = weekly_reset_member(
            store.gateway(),
            store.accounts(),
            &response.account,
            &credentials,
        ) else {
            return Ok(None);
        };
        if !has_weekly_reserve_rule(&store.automations().tasks, &response.account, selected) {
            return Ok(None);
        }
        store.account_refresh_scope(&selected.account.id)?.1
    };
    // Hold only the reset coordinator across redemption and verification.
    // Other quota jobs and all request execution remain independent.
    let _reset = state.reset_redemption_guard().await;
    let member_fence = &member_fence;
    if state.weekly_reset_was_applied(&account.id, &fingerprint)?
        || !weekly_reset_still_current(state, response, fence, &member_fence.account_id)?
    {
        return Ok(None);
    }
    let attempt = ResetAttempt::new(ResetOrigin::WeeklyReserve(&account.quota));
    let redeemed = consume_reset_credit_when(
        state,
        fence,
        Some(&fingerprint),
        &attempt,
        |expiry| async move {
            state
                .store()?
                .ensure_account_refresh_current(member_fence)?;
            if !weekly_reset_still_current(state, response, fence, &member_fence.account_id)? {
                return Ok(false);
            }
            let now_ms = current_time_ms();
            let subscription_deadline =
                state
                    .store()?
                    .account(&fence.account_id)
                    .and_then(|record| {
                        zenith_relay_core::quota::QuotaSpendDeadline::from_subscription(
                            &record.account.subscription,
                            now_ms,
                            zenith_relay_core::providers::chatgpt::SUBSCRIPTION_REFRESH_INTERVAL_MS,
                        )
                    });
            let ordinary_capacity = state.gateway.runtime().await.and_then(|runtime| {
                runtime.ordinary_quota_alternative_available(&member_fence.account_id, now_ms)
            });
            let cadence = RefreshKind::Quota
                .interval_ms(recently_active(account.last_used_at_ms, now_ms))
                .expect("quota reads have a periodic cadence");
            let next_check_at_ms = now_ms.saturating_add(
                super::reset_due_delay(response, now_ms)
                    .unwrap_or(cadence)
                    .min(cadence),
            );
            let natural_reset = account.quota.secondary.as_ref().and_then(|w| w.reset_at_ms);
            Ok(reset_policy::should_redeem(
                ordinary_capacity,
                natural_reset,
                expiry,
                subscription_deadline,
                next_check_at_ms,
                now_ms,
            ))
        },
    )
    .await;
    if !redeemed.is_ok_and(|redeemed| redeemed) {
        return Ok(None);
    }
    verify_automatic_weekly_reset(state, fence, &attempt).await
}

async fn verify_automatic_weekly_reset(
    state: &DesktopState,
    fence: &AccountRefreshFence,
    attempt: &ResetAttempt<'_>,
) -> Result<Option<AccountQuotaRefreshResponse>> {
    let verification = async {
        let scope = AccountRefreshScope::capture(state, &fence.account_id).await?;
        if scope.fence != *fence {
            return Ok(None);
        }
        read_account_quota_once(state, &scope, false)
            .await
            .map(Some)
    }
    .await;
    attempt.record_verification(
        &fence.account_id,
        verification.as_ref().ok().and_then(Option::as_ref),
    );
    verification
}

fn has_weekly_reserve_rule(
    tasks: &[WakeTask],
    reader: &LocalAccountRecord,
    selected: &LocalAccountRecord,
) -> bool {
    let mut reserve = false;
    for task in tasks
        .iter()
        .filter(|task| weekly_reset_rule_matches(task, reader, selected))
    {
        if task.weekly_reset_remaining_basis_points.is_some() {
            return false;
        }
        reserve = true;
    }
    reserve
}

fn weekly_reset_still_current(
    state: &DesktopState,
    response: &AccountQuotaRefreshResponse,
    fence: &AccountRefreshFence,
    selected_id: &str,
) -> Result<bool> {
    let now_ms = current_time_ms();
    let store = state.store()?;
    store.ensure_account_refresh_current(fence)?;
    let credentials = state.cached_account_credential_facts(store.accounts())?;
    Ok(weekly_reset_current(
        &store,
        response,
        fence,
        selected_id,
        &credentials,
        now_ms,
    ))
}

fn weekly_reset_current(
    store: &crate::local_pool::store::LocalPoolStore,
    response: &AccountQuotaRefreshResponse,
    fence: &AccountRefreshFence,
    selected_id: &str,
    credentials: &std::collections::HashMap<
        String,
        Option<crate::local_pool::state::AccountCredentialFacts>,
    >,
    now_ms: u64,
) -> bool {
    let Some(account) = store.account(&fence.account_id) else {
        return false;
    };
    let Some(selected) =
        weekly_reset_member(store.gateway(), store.accounts(), account, credentials)
    else {
        return false;
    };
    let awaiting_confirmation = weekly_threshold::has_pending_confirmation(
        store.automations(),
        store.accounts(),
        store.gateway(),
        &fence.account_id,
    );
    selected.account.id == selected_id
        && !awaiting_confirmation
        && account.recovery.is_none()
        && subscription_allows_reset(&account.account.subscription, now_ms)
        && fresh_weekly_exhaustion(response, now_ms).is_some()
        && account.account.quota == response.account.account.quota
        && has_weekly_reserve_rule(&store.automations().tasks, account, selected)
}

pub(super) fn schedule_weekly_reserve_checks(
    state: &DesktopState,
    response: &AccountQuotaRefreshResponse,
) -> Result<()> {
    for identity in weekly_reserve_targets(state, response)? {
        state.refresh.mark_dirty(&identity, RefreshKind::Quota);
    }
    Ok(())
}

fn weekly_reserve_targets(
    state: &DesktopState,
    response: &AccountQuotaRefreshResponse,
) -> Result<Vec<RefreshIdentity>> {
    if !matches!(response.quota, AccountQuotaOutcome::Updated { .. })
        || response.exhaustion_transitions.is_empty()
    {
        return Ok(Vec::new());
    }
    let store = state.store()?;
    let credentials = state.cached_account_credential_facts(store.accounts())?;
    let mut identities = Vec::new();
    let now_ms = current_time_ms();
    for reader in store.accounts() {
        let account = &reader.account;
        if account.id == response.account.account.id
            || account.quota.reset_credits_available == Some(0)
            || !subscription_allows_reset(&account.subscription, now_ms)
            || !weekly_reset_window(&account.quota, now_ms).is_some_and(is_weekly_exhausted)
        {
            continue;
        }
        if let Some(selected) =
            weekly_reset_member(store.gateway(), store.accounts(), reader, &credentials)
            && has_weekly_reserve_rule(&store.automations().tasks, reader, selected)
        {
            let (_, fence) = store.account_refresh_scope(&account.id)?;
            identities.push(fence.identity());
        }
    }
    Ok(identities)
}

fn is_weekly_exhausted(window: &QuotaWindow) -> bool {
    window.is_exhausted() && window.explicitly_full != Some(true)
}

fn fresh_weekly_exhaustion(
    response: &AccountQuotaRefreshResponse,
    now_ms: u64,
) -> Option<&QuotaWindow> {
    if !matches!(response.quota, AccountQuotaOutcome::Updated { .. }) {
        return None;
    }
    fresh_weekly_reset_window(&response.account.account.quota, now_ms)
        .filter(|w| is_weekly_exhausted(w))
}

fn weekly_exhaustion_fingerprint(
    response: &AccountQuotaRefreshResponse,
    now_ms: u64,
) -> Option<String> {
    let window = fresh_weekly_exhaustion(response, now_ms)?;
    window
        .exhaustion_transition_fingerprint
        .clone()
        .or_else(|| {
            response
                .exhaustion_transitions
                .iter()
                .find(|transition| transition.window_kind == QuotaWindowKind::Secondary)
                .map(|transition| transition.fingerprint.clone())
        })
        .or_else(|| {
            window
                .exhaustion_transition()
                .map(|event| event.fingerprint)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_pool::refresh::tests::{account, cleanup, state};
    use zenith_relay_core::automations::{
        AccountSelector, WakeExecutionPolicy, WakeModelPolicy, WakeTrigger,
    };

    #[tokio::test]
    async fn countdown_recovers_a_full_primary_window_without_a_new_transition() {
        let state = state();
        let mut account = account();
        account.account.in_pool = true;
        account.account.quota.primary = Some(QuotaWindow {
            kind: QuotaWindowKind::Primary,
            provider_cycle_id: None,
            window_start_ms: None,
            available_basis_points: Some(10_000),
            explicitly_full: Some(true),
            reset_at_ms: None,
            window_minutes: Some(300),
            observed_at_ms: current_time_ms(),
            full_transition_fingerprint: Some("full-before-automation".into()),
            exhaustion_transition_fingerprint: None,
        });
        state
            .store()
            .unwrap()
            .upsert_account(account.clone())
            .unwrap();
        let mut records = state.store().unwrap().automations().clone();
        records.tasks.push(WakeTask {
            id: "wake_recovery".into(),
            name: "Synthetic countdown".into(),
            enabled: true,
            account_selector: AccountSelector::AllEligible,
            window_kinds: [QuotaWindowKind::Primary].into(),
            model_policy: WakeModelPolicy::Explicit("gpt-test".into()),
            trigger: WakeTrigger::QuotaFull,
            weekly_reset_remaining_basis_points: None,
            fallback_schedule: None,
            execution_policy: WakeExecutionPolicy::Automatic,
            jitter_seconds: 0,
            max_attempts_per_cycle: 1,
            created_at_ms: 1,
            updated_at_ms: 1,
        });
        state.store().unwrap().replace_automations(records).unwrap();
        let response = AccountQuotaRefreshResponse {
            account,
            quota: AccountQuotaOutcome::Updated {
                transitions: Vec::new(),
                exhaustion_transitions: Vec::new(),
            },
            exhaustion_transitions: Vec::new(),
        };

        evaluate_updated_transitions(&state, &response).unwrap();
        assert!(state.next_automatic_wake_due().unwrap().is_some());
        evaluate_updated_transitions(&state, &response).unwrap();
        let permits = state
            .claim_due_automatic_wakes(current_time_ms(), 10)
            .unwrap();
        assert_eq!(permits.len(), 1);
        state
            .complete_wake(
                permits.into_iter().next().unwrap(),
                zenith_relay_core::automations::WakeCompletion {
                    outcome: zenith_relay_core::automations::WakeCompletionOutcome::Confirmed,
                    completed_at_ms: current_time_ms(),
                    latency_ms: None,
                    input_tokens: None,
                    output_tokens: None,
                    error_code: None,
                },
            )
            .unwrap();
        evaluate_updated_transitions(&state, &response).unwrap();
        assert!(state.next_automatic_wake_due().unwrap().is_none());
        cleanup(state).await;
    }

    #[test]
    fn countdown_recovery_preserves_cycle_identity_and_requires_successful_full_quota() {
        let mut response = AccountQuotaRefreshResponse {
            account: account(),
            quota: AccountQuotaOutcome::Updated {
                transitions: Vec::new(),
                exhaustion_transitions: Vec::new(),
            },
            exhaustion_transitions: Vec::new(),
        };
        response.account.account.quota.primary = Some(QuotaWindow {
            kind: QuotaWindowKind::Primary,
            provider_cycle_id: None,
            window_start_ms: Some(100),
            available_basis_points: Some(10_000),
            explicitly_full: Some(true),
            reset_at_ms: Some(18_000_100),
            window_minutes: Some(300),
            observed_at_ms: 200,
            full_transition_fingerprint: Some("stable-cycle".into()),
            exhaustion_transition_fingerprint: None,
        });
        let candidates = quota_full_candidates(&response);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].fingerprint, "stable-cycle");
        assert_eq!(candidates[0].transitioned_at_ms, 100);

        response.quota = AccountQuotaOutcome::Updated {
            transitions: vec![QuotaTransition {
                window_kind: QuotaWindowKind::Primary,
                fingerprint: "provider-edge".into(),
                transitioned_at_ms: 150,
            }],
            exhaustion_transitions: Vec::new(),
        };
        assert_eq!(
            quota_full_candidates(&response)[0].fingerprint,
            "provider-edge"
        );
        response.quota = AccountQuotaOutcome::Failed {
            code: "quota_transport".into(),
            retryable: true,
        };
        assert!(quota_full_candidates(&response).is_empty());
        response.quota = AccountQuotaOutcome::Skipped;
        assert!(quota_full_candidates(&response).is_empty());
        response.quota = AccountQuotaOutcome::Updated {
            transitions: Vec::new(),
            exhaustion_transitions: Vec::new(),
        };
        let window = response.account.account.quota.primary.as_mut().unwrap();
        window.available_basis_points = Some(9_000);
        window.explicitly_full = Some(false);
        assert!(quota_full_candidates(&response).is_empty());
    }

    #[test]
    fn weekly_exhaustion_recovers_a_missed_secondary_transition() {
        let mut account = account();
        account.account.quota.reset_credits_available = None;
        account.account.quota.secondary = Some(QuotaWindow {
            kind: QuotaWindowKind::Secondary,
            provider_cycle_id: Some("weekly-cycle".into()),
            window_start_ms: Some(1_000),
            available_basis_points: Some(0),
            explicitly_full: Some(false),
            reset_at_ms: Some(3_601_000),
            window_minutes: Some(10_080),
            observed_at_ms: 1_000,
            full_transition_fingerprint: None,
            exhaustion_transition_fingerprint: Some("weekly-fingerprint".into()),
        });
        let mut response = AccountQuotaRefreshResponse {
            account,
            quota: AccountQuotaOutcome::Updated {
                transitions: Vec::new(),
                exhaustion_transitions: Vec::new(),
            },
            exhaustion_transitions: Vec::new(),
        };

        assert_eq!(
            weekly_exhaustion_fingerprint(&response, 1_000).as_deref(),
            Some("weekly-fingerprint")
        );
        response.quota = AccountQuotaOutcome::Skipped;
        assert!(weekly_exhaustion_fingerprint(&response, 1_000).is_none());
    }

    #[test]
    fn weekly_exhaustion_uses_the_provider_edge_if_no_fingerprint_was_saved() {
        let mut response = AccountQuotaRefreshResponse {
            account: account(),
            quota: AccountQuotaOutcome::Updated {
                transitions: vec![],
                exhaustion_transitions: vec![],
            },
            exhaustion_transitions: vec![],
        };
        response
            .exhaustion_transitions
            .push(zenith_relay_core::quota::QuotaTransition {
                window_kind: QuotaWindowKind::Secondary,
                fingerprint: "provider-fingerprint".into(),
                transitioned_at_ms: 200,
            });
        response.account.account.quota.secondary = Some(QuotaWindow {
            kind: QuotaWindowKind::Secondary,
            provider_cycle_id: None,
            window_start_ms: None,
            available_basis_points: Some(0),
            explicitly_full: Some(false),
            reset_at_ms: Some(60_300),
            window_minutes: Some(10_080),
            observed_at_ms: 300,
            full_transition_fingerprint: None,
            exhaustion_transition_fingerprint: None,
        });

        assert_eq!(
            weekly_exhaustion_fingerprint(&response, 300).as_deref(),
            Some("provider-fingerprint")
        );
    }
}
