use super::*;
use crate::{CapabilityOrigin, CapabilityStatus, WireApi};

struct PartialInventory {
    models: Vec<String>,
    bindings: Vec<SourceProtocolBinding>,
    retained_only: BTreeSet<String>,
    unread: BTreeMap<WireApi, BTreeSet<String>>,
}

impl SourceDiscovery {
    /// Inventory projection only; retained names do not establish new routes.
    pub fn models_with_previous(&self, previous: &[String]) -> Vec<String> {
        if self.failures.is_empty() {
            self.models.clone()
        } else {
            self.partial_inventory(previous).models
        }
    }

    pub fn failure_summary(&self) -> Option<String> {
        if self.failures.is_empty() {
            return None;
        }
        let details = self
            .failures
            .iter()
            .map(|failure| {
                format!(
                    "{:?}: {}",
                    failure
                        .binding
                        .adapter
                        .upstream_protocol(failure.binding.wire_api),
                    failure.message
                )
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>()
            .join("; ");
        Some(format!(
            "source model catalog refresh is incomplete: {details}"
        ))
    }

    /// Applies complete observations while preserving unread catalog scopes.
    /// Empty failed bindings remain discovery plans, not model permissions.
    pub fn apply_catalog(
        &self,
        base_url: &mut String,
        models: &mut Vec<String>,
        protocol_bindings: &mut Vec<SourceProtocolBinding>,
        protocol_config: &mut SourceProtocolConfig,
        detected_model_prices: &mut BTreeMap<String, ApiModelPriceOverride>,
    ) {
        if let Some(resolved) = &self.resolved_base_url {
            *base_url = resolved.clone();
        }
        if self.failures.is_empty() {
            *models = self.models.clone();
            *protocol_bindings = self.protocol_bindings.clone();
            protocol_config.merge_catalog(self.capabilities.clone());
            *detected_model_prices = self.detected_model_prices.clone();
            return;
        }
        let PartialInventory {
            models: updated_models,
            bindings,
            retained_only,
            unread,
        } = self.partial_inventory(models);
        let mut prices = self.detected_model_prices.clone();
        for (model, &price) in detected_model_prices.iter() {
            let key = crate::model_id_key(model);
            if retained_only.contains(&key) {
                prices.entry(key).or_insert(price);
            }
        }
        let mut fresh_by_route = BTreeMap::<_, Vec<_>>::new();
        for capability in &self.capabilities {
            fresh_by_route
                .entry((
                    crate::model_id_key(&capability.model_id),
                    capability.upstream_wire_api,
                ))
                .or_default()
                .push(capability);
        }
        let mut capabilities = self.capabilities.clone();
        for previous in &protocol_config.capabilities {
            if previous.origin != CapabilityOrigin::Catalog {
                continue;
            }
            let key = crate::model_id_key(&previous.model_id);
            if retained_only.contains(&key) {
                capabilities.push(previous.clone());
            } else if unread
                .get(&previous.upstream_wire_api)
                .is_some_and(|models| models.contains(&key))
            {
                let fresh = fresh_by_route
                    .get(&(key, previous.upstream_wire_api))
                    .map(Vec::as_slice)
                    .unwrap_or_default();
                let mut retained = previous.clone();
                if fresh
                    .iter()
                    .any(|row| row.status != CapabilityStatus::Unknown)
                {
                    retained.status = CapabilityStatus::Unknown;
                }
                retained.features.retain(|feature, _| {
                    !fresh.iter().any(|row| {
                        row.features
                            .get(feature)
                            .is_some_and(|status| *status != CapabilityStatus::Unknown)
                    })
                });
                if fresh.iter().any(|row| !row.reasoning_efforts.is_empty()) {
                    retained.reasoning_efforts.clear();
                }
                if fresh.iter().any(|row| row.service_tiers.is_some()) {
                    retained.service_tiers = None;
                }
                if fresh
                    .iter()
                    .any(|row| !row.token_limits.validated().is_empty())
                {
                    retained.token_limits = Default::default();
                }
                capabilities.push(retained);
            }
        }
        *models = updated_models;
        *protocol_bindings = bindings;
        protocol_config.merge_catalog(capabilities);
        *detected_model_prices = prices;
    }

    fn partial_inventory(&self, previous_models: &[String]) -> PartialInventory {
        let previous = previous_models
            .iter()
            .map(|model| crate::model_id_key(model))
            .collect::<BTreeSet<_>>();
        let fresh = self
            .models
            .iter()
            .map(|model| crate::model_id_key(model))
            .collect::<BTreeSet<_>>();
        let observed = self
            .catalog_models
            .iter()
            .map(|(&protocol, models)| {
                (
                    protocol,
                    models
                        .iter()
                        .map(|model| crate::model_id_key(model))
                        .collect::<BTreeSet<_>>(),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let mut retained = BTreeSet::new();
        let mut unread = BTreeMap::<WireApi, BTreeSet<String>>::new();
        let mut bindings = self.protocol_bindings.clone();
        for failure in &self.failures {
            let protocol = failure
                .binding
                .adapter
                .upstream_protocol(failure.binding.wire_api)
                .wire_api();
            let mut binding = failure.binding.clone();
            if let Some(known) = observed.get(&protocol) {
                binding
                    .model_ids
                    .retain(|model| known.contains(&crate::model_id_key(model)));
            } else {
                binding
                    .model_ids
                    .retain(|model| previous.contains(&crate::model_id_key(model)));
                for model in &binding.model_ids {
                    let key = crate::model_id_key(model);
                    retained.insert(key.clone());
                    unread.entry(protocol).or_default().insert(key);
                }
            }
            if !bindings.iter().any(|known| known.key() == binding.key()) {
                bindings.push(binding);
            }
        }
        let retained_only = retained
            .difference(&fresh)
            .cloned()
            .collect::<BTreeSet<_>>();
        let mut models = self.models.clone();
        let mut seen = fresh;
        models.extend(
            previous_models
                .iter()
                .filter(|model| {
                    let key = crate::model_id_key(model);
                    retained_only.contains(&key) && seen.insert(key)
                })
                .cloned(),
        );
        PartialInventory {
            models,
            bindings,
            retained_only,
            unread,
        }
    }
}
