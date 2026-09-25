//! Quota job finalization. Only the owning read schedules wake/reset work;
//! followers and canceled UI requests cannot repeat side effects.
use crate::local_pool::{
    accounts::{
        quota_refresh::{AccountQuotaOutcome, AccountQuotaRefreshResponse},
        reset_credits::consume_reset_credit_for_scope,
    },
    background::codex_wake_policy,
    commands::current_time_ms,
    error::{ErrorCode, LocalPoolError, Result},
    state::DesktopState,
    store::AccountRefreshFence,
};
use zenith_relay_core::{automations::WakeTrigger, providers::chatgpt::CodexQuotaClient};

pub(super) fn evaluate_updated_transitions(
    state: &DesktopState,
    response: &AccountQuotaRefreshResponse,
) -> Result<()> {
    let AccountQuotaOutcome::Updated { transitions, .. } = &response.quota else {
        return Ok(());
    };
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
    for transition in transitions {
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

pub(super) async fn evaluate_weekly_exhaustions(
    state: &DesktopState,
    response: &AccountQuotaRefreshResponse,
    fence: &AccountRefreshFence,
) -> Result<bool> {
    if response.account.remote_location.is_some()
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
    }) {
        if let Some(transition) = response
            .account
            .account
            .quota
            .secondary
            .as_ref()
            .and_then(zenith_relay_core::quota::QuotaWindow::exhaustion_transition)
        {
            transitions.push(transition);
        }
    }
    transitions
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_pool::refresh::tests::account;
    use zenith_relay_core::quota::{QuotaWindow, QuotaWindowKind};
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
