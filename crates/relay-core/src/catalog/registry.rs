use crate::scheduler::{CandidateScope, PoolScheduler};
use crate::WireApi;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Default)]
pub struct ModelRegistry {
    models_by_candidate: BTreeMap<String, BTreeSet<String>>,
}

impl ModelRegistry {
    pub fn replace<I, S>(&mut self, candidate_id: impl Into<String>, models: I)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let candidate_id = candidate_id.into();
        let models = normalized_models(models);
        if models.is_empty() {
            self.models_by_candidate.remove(&candidate_id);
        } else {
            self.models_by_candidate.insert(candidate_id, models);
        }
    }

    pub fn remove(&mut self, candidate_id: &str) -> bool {
        self.models_by_candidate.remove(candidate_id).is_some()
    }

    pub fn visible_models(
        &self,
        scheduler: &PoolScheduler,
        scope: &CandidateScope,
        allowed_protocols: &[WireApi],
        _now_ms: u64,
    ) -> Vec<String> {
        let mut visible = BTreeMap::new();
        for (candidate_id, models) in &self.models_by_candidate {
            let Some(candidate) = scheduler.candidate(candidate_id) else {
                continue;
            };
            for model in models {
                if candidate.is_catalog_visible(model, allowed_protocols, scope) {
                    visible
                        .entry(model.to_ascii_lowercase())
                        .or_insert_with(|| model.clone());
                }
            }
        }
        visible.into_values().collect()
    }
}

fn normalized_models<I, S>(models: I) -> BTreeSet<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut normalized = BTreeMap::new();
    for model in models {
        let model = model.as_ref().trim();
        if !model.is_empty() {
            normalized
                .entry(model.to_ascii_lowercase())
                .or_insert_with(|| model.to_string());
        }
    }
    normalized.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::{CandidateHealth, CandidateKind, CandidateQuota, RuntimeCandidate};
    use crate::ModelRules;

    fn candidate(id: &str, source_id: &str) -> RuntimeCandidate {
        RuntimeCandidate {
            id: id.to_string(),
            kind: CandidateKind::ApiSource,
            source_id: source_id.to_string(),
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
            cooldowns: BTreeMap::new(),
            last_used_at: None,
            consecutive_failures: 0,
            secret_available: true,
        }
    }

    #[test]
    fn model_visibility_ignores_cooldown_but_respects_scope_and_protocol() {
        let mut scheduler = PoolScheduler::new(4, 100);
        let mut cooled = candidate("cooled", "source-a");
        cooled.cooldowns.insert("gpt-5".to_string(), 200);
        scheduler.upsert(cooled);
        scheduler.upsert(candidate("ready", "source-b"));

        let mut registry = ModelRegistry::default();
        registry.replace("cooled", ["gpt-5"]);
        registry.replace("ready", ["GPT-5", ""]);

        assert_eq!(
            registry.visible_models(
                &scheduler,
                &CandidateScope::default(),
                &[WireApi::Responses],
                100,
            ),
            vec!["gpt-5"]
        );

        let scope = CandidateScope {
            source_ids: Some(["source-a".to_string()].into()),
            ..CandidateScope::default()
        };
        assert_eq!(
            registry.visible_models(&scheduler, &scope, &[WireApi::Responses], 100),
            vec!["gpt-5"]
        );
        assert!(registry
            .visible_models(
                &scheduler,
                &CandidateScope::default(),
                &[WireApi::Messages],
                200,
            )
            .is_empty());
    }

    #[test]
    fn empty_snapshot_unregisters_candidate_models() {
        let mut scheduler = PoolScheduler::new(1, 100);
        scheduler.upsert(candidate("ready", "source-a"));
        let mut registry = ModelRegistry::default();
        registry.replace("ready", ["gpt-5"]);
        registry.replace("ready", [""]);

        assert!(registry
            .visible_models(
                &scheduler,
                &CandidateScope::default(),
                &[WireApi::Responses],
                0,
            )
            .is_empty());
    }

    #[test]
    fn exhausted_quota_keeps_catalog_visible_but_unhealthy_accounts_do_not() {
        let mut scheduler = PoolScheduler::new(2, 100);
        let mut exhausted = candidate("exhausted", "source-a");
        exhausted.quota = CandidateQuota::Exhausted;
        scheduler.upsert(exhausted);
        let mut unhealthy = candidate("unhealthy", "source-b");
        unhealthy.health = CandidateHealth::Unhealthy;
        scheduler.upsert(unhealthy);

        let mut registry = ModelRegistry::default();
        registry.replace("exhausted", ["gpt-5"]);
        registry.replace("unhealthy", ["gpt-private"]);

        assert_eq!(
            registry.visible_models(
                &scheduler,
                &CandidateScope::default(),
                &[WireApi::Responses],
                100,
            ),
            vec!["gpt-5"]
        );
    }
}
