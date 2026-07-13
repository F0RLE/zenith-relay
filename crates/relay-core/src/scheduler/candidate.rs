use super::capacity::CandidateQuota;
use super::cooldown::active_retry_at;
use crate::{ModelRules, WireApi};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateKind {
    ApiSource,
    OAuthAccount,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateHealth {
    #[default]
    Unknown,
    Healthy,
    Degraded,
    Unhealthy,
    ReauthRequired,
    Checkpoint,
    Captcha,
    Blocked,
    Expired,
}

impl CandidateHealth {
    fn is_eligible(self) -> bool {
        matches!(self, Self::Unknown | Self::Healthy | Self::Degraded)
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidateScope {
    pub source_ids: Option<BTreeSet<String>>,
    pub account_ids: Option<BTreeSet<String>>,
    pub model_rules: ModelRules,
}

impl CandidateScope {
    fn includes(&self, candidate: &RuntimeCandidate) -> bool {
        if self.source_ids.is_none() && self.account_ids.is_none() {
            return true;
        }
        self.source_ids
            .as_ref()
            .is_some_and(|ids| ids.contains(&candidate.source_id))
            || candidate.account_id.as_ref().is_some_and(|id| {
                self.account_ids
                    .as_ref()
                    .is_some_and(|ids| ids.contains(id))
            })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeCandidate {
    pub id: String,
    pub kind: CandidateKind,
    pub source_id: String,
    pub account_id: Option<String>,
    pub protocol: WireApi,
    pub enabled: bool,
    pub draining: bool,
    pub priority: i32,
    pub weight: u32,
    pub models: BTreeSet<String>,
    pub model_rules: ModelRules,
    pub health: CandidateHealth,
    pub quota: CandidateQuota,
    pub cooldowns: BTreeMap<String, u64>,
    pub last_used_at: Option<u64>,
    pub consecutive_failures: u32,
    pub secret_available: bool,
}

impl RuntimeCandidate {
    pub fn is_eligible(
        &self,
        model: &str,
        allowed_protocols: &[WireApi],
        scope: &CandidateScope,
        now_ms: u64,
    ) -> bool {
        self.is_visible(model, allowed_protocols, scope)
            && active_retry_at(&self.cooldowns, model, now_ms).is_none()
    }

    pub fn is_visible(
        &self,
        model: &str,
        allowed_protocols: &[WireApi],
        scope: &CandidateScope,
    ) -> bool {
        self.is_configured(model, allowed_protocols, scope)
            && self.health.is_eligible()
            && self.quota.is_eligible()
    }

    pub(crate) fn is_configured(
        &self,
        model: &str,
        allowed_protocols: &[WireApi],
        scope: &CandidateScope,
    ) -> bool {
        self.enabled
            && !self.draining
            && self.secret_available
            && allowed_protocols.contains(&self.protocol)
            && self.supports_model(model)
            && self.model_rules.allows(model)
            && scope.includes(self)
            && scope.model_rules.allows(model)
    }

    pub fn retry_at_if_visible(
        &self,
        model: &str,
        allowed_protocols: &[WireApi],
        scope: &CandidateScope,
        now_ms: u64,
    ) -> Option<u64> {
        self.is_visible(model, allowed_protocols, scope)
            .then(|| active_retry_at(&self.cooldowns, model, now_ms))
            .flatten()
    }

    fn supports_model(&self, model: &str) -> bool {
        self.models
            .iter()
            .any(|candidate_model| candidate_model.eq_ignore_ascii_case(model))
    }
}
