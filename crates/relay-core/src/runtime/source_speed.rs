use super::GatewayRuntime;
use crate::scheduler::CandidateScope;
use crate::DefaultServiceTier;
use serde_json::Value;

impl GatewayRuntime {
    /// A service tier is an upstream entitlement, not a model-name heuristic.
    /// The management surface consults confirmed metadata for an eligible
    /// route, so an unavailable account, cooling route, or generic API spelling
    /// cannot fabricate a speed control.
    pub fn model_supports_service_tier(&self, model: &str, tier: DefaultServiceTier) -> bool {
        self.model_supported_service_tiers(model).contains(&tier)
    }

    /// Returns the exact service tiers confirmed by at least one currently
    /// eligible route for this model. Standard is included whenever the model
    /// has an active route; Fast and Ultrafast require matching upstream
    /// catalog evidence.
    pub(crate) fn model_supported_service_tiers(&self, model: &str) -> Vec<DefaultServiceTier> {
        let model = model.trim();
        if model.is_empty() {
            return Vec::new();
        }
        let candidates = self.current_candidate_ids_for_model(model);
        let mut tiers = Vec::with_capacity(3);
        for tier in [
            DefaultServiceTier::Standard,
            DefaultServiceTier::Fast,
            DefaultServiceTier::Ultrafast,
        ] {
            if candidates
                .iter()
                .any(|candidate_id| self.candidate_supports_service_tier(candidate_id, model, tier))
            {
                tiers.push(tier);
            }
        }
        tiers
    }

    /// Fast is kept as a named compatibility helper for existing management
    /// clients and integrations.
    pub fn model_supports_fast_service_tier(&self, model: &str) -> bool {
        self.model_supports_service_tier(model, DefaultServiceTier::Fast)
    }

    /// A model can be shared by several routes. A confirmed tier on one route
    /// must not authorize that request on another route that did not advertise
    /// it.
    pub(crate) fn candidate_supports_service_tier(
        &self,
        candidate_id: &str,
        model: &str,
        tier: DefaultServiceTier,
    ) -> bool {
        let model = model.trim();
        if candidate_id.trim().is_empty() || model.is_empty() {
            return false;
        }
        let has_active_model_route =
            self.lock_scheduler()
                .candidate(candidate_id)
                .is_some_and(|candidate| {
                    candidate.is_eligible(
                        model,
                        &[candidate.protocol],
                        &CandidateScope::default(),
                        crate::unix_time_ms(),
                    )
                });
        if !has_active_model_route {
            return false;
        }
        if tier == DefaultServiceTier::Standard {
            return true;
        }
        let manifest = if self.source_candidate_bindings.contains_key(candidate_id) {
            self.model_metadata
                .source_manifests
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(candidate_id)
                .cloned()
        } else if self.chatgpt_accounts.contains_key(candidate_id) {
            self.model_metadata
                .codex_manifests
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(candidate_id)
                .cloned()
        } else {
            None
        };
        manifest
            .is_some_and(|manifest| manifest_supports_service_tier(&manifest.value, model, tier))
    }

    #[cfg(test)]
    pub(crate) fn candidate_supports_fast_service_tier(
        &self,
        candidate_id: &str,
        model: &str,
    ) -> bool {
        self.candidate_supports_service_tier(candidate_id, model, DefaultServiceTier::Fast)
    }

    /// Selects the fastest tier that this exact candidate has advertised for
    /// the requested model. A global Ultrafast preference may therefore fall
    /// back to Fast on a route that has Fast evidence but no Ultrafast grant.
    pub(crate) fn effective_service_tier_for_candidate(
        &self,
        candidate_id: &str,
        model: &str,
        requested: DefaultServiceTier,
    ) -> DefaultServiceTier {
        match requested {
            DefaultServiceTier::Standard => DefaultServiceTier::Standard,
            DefaultServiceTier::Fast => {
                if self.candidate_supports_service_tier(
                    candidate_id,
                    model,
                    DefaultServiceTier::Fast,
                ) {
                    DefaultServiceTier::Fast
                } else {
                    DefaultServiceTier::Standard
                }
            }
            DefaultServiceTier::Ultrafast => {
                if self.candidate_supports_service_tier(
                    candidate_id,
                    model,
                    DefaultServiceTier::Ultrafast,
                ) {
                    DefaultServiceTier::Ultrafast
                } else if self.candidate_supports_service_tier(
                    candidate_id,
                    model,
                    DefaultServiceTier::Fast,
                ) {
                    DefaultServiceTier::Fast
                } else {
                    DefaultServiceTier::Standard
                }
            }
        }
    }

