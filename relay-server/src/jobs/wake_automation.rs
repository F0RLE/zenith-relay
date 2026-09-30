use crate::{
    app::{account_proxy_config, prepare_server_account_authorization},
    state::{now_ms, AccountCredential, AppState, ServerAccountRecord},
};
use futures_util::StreamExt;
use reqwest::{
    header::{HeaderValue, AUTHORIZATION},
    redirect::Policy,
};
use std::{
    collections::BTreeSet,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{sync::watch, task::JoinHandle};
use zenith_relay_core::error_codes;
use zenith_relay_core::{
    accounts::{AccountAuthMode, AccountIdentity, AccountRecord},
    automations::{
        model_lightness_rank, verify_wake_countdown, WakeAdapterPolicy, WakeCompletion,
        WakeCompletionOutcome, WakeCoordinator, WakeModel, WakePermit,
    },
    providers::chatgpt::CodexIdentityEnvelope,
    quota::{QuotaTransition, QuotaWindowKind},
    ModelRules,
};

const INTERVAL: Duration = Duration::from_secs(30);
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const WAKE_PROMPT: &str = "Reply with OK.";

pub fn start(state: Arc<AppState>, shutdown: watch::Receiver<bool>) -> JoinHandle<()> {
    super::start_periodic(state, shutdown, INTERVAL, |state| async move {
        let _ = run_due(&state).await;
    })
}

pub async fn schedule_transitions(
    state: &Arc<AppState>,
    account: &ServerAccountRecord,
    transitions: &[QuotaTransition],
) -> Result<(), String> {
    let _guard = state.wake_lock.lock().await;
    let tasks = state.store.wake_tasks()?;
    let mut coordinator =
        WakeCoordinator::from_state(state.store.wake_state()?).map_err(str::to_string)?;
    let account_record = core_account(account)?;
    let policy = policy(account);
    for task in tasks {
        for transition in transitions {
            let _ = coordinator.evaluate(
                &task,
                &account_record,
                transition,
                account.last_used_at_ms,
                &policy,
                now_ms(),
            );
        }
    }
    state.store.save_wake_state(coordinator.state())
}

async fn run_due(state: &Arc<AppState>) -> Result<(), String> {
    let permits = {
        let _guard = state.wake_lock.lock().await;
        let mut coordinator =
            WakeCoordinator::from_state(state.store.wake_state()?).map_err(str::to_string)?;
        let permits = coordinator.claim_due_automatic(now_ms(), 2);
        state.store.save_wake_state(coordinator.state())?;
        permits
    };
    for permit in permits {
        let completion = delivery::execute(state, &permit).await;
        let _guard = state.wake_lock.lock().await;
        let mut coordinator =
            WakeCoordinator::from_state(state.store.wake_state()?).map_err(str::to_string)?;
        coordinator.complete(permit, completion);
        state.store.save_wake_state(coordinator.state())?;
    }
    Ok(())
}

mod delivery;

pub(crate) fn core_account(account: &ServerAccountRecord) -> Result<AccountRecord, String> {
    let identity = AccountIdentity::from_hashed_parts(
        "openai_codex",
        "chatgpt.com",
        &account.identity_hint,
        &account.identity_hint,
        "remote",
        None,
    )
    .map_err(str::to_string)?;
    Ok(AccountRecord {
        id: account.id.clone(),
        label: account.label.clone(),
        identity,
        auth_mode: AccountAuthMode::OAuth,
        auth_state: account.auth_state,
        health: account.health,
        source_id: account.source_id.clone(),
        secret_refs: vec![account.secret_ref.clone()],
        subscription: account.subscription.clone(),
        quota: account.quota.clone(),
        token_generation: 0,
        token_updated_at_ms: None,
        tags: BTreeSet::new(),
        enabled: account.enabled,
        in_pool: account.in_pool,
        draining: account.draining,
        created_at_ms: 0,
        last_used_at_ms: account.last_used_at_ms,
        last_error_code: account.last_error_code.clone(),
    })
}

fn policy(account: &ServerAccountRecord) -> WakeAdapterPolicy {
    let rules = ModelRules::from_allow_deny(&account.allowed_models, &account.excluded_models);
    WakeAdapterPolicy {
        windows_requiring_activity: BTreeSet::from([
            QuotaWindowKind::Primary,
            QuotaWindowKind::Secondary,
        ]),
        models: account
            .effective_models()
            .iter()
            .filter(|id| rules.allows(id))
            .enumerate()
            .map(|(index, id)| WakeModel {
                id: id.clone(),
                lightness_rank: model_lightness_rank(id, index),
                wake_capable: true,
            })
            .collect(),
        verification_delay_ms: 5_000,
        output_token_cap: 8,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zenith_relay_core::{
        accounts::{AccountAuthState, AccountHealthState, ReauthReason},
        quota::Subscription,
    };

    #[test]
    fn core_account_keeps_secret_refs_out_of_debug_identity() {
        let account = sample_account();
        let mapped = core_account(&account).unwrap();
        assert_eq!(mapped.id, "account_test");
        assert!(!format!("{:?}", mapped.identity).contains("account:synthetic"));
    }

    #[test]
    fn wake_models_follow_pool_allow_and_exclude_rules() {
        let mut account = sample_account();
        account.models = vec![
            "gpt-codex".into(),
            "gpt-excluded".into(),
            "claude-sonnet".into(),
        ];
        account.allowed_models = vec!["gpt-*".into()];
        account.excluded_models = vec!["gpt-excluded".into()];

        let ids = policy(&account)
            .models
            .into_iter()
            .map(|model| model.id)
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["gpt-codex".to_string()]);
    }

    fn sample_account() -> ServerAccountRecord {
        ServerAccountRecord {
            id: "account_test".into(),
            label: "Test".into(),
            identity_hint: "abcdef123456".into(),
            enabled: true,
            in_pool: true,
            draining: false,
            source_id: "openai_codex".into(),
            secret_ref: "account:synthetic".into(),
            provider_family: Some("openai".into()),
            auth_state: AccountAuthState::RequiresReauth(ReauthReason::InvalidGrant),
            health: AccountHealthState::Unhealthy,
            models: vec!["gpt-test".into()],
            discovered_models: None,
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
            priority: 0,
            weight: 1,
            subscription: Subscription::default(),
            quota: Default::default(),
            purchase_cost_micro_usd: None,
            cooldowns: Default::default(),
            consecutive_failures: 0,
            created_at_ms: 1,
            last_used_at_ms: None,
            last_error_code: None,
            proxy_id: None,
            bypass_common_proxy: false,
        }
    }
}
