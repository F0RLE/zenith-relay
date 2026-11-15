use super::*;

#[test]
fn siwc_reauthorization_keeps_an_empty_catalog_through_both_settings_merges() {
    for cached_models in [Vec::new(), vec!["gpt-cached".to_string()]] {
        let mut current = account("account_siwc", "synthetic-account", "old-refresh");
        current.discovered_models = Some(vec!["gpt-old".into()]);
        current.account.in_pool = true;
        current.account.enabled = false;
        current.allowed_models = vec!["gpt-old".into()];
        let mut updated = account("account_siwc", "synthetic-account", "new-refresh");
        updated.models = cached_models;
        updated.discovered_models = Some(Vec::new());

        for _ in 0..2 {
            preserve_existing_settings(&mut updated, &current, OAuthClientKind::Siwc);

            assert_eq!(updated.models, current.models);
            assert_eq!(updated.discovered_models, Some(Vec::new()));
            assert!(updated.effective_models().is_empty());
            assert_eq!(updated.pool_models(false).count(), 0);
            assert_eq!(updated.allowed_models, current.allowed_models);
            assert_eq!(updated.account.in_pool, current.account.in_pool);
            assert_eq!(updated.account.enabled, current.account.enabled);
            assert_eq!(updated.account.auth_state, AccountAuthState::Active);
        }
    }
}

#[test]
fn siwc_reauthorization_preserves_a_reduced_fresh_catalog_over_the_saved_baseline() {
    let mut current = account("account_siwc", "synthetic-account", "old-refresh");
    current.discovered_models = Some(vec!["gpt-first".into(), "gpt-second".into()]);
    let mut updated = account("account_siwc", "synthetic-account", "new-refresh");
    updated.discovered_models = Some(vec!["gpt-second".into()]);

    for _ in 0..2 {
        preserve_existing_settings(&mut updated, &current, OAuthClientKind::Siwc);
        assert_eq!(updated.models, current.models);
        assert_eq!(updated.effective_models(), ["gpt-second"]);
    }
}
