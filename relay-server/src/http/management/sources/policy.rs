use crate::state::SourceRecord;
use zenith_relay_core::{changed_runtime_source_policy_updates, RuntimeSourcePolicyUpdate};

pub(super) fn updates(
    previous: &[SourceRecord],
    next: &[SourceRecord],
) -> Vec<RuntimeSourcePolicyUpdate> {
    changed_runtime_source_policy_updates(previous, next)
}

pub(super) fn source_runtime_policy_compatible(
    previous: &[SourceRecord],
    next: &[SourceRecord],
) -> bool {
    previous.len() == next.len()
        && previous.iter().all(|source| {
            next.iter()
                .find(|candidate| candidate.id == source.id)
                .is_some_and(|candidate| {
                    source.base_url == candidate.base_url
                        && source.secret_ref == candidate.secret_ref
                        && source.wire_api == candidate.wire_api
                        && source.protocol_bindings == candidate.protocol_bindings
                        && source.models == candidate.models
                })
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use zenith_relay_core::WireApi;

    fn source() -> SourceRecord {
        SourceRecord {
            id: "source-test".into(),
            name: "Test source".into(),
            enabled: true,
            in_pool: true,
            draining: false,
            base_url: "https://example.test/v1".into(),
            secret_ref: "source:test".into(),
            wire_api: WireApi::Responses,
            protocol_bindings: Vec::new(),
            models: vec!["gpt-test".into()],
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
            priority: 0,
            weight: 1,
            recovery_delay_seconds: 0,
            model_price_overrides: BTreeMap::new(),
            detected_model_prices: BTreeMap::new(),
            last_error_code: None,
        }
    }

    #[test]
    fn source_policy_changes_stay_hot_but_transport_changes_rebuild() {
        let previous = source();
        let mut policy = previous.clone();
        policy.enabled = false;
        policy.in_pool = false;
        policy.draining = true;
        policy.priority = 2;
        policy.weight = 3;
        policy.allowed_models = vec!["gpt-allowed".into()];
        policy.excluded_models = vec!["gpt-blocked".into()];
        policy.recovery_delay_seconds = 15;

        assert!(source_runtime_policy_compatible(
            std::slice::from_ref(&previous),
            std::slice::from_ref(&policy)
        ));
        assert_eq!(
            updates(
                std::slice::from_ref(&previous),
                std::slice::from_ref(&policy)
            )
            .len(),
            1
        );

        let mut membership_only = previous.clone();
        membership_only.in_pool = false;
        assert!(source_runtime_policy_compatible(
            std::slice::from_ref(&previous),
            std::slice::from_ref(&membership_only)
        ));
        assert!(
            updates(
                std::slice::from_ref(&previous),
                std::slice::from_ref(&membership_only)
            )
            .is_empty(),
            "pool membership refreshes key scope separately"
        );

        let mut model_change = previous.clone();
        model_change.models.push("gpt-new".into());
        assert!(!source_runtime_policy_compatible(
            std::slice::from_ref(&previous),
            std::slice::from_ref(&model_change)
        ));

        let mut transport_change = previous.clone();
        transport_change.base_url = "https://other.example.test/v1".into();
        assert!(!source_runtime_policy_compatible(
            std::slice::from_ref(&previous),
            std::slice::from_ref(&transport_change)
        ));
    }
}
