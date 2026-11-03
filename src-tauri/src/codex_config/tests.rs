use super::auth::zenith_auth_key_if_configured;
use super::*;
use std::{sync::mpsc, thread, time::Duration};

#[test]
fn local_pool_key_is_not_a_zenith_customer_key() {
    assert!(!is_zenith_customer_key("zlr_local_generated_key"));
    assert!(!is_zenith_customer_key("zrk_retired_reseller_key"));
    assert!(is_zenith_customer_key("znt_customer_key"));
    assert!(
        zenith_auth_key_if_configured(
            "model_provider = \"zenith_relay_local\"",
            Some("zlr_local_generated_key".into())
        )
        .is_none()
    );
}

#[test]
fn legacy_auth_requires_an_explicitly_selected_provider_and_customer_key() {
    for provider in ["codex_local_access", "zenith"] {
        let config = format!("model_provider = \"{provider}\"");
        assert_eq!(
            zenith_auth_key_if_configured(&config, Some("znt_customer_key".into())).as_deref(),
            Some("znt_customer_key")
        );
        assert!(zenith_auth_key_if_configured(&config, Some("sk_unrelated_key".into())).is_none());
        assert!(zenith_auth_key_if_configured(&config, None).is_none());
    }
    assert!(!config_uses_zenith_provider(
        "model_provider = \"openai\"\n\n[model_providers.codex_local_access]"
    ));
    assert!(!config_uses_zenith_provider("model_provider = ["));
}

#[test]
fn profile_lock_serializes_mutations() {
    let guard = lock_codex_profile();
    let (sender, receiver) = mpsc::channel();
    let worker = thread::spawn(move || {
        let _guard = lock_codex_profile();
        sender.send(()).unwrap();
    });
    assert!(receiver.recv_timeout(Duration::from_millis(50)).is_err());
    drop(guard);
    receiver.recv_timeout(Duration::from_secs(5)).unwrap();
    worker.join().unwrap();
}
