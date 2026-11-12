use super::*;
mod admission;
mod affinity;
mod automatic;
mod cache_owner;
mod capacity;
mod live_order;
mod policy;
mod preview;
mod provider_backoff;
mod rate_limits;
mod recovery;
mod rotation;
mod warm_fallback;

#[test]
fn spending_evidence_is_account_scoped_and_cannot_survive_removal() {
    let mut scheduler = PoolScheduler::new();
    scheduler.upsert(oauth_candidate("account"));
    scheduler.upsert(candidate("source"));
    let deadline = crate::quota::QuotaSpendDeadline {
        expires_at_ms: 5_000,
        observed_at_ms: 100,
        valid_until_ms: 1_000,
    };
    assert!(scheduler.set_spend_deadlines(BTreeMap::from([
        ("account".into(), deadline),
        ("source".into(), deadline),
        ("missing".into(), deadline),
    ])));
    assert_eq!(scheduler.spend_deadlines.len(), 1);
    assert_eq!(
        scheduler
            .rotation_candidate(&oauth_candidate("account"))
            .unwrap()
            .spend_deadline,
        Some(deadline)
    );
    scheduler.upsert(candidate("account"));
    assert!(
        scheduler.spend_deadlines.is_empty(),
        "a replaced identity must not inherit an entitlement deadline"
    );
    scheduler.upsert(oauth_candidate("account"));
    assert!(scheduler.spend_deadlines.is_empty());
    scheduler.set_spend_deadlines(BTreeMap::from([("account".into(), deadline)]));
    scheduler.remove("account");
    scheduler.upsert(oauth_candidate("account"));
    assert!(scheduler.spend_deadlines.is_empty());
    assert_eq!(
        scheduler
            .rotation_candidate(&oauth_candidate("account"))
            .unwrap()
            .spend_deadline,
        None
    );
}
use crate::ModelRules;
use crate::scheduler::{CandidateKind, CandidateQuota};
use std::collections::{BTreeSet, HashSet};

fn candidate(id: &str) -> RuntimeCandidate {
    RuntimeCandidate {
        id: id.to_string(),
        kind: CandidateKind::ApiSource,
        source_id: id.to_string(),
        account_id: None,
        protocol: WireApi::Responses,
        model_protocols: BTreeMap::new(),
        enabled: true,
        draining: false,
        priority: 0,
        weight: 1,
        models: ["gpt-5".to_string()].into(),
        model_rules: ModelRules::default(),
        health: CandidateHealth::Healthy,
        quota: CandidateQuota::Unknown,
        provider_credits_micro_units: None,
        provider_credits_unlimited: false,
        provider_credits_observed_at_ms: None,
        quota_updated_at_ms: None,
        quota_reset_at_ms: None,
        cooldowns: BTreeMap::new(),
        last_used_at: None,

        secret_available: true,
    }
}

fn oauth_candidate(id: &str) -> RuntimeCandidate {
    RuntimeCandidate {
        kind: CandidateKind::OAuthAccount,
        account_id: Some(id.to_string()),
        ..candidate(id)
    }
}

fn select(scheduler: &mut PoolScheduler, tried: &HashSet<String>) -> Option<Selection> {
    scheduler.select(SelectionRequest {
        model: "gpt-5",
        allowed_protocols: &[WireApi::Responses, WireApi::ChatCompletions],
        scope: &CandidateScope::default(),
        tried,
        response_affinity_key: None,
        prompt_affinity_key: None,
        now_ms: 100,
    })
}

fn select_image(scheduler: &mut PoolScheduler, tried: &HashSet<String>) -> Option<Selection> {
    scheduler.select_image(SelectionRequest {
        model: "gpt-image-2",
        allowed_protocols: &[WireApi::Responses, WireApi::ChatCompletions],
        scope: &CandidateScope::default(),
        tried,
        response_affinity_key: None,
        prompt_affinity_key: None,
        now_ms: 100,
    })
}
