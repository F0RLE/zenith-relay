use crate::state::SourceRecord;
use zenith_relay_core::{
    changed_runtime_source_policy_updates, pool_dispatch_permission_changed,
    source_runtime_policy_compatible as transport_compatible, PoolParticipant,
    RuntimeSourcePolicyUpdate,
};

pub(super) fn updates(
    previous_sources: &[SourceRecord],
    updated_sources: &[SourceRecord],
) -> Vec<RuntimeSourcePolicyUpdate> {
    changed_runtime_source_policy_updates(previous_sources, updated_sources)
}

pub(super) fn source_runtime_policy_compatible(
    previous_sources: &[SourceRecord],
    updated_sources: &[SourceRecord],
) -> bool {
    transport_compatible(previous_sources, updated_sources)
}

pub(super) fn source_dispatch_permission_changed(
    previous_source: &SourceRecord,
    updated_source: &SourceRecord,
    credential_replaced: bool,
) -> bool {
    credential_replaced
        || !source_runtime_policy_compatible(
            std::slice::from_ref(previous_source),
            std::slice::from_ref(updated_source),
        )
        || pool_dispatch_permission_changed(
            previous_source.pool_access(),
            updated_source.pool_access(),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixtures::pooled_source;

    #[test]
    fn source_policy_changes_stay_hot_but_transport_changes_rebuild() {
        let previous_source = pooled_source("source-test", "gpt-test");
        let mut policy = previous_source.clone();
        policy.enabled = false;
        policy.in_pool = false;
        policy.draining = true;
        policy.priority = 2;
        policy.weight = 3;
        policy.allowed_models = vec!["gpt-allowed".into()];
        policy.excluded_models = vec!["gpt-blocked".into()];
        policy.recovery_delay_seconds = 15;

        assert!(source_runtime_policy_compatible(
            std::slice::from_ref(&previous_source),
            std::slice::from_ref(&policy)
        ));
        assert_eq!(
            updates(
                std::slice::from_ref(&previous_source),
                std::slice::from_ref(&policy)
            )
            .len(),
            1
        );

        let mut membership_only = previous_source.clone();
        membership_only.in_pool = false;
        assert!(source_runtime_policy_compatible(
            std::slice::from_ref(&previous_source),
            std::slice::from_ref(&membership_only)
        ));
        assert!(
            updates(
                std::slice::from_ref(&previous_source),
                std::slice::from_ref(&membership_only)
            )
            .is_empty(),
            "pool membership refreshes key scope separately"
        );

        let mut model_change = previous_source.clone();
        model_change.models.push("gpt-new".into());
        assert!(!source_runtime_policy_compatible(
            std::slice::from_ref(&previous_source),
            std::slice::from_ref(&model_change)
        ));

        let mut transport_change = previous_source.clone();
        transport_change.base_url = "https://other.example.test/v1".into();
        assert!(!source_runtime_policy_compatible(
            std::slice::from_ref(&previous_source),
            std::slice::from_ref(&transport_change)
        ));

        let mut catalog_change = previous_source.clone();
        catalog_change.protocol_config.revision =
            catalog_change.protocol_config.revision.saturating_add(1);
        assert!(!source_runtime_policy_compatible(
            std::slice::from_ref(&previous_source),
            std::slice::from_ref(&catalog_change)
        ));
    }

    #[test]
    fn dispatch_fence_only_follows_permission_or_transport_edits() {
        let previous_source = pooled_source("source-test", "gpt-test");
        let mut weighting = previous_source.clone();
        weighting.priority = 2;
        weighting.weight = 3;
        weighting.recovery_delay_seconds = 15;
        assert!(!source_dispatch_permission_changed(
            &previous_source,
            &weighting,
            false
        ));
        assert!(source_dispatch_permission_changed(
            &previous_source,
            &weighting,
            true
        ));

        let mut membership = previous_source.clone();
        membership.in_pool = false;
        assert!(source_dispatch_permission_changed(
            &previous_source,
            &membership,
            false
        ));
        let mut transport = previous_source.clone();
        transport.base_url = "https://other.example.test/v1".into();
        assert!(source_dispatch_permission_changed(
            &previous_source,
            &transport,
            false
        ));
    }
}