    /// Projects the requested tier across all currently eligible routes for
    /// management clients. The highest confirmed tier is shown so the UI can
    /// explain whether Ultrafast is actually available for this model.
    pub(crate) fn project_service_tier_for_model(
        &self,
        model: &str,
        requested: DefaultServiceTier,
    ) -> DefaultServiceTier {
        self.current_candidate_ids_for_model(model)
            .into_iter()
            .map(|candidate_id| {
                self.effective_service_tier_for_candidate(&candidate_id, model, requested)
            })
            .max_by_key(|tier| service_tier_rank(*tier))
            .unwrap_or(DefaultServiceTier::Standard)
    }

    fn current_candidate_ids_for_model(&self, model: &str) -> Vec<String> {
        let scheduler = self.lock_scheduler();
        scheduler
            .candidates()
            .filter(|candidate| {
                candidate
                    .models
                    .iter()
                    .any(|candidate_model| candidate_model.eq_ignore_ascii_case(model))
            })
            .map(|candidate| candidate.id.clone())
            .collect()
    }
}

fn manifest_supports_service_tier(
    manifest: &Value,
    model: &str,
    requested: DefaultServiceTier,
) -> bool {
    let Some(models) = manifest
        .get("models")
        .or_else(|| manifest.get("data"))
        .and_then(Value::as_array)
    else {
        return false;
    };
    models.iter().any(|entry| {
        let Some(object) = entry.as_object() else {
            return false;
        };
        let id = object
            .get("slug")
            .or_else(|| object.get("id"))
            .and_then(Value::as_str)
            .map(str::trim);
        if !id.is_some_and(|id| id.eq_ignore_ascii_case(model)) {
            return false;
        }
        object
            .get("service_tiers")
            .and_then(Value::as_array)
            .is_some_and(|tiers| {
                tiers.iter().any(|tier| {
                    tier.get("id")
                        .and_then(Value::as_str)
                        .or_else(|| tier.as_str())
                        .is_some_and(|value| is_requested_service_tier(value, requested))
                })
            })
            || object
                .get("additional_speed_tiers")
                .and_then(Value::as_array)
                .is_some_and(|tiers| {
                    tiers
                        .iter()
                        .filter_map(Value::as_str)
                        .any(|value| is_requested_service_tier(value, requested))
                })
    })
}

fn is_requested_service_tier(value: &str, requested: DefaultServiceTier) -> bool {
    match requested {
        DefaultServiceTier::Standard => false,
        DefaultServiceTier::Fast => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "fast" | "priority"
        ),
        DefaultServiceTier::Ultrafast => value.trim().eq_ignore_ascii_case("ultrafast"),
    }
}

const fn service_tier_rank(tier: DefaultServiceTier) -> u8 {
    match tier {
        DefaultServiceTier::Standard => 0,
        DefaultServiceTier::Fast => 1,
        DefaultServiceTier::Ultrafast => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_requires_the_exact_requested_speed_tier() {
        let manifest = serde_json::json!({
            "data": [{
                "id": "gpt-5.6-sol",
                "service_tiers": [
                    {"id": "priority"},
                    {"id": "ultrafast"}
                ],
                "additional_speed_tiers": ["fast"]
            }]
        });

        assert!(manifest_supports_service_tier(
            &manifest,
            "gpt-5.6-sol",
            DefaultServiceTier::Fast
        ));
        assert!(manifest_supports_service_tier(
            &manifest,
            "gpt-5.6-sol",
            DefaultServiceTier::Ultrafast
        ));
        assert!(!manifest_supports_service_tier(
            &manifest,
            "gpt-5.6-sol",
            DefaultServiceTier::Standard
        ));
        assert!(!manifest_supports_service_tier(
            &manifest,
            "gpt-5.6-terra",
            DefaultServiceTier::Ultrafast
        ));
    }

    #[test]
    fn additional_speed_tiers_can_advertise_ultrafast() {
        let manifest = serde_json::json!({
            "models": [{
                "slug": "gpt-5.6-sol",
                "additional_speed_tiers": ["ultrafast"]
            }]
        });
        assert!(manifest_supports_service_tier(
            &manifest,
            "gpt-5.6-sol",
            DefaultServiceTier::Ultrafast
        ));
    }
}
