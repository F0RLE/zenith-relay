use super::*;
mod admission;
mod affinity;
mod automatic;
mod capacity;
mod live_order;
mod policy;
mod preview;
mod recovery;
mod rotation;
use crate::scheduler::{CandidateKind, CandidateQuota};
use crate::ModelRules;
use std::collections::{BTreeSet, HashSet};

fn candidate(id: &str) -> RuntimeCandidate {
    RuntimeCandidate {
        id: id.to_string(),
        kind: CandidateKind::ApiSource,
        source_id: id.to_string(),
        account_id: None,
        protocol: WireApi::Responses,
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
