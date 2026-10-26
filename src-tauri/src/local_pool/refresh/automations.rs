//! Quota job finalization. Only the owning read schedules wake/reset work;
//! followers and canceled UI requests cannot repeat side effects.
use crate::local_pool::{
    accounts::{
        quota_refresh::{AccountQuotaOutcome, AccountQuotaRefreshResponse},
        reset_credits::consume_reset_credit_for_scope,
    },
    background::codex_wake_policy,
    error::{ErrorCode, LocalPoolError, Result},
    runtime::current_time_ms,
    state::DesktopState,
    store::AccountRefreshFence,
};
use zenith_relay_core::{
    automations::WakeTrigger,
    providers::chatgpt::CodexQuotaClient,
    quota::{QuotaTransition, QuotaWindowKind},
};

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
) -> Result<bool> {
    if response.account.recovery.is_some()
        || response
            .account
            .account
            .quota
            .reset_credits_available
            .is_some_and(|available| available == 0)
    {
        return Ok(false);
    }
    let tasks = {
        let store = state.store()?;
        store.ensure_account_refresh_current(fence)?;
        if store
            .account(&fence.account_id)
            .is_none_or(|latest| latest.account.quota != response.account.account.quota)
        {
            return Ok(false);
        }
        store.automations().tasks.clone()
    };
    let has_weekly_task = tasks.iter().any(|task| {
        task.enabled
            && task.trigger == WakeTrigger::Weekly
            && task.account_selector.matches(&response.account.account)
    });
    if !has_weekly_task {
        return Ok(false);
    }
    let transitions = weekly_exhaustion_candidates(response);
    for transition in &transitions {
        if transition.window_kind != zenith_relay_core::quota::QuotaWindowKind::Secondary
            || state
                .weekly_reset_was_applied(&response.account.account.id, &transition.fingerprint)?
        {
            continue;
        }
        if consume_reset_credit_for_scope(state, fence).await.is_ok() {
            let _mutation = state.setup_guard().await;
            state.store()?.ensure_account_refresh_current(fence)?;
            state
                .mark_weekly_reset_applied(&response.account.account.id, &transition.fingerprint)?;
            return Ok(true);
        }
    }
    Ok(false)
}

fn weekly_exhaustion_candidates(
    response: &AccountQuotaRefreshResponse,
) -> Vec<zenith_relay_core::quota::QuotaTransition> {
    if matches!(response.quota, AccountQuotaOutcome::Skipped) {
        return Vec::new();
    }
    let mut transitions = response.exhaustion_transitions.clone();
    if !transitions.iter().any(|transition| {
        transition.window_kind == zenith_relay_core::quota::QuotaWindowKind::Secondary
    }) && let Some(transition) = response
        .account
        .account
        .quota
        .secondary
        .as_ref()
        .and_then(zenith_relay_core::quota::QuotaWindow::exhaustion_transition)
    {
        transitions.push(transition);
    }
    transitions
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_pool::refresh::tests::{account, cleanup, state};
    use zenith_relay_core::{
        automations::{AccountSelector, WakeExecutionPolicy, WakeModelPolicy, WakeTask},
        quota::{QuotaWindow, QuotaWindowKind},
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
    fn weekly_exhaustion_candidates_recover_a_missed_secondary_transition() {
        let mut account = account();
        account.account.quota.reset_credits_available = None;
        account.account.quota.secondary = Some(QuotaWindow {
            kind: QuotaWindowKind::Secondary,
            provider_cycle_id: Some("weekly-cycle".into()),
            window_start_ms: Some(1_000),
            available_basis_points: Some(0),
            explicitly_full: Some(false),
            reset_at_ms: Some(3_601_000),
            window_minutes: Some(60),
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

        let candidates = weekly_exhaustion_candidates(&response);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].window_kind, QuotaWindowKind::Secondary);
        assert_eq!(candidates[0].fingerprint, "weekly-fingerprint");
        response.quota = AccountQuotaOutcome::Skipped;
        assert!(weekly_exhaustion_candidates(&response).is_empty());
    }

    #[test]
    fn weekly_exhaustion_candidates_keep_provider_transition_identity() {
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
            reset_at_ms: None,
            window_minutes: Some(60),
            observed_at_ms: 300,
            full_transition_fingerprint: None,
            exhaustion_transition_fingerprint: Some("derived-fingerprint".into()),
        });

        let candidates = weekly_exhaustion_candidates(&response);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].fingerprint, "provider-fingerprint");
    }
}
