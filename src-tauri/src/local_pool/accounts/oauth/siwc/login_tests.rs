use super::tests::registration;
use crate::local_pool::accounts::oauth::{CodexOAuthClient, OAuthClientKind};

#[test]
fn siwc_returning_hint_is_bound_to_the_attempt_without_id_token_in_url() {
    let registration = registration();
    let client = CodexOAuthClient::new_with_proxy_for_kind(OAuthClientKind::Siwc, None)
        .unwrap()
        .with_siwc_registration(registration.host_id.clone(), Some(&registration))
        .unwrap()
        .with_siwc_login_hint(Some("returning@example.invalid"))
        .unwrap();
    let start = client.begin(54321, 1000).unwrap();
    start
        .pending()
        .validate_authorization_url(start.authorization_url().as_str())
        .unwrap();
    let query: std::collections::BTreeMap<_, _> = start
        .authorization_url()
        .query_pairs()
        .into_owned()
        .collect();
    assert_eq!(query["login_hint"], "returning@example.invalid");
    assert!(!query.contains_key("id_token_hint"));
    assert!(!format!("{:?}", start.pending()).contains("returning@example.invalid"));
    let tampered = start
        .authorization_url()
        .as_str()
        .replace("returning", "another");
    assert!(
        start
            .pending()
            .validate_authorization_url(&tampered)
            .is_err()
    );
    let mut removed = start.authorization_url().clone();
    removed.set_query(None);
    for (key, value) in query.iter().filter(|(key, _)| key.as_str() != "login_hint") {
        removed.query_pairs_mut().append_pair(key, value);
    }
    assert!(
        start
            .pending()
            .validate_authorization_url(removed.as_str())
            .is_err()
    );
}

#[test]
fn siwc_bootstrap_and_unsafe_hints_are_omitted() {
    let registration = registration();
    for (saved, hint) in [
        (None, "returning@example.invalid"),
        (Some(&registration), "invalid\n@example.invalid"),
        (Some(&registration), ""),
    ] {
        let client = CodexOAuthClient::new_with_proxy_for_kind(OAuthClientKind::Siwc, None)
            .unwrap()
            .with_siwc_registration(registration.host_id.clone(), saved)
            .unwrap()
            .with_siwc_login_hint(Some(hint))
            .unwrap();
        let start = client.begin(54321, 1000).unwrap();
        assert!(
            !start
                .authorization_url()
                .query_pairs()
                .any(|(key, _)| key == "login_hint")
        );
        start
            .pending()
            .validate_authorization_url(start.authorization_url().as_str())
            .unwrap();
    }
}
