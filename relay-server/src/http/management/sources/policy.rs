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

pub(super) fn source_dispatch_permission_changed(
    previous: &SourceRecord,
    next: &SourceRecord,
    credential_replaced: bool,
) -> bool {
    credential_replaced
        || !source_runtime_policy_compatible(
            std::slice::from_ref(previous),
            std::slice::from_ref(next),
        )
        || previous.enabled != next.enabled
        || previous.in_pool != next.in_pool
        || previous.draining != next.draining
        || previous.allowed_models != next.allowed_models
        || previous.excluded_models != next.excluded_models
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixtures::pooled_source;

    #[test]
    fn source_policy_changes_stay_hot_but_transport_changes_rebuild() {
        let previous = pooled_source("source-test", "gpt-test");
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

    #[test]
    fn dispatch_fence_only_follows_permission_or_transport_edits() {
        let previous = pooled_source("source-test", "gpt-test");
        let mut weighting = previous.clone();
        weighting.priority = 2;
        weighting.weight = 3;
        weighting.recovery_delay_seconds = 15;
        assert!(!source_dispatch_permission_changed(
            &previous, &weighting, false
        ));
        assert!(source_dispatch_permission_changed(
            &previous, &weighting, true
        ));

        let mut membership = previous.clone();
        membership.in_pool = false;
        assert!(source_dispatch_permission_changed(
            &previous,
            &membership,
            false
        ));
        let mut transport = previous.clone();
        transport.base_url = "https://other.example.test/v1".into();
        assert!(source_dispatch_permission_changed(
            &previous, &transport, false
        ));
    }
}
