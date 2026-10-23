use super::*;
use crate::local_pool::accounts::oauth::{CodexOAuthClient, OAuthClientKind};
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use serde_json::{json, Value};

// Synthetic RSA key generated only for these tests; never used by an account.
const KEY: &[u8] = include_bytes!("test-key.pem");
fn jwks() -> jsonwebtoken::jwk::JwkSet {
    serde_json::from_value(json!({"keys":[{"kid":"synthetic","kty":"RSA","alg":"RS256","use":"sig","n":"51zSyAmkFu7j50VJRPuFbSD6-gEQyLSQx14qseDE-U3NJcKuYA-Q5ejJrB7qfvRNz8ZuXvE5nFihe75wo0ZKp_BSQ6u2NT_djPLpS7N7qw4NCHvaAks_DBdTTkmSqdYRmeGp1rKrIPg4hbe58JeB23-q94M6PmK7Pj3ihUZCg-MjI-hvvv5J3t46r6-bHv78GvMa8KgMu_eIzO25eedcMlpMmy32x1bDR3JzuyhtcvyxixFJxHWBJsX6Iku1Ol7jn9iEvz3XHxqXf1f5redI5W6wnVE3k7NrW1mQI2fG3omVX0er4VIU8-HMhz4Jqp_uK4GcpT0_MMalwzI3Trf2iQ","e":"AQAB"}]})).unwrap()
}
fn sign(claims: &Value) -> String {
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some("synthetic".into());
    encode(&header, claims, &EncodingKey::from_rsa_pem(KEY).unwrap()).unwrap()
}
fn now_ms() -> u64 {
    jsonwebtoken::get_current_timestamp() * 1000
}
fn registration() -> SiwcRegistration {
    SiwcRegistration {
        client_id: "oaiapp_synthetic".into(),
        host_id: "urn:uuid:synthetic-host".into(),
        issuer: ISSUER.into(),
        subject: "synthetic-subject".into(),
        scopes: vec![
            "openid".into(),
            "resource.invoke".into(),
            "chatgpt.tokens.use.direct".into(),
        ],
        earliest_refresh_at_ms: None,
        requires_reauthentication: false,
    }
}

#[test]
fn verifies_signature_issuer_audience_expiry_subject_and_nonce() {
    let now = now_ms();
    let claims = json!({"iss":ISSUER,"aud":"oaiapp_synthetic","sub":"synthetic-subject","exp":now / 1000 + 300,"nonce":"synthetic-nonce","email":"test@example.invalid"});
    assert!(verify_id_token(
        &sign(&claims),
        &jwks(),
        "oaiapp_synthetic",
        Some("synthetic-nonce"),
        now
    )
    .is_ok());
    for (field, value) in [
        ("iss", json!("https://invalid.example")),
        ("aud", json!("other-client")),
        ("sub", json!("")),
        ("exp", json!(now / 1000 - 1)),
        ("nonce", json!("wrong")),
        ("nbf", json!(now / 1000 + 100)),
    ] {
        let mut invalid = claims.clone();
        invalid[field] = value;
        assert!(
            verify_id_token(
                &sign(&invalid),
                &jwks(),
                "oaiapp_synthetic",
                Some("synthetic-nonce"),
                now
            )
            .is_err(),
            "{field}"
        );
    }
    let signed = sign(&claims);
    let mut parts: Vec<_> = signed.split('.').map(str::to_string).collect();
    let replacement = if parts[2].starts_with('A') { "B" } else { "A" };
    parts[2].replace_range(..1, replacement);
    assert!(verify_id_token(
        &parts.join("."),
        &jwks(),
        "oaiapp_synthetic",
        Some("synthetic-nonce"),
        now
    )
    .is_err());
    let mut multiple = claims.clone();
    multiple["aud"] = json!(["oaiapp_synthetic", "other-client"]);
    for azp in [Value::Null, json!("other-client")] {
        multiple["azp"] = azp;
        assert!(verify_id_token(
            &sign(&multiple),
            &jwks(),
            "oaiapp_synthetic",
            Some("synthetic-nonce"),
            now
        )
        .is_err());
    }
    multiple["azp"] = json!("oaiapp_synthetic");
    assert!(verify_id_token(
        &sign(&multiple),
        &jwks(),
        "oaiapp_synthetic",
        Some("synthetic-nonce"),
        now
    )
    .is_ok());
}

#[test]
fn imported_registration_can_reauthorize_on_a_new_host_without_enabling_old_tokens() {
    let mut registration = registration();
    assert!(super::super::CodexOAuthClient::new()
        .unwrap()
        .with_siwc_registration("urn:uuid:new-host".into(), Some(&registration))
        .is_err());
    registration.requires_reauthentication = true;
    assert!(!registration.plan_use_enabled());
    let client = super::super::CodexOAuthClient::new()
        .unwrap()
        .with_siwc_registration("urn:uuid:new-host".into(), Some(&registration))
        .unwrap();
    let pending = client.siwc.unwrap();
    assert_eq!(pending.host_id, "urn:uuid:new-host");
    assert_eq!(pending.client_id, registration.client_id);
    assert_eq!(
        pending.subject.as_deref(),
        Some(registration.subject.as_str())
    );
}

#[test]
fn verified_optional_plan_metadata_preserves_the_siwc_subject() {
    let now = now_ms();
    let mut claims = json!({
        "iss": ISSUER, "aud": "oaiapp_synthetic", "sub": "synthetic-subject",
        "exp": now / 1000 + 300, "nonce": "synthetic-nonce", "plan_type": "pro"
    });
    let absent = verify_id_token(
        &sign(&claims),
        &jwks(),
        "oaiapp_synthetic",
        Some("synthetic-nonce"),
        now,
    )
    .unwrap();
    assert_eq!(absent.plan_type, None);
    assert_eq!(absent.subscription_active_until_ms, None);
    claims["https://api.openai.com/auth"] = json!({
        "chatgpt_plan_type": "plus",
        "chatgpt_subscription_active_until": now / 1000 + 86_400,
        "chatgpt_account_id": "synthetic-native-workspace",
        "chatgpt_user_id": "synthetic-native-user"
    });
    let verified = verify_id_token(
        &sign(&claims),
        &jwks(),
        "oaiapp_synthetic",
        Some("synthetic-nonce"),
        now,
    )
    .unwrap();
    assert_eq!(verified.plan_type.as_deref(), Some("plus"));
    assert_eq!(
        verified.subscription_active_until_ms,
        Some(now + 86_400_000)
    );
    assert_eq!(verified.sub, "synthetic-subject");
    claims["https://api.openai.com/auth"]["chatgpt_plan_type"] = json!(false);
    let malformed = verify_id_token(
        &sign(&claims),
        &jwks(),
        "oaiapp_synthetic",
        Some("synthetic-nonce"),
        now,
    )
    .unwrap();
    assert_eq!(malformed.plan_type, None);
    claims["https://api.openai.com/auth"] = json!({
        "per_user_salt": "synthetic-salt", "encrypted_auth_metadata": "opaque-synthetic-metadata"
    });
    let opaque = verify_id_token(
        &sign(&claims),
        &jwks(),
        "oaiapp_synthetic",
        Some("synthetic-nonce"),
        now,
    )
    .unwrap();
    assert_eq!(opaque.plan_type, None);
    assert_eq!(opaque.subscription_active_until_ms, None);
}

#[test]
fn initial_registration_binds_loopback_pkce_nonce_and_issued_client() {
    let client = CodexOAuthClient::new_with_proxy_for_kind(OAuthClientKind::Siwc, None)
        .unwrap()
        .with_siwc_registration("urn:uuid:synthetic-host".into(), None)
        .unwrap();
    let start = client.begin(54321, 1000).unwrap();
    let pending = start.pending();
    pending
        .validate_authorization_url(start.authorization_url().as_str())
        .unwrap();
    assert_eq!(
        pending.redirect_uri(),
        "http://127.0.0.1:54321/auth/callback"
    );
    let query: std::collections::BTreeMap<_, _> = start
        .authorization_url()
        .query_pairs()
        .into_owned()
        .collect();
    assert_eq!(query["client_id"], "dynamic_agent_client");
    assert_eq!(query["agent_name_hint"], "Zenith Relay");
    assert_ne!(query["nonce"], query["state"]);
    let base = format!(
        "{}?code=synthetic-code&state={}",
        pending.redirect_uri(),
        query["state"]
    );
    for suffix in [
        "",
        "&client_id=dynamic_agent_client",
        "&client_id=oaiapp_one&client_id=oaiapp_two",
        "&client_id=oaiapp_one&state=wrong",
    ] {
        assert!(pending
            .parse_callback(&format!("{base}{suffix}"), 1001)
            .is_err());
    }
    let callback = pending
        .parse_callback(&format!("{base}&client_id=oaiapp_synthetic"), 1001)
        .unwrap();
    assert_eq!(callback.client_id.as_deref(), Some("oaiapp_synthetic"));
    assert!(pending
        .parse_callback(
            &format!("{base}&client_id=oaiapp_synthetic").replace("127.0.0.1", "localhost"),
            1001
        )
        .is_err());
}

#[test]
fn returning_registration_cannot_be_rebound_and_consent_is_explicit() {
    let mut reg = registration();
    reg.scopes.clear();
    let client = CodexOAuthClient::new_with_proxy_for_kind(OAuthClientKind::Siwc, None)
        .unwrap()
        .with_siwc_registration(reg.host_id.clone(), Some(&reg))
        .unwrap();
    let start = client.begin(54322, 1000).unwrap();
    start
        .pending()
        .validate_authorization_url(start.authorization_url().as_str())
        .unwrap();
    let query: std::collections::BTreeMap<_, _> = start
        .authorization_url()
        .query_pairs()
        .into_owned()
        .collect();
    assert_eq!(query["client_id"], reg.client_id);
    assert_eq!(query["prompt"], "consent");
    assert!(!query.contains_key("agent_name_hint"));
    let callback = format!(
        "{}?code=synthetic&state={}",
        start.pending().redirect_uri(),
        query["state"]
    );
    assert!(start.pending().parse_callback(&callback, 1001).is_ok());
    assert!(start
        .pending()
        .parse_callback(&format!("{callback}&client_id=oaiapp_foreign"), 1001)
        .is_err());
    let denied = format!(
        "{}?error=access_denied&state={}",
        start.pending().redirect_uri(),
        query["state"]
    );
    assert_eq!(
        start
            .pending()
            .parse_callback(&denied, 1001)
            .unwrap_err()
            .code,
        OAuthErrorCode::AuthorizationDenied
    );
}

#[test]
fn registrations_separate_workspaces_and_do_not_expose_identity() {
    let reg = registration();
    let mut other = reg.clone();
    other.client_id = "oaiapp_other_workspace".into();
    assert_ne!(reg.identity_key(), other.identity_key());
    assert!(!format!("{reg:?}").contains(&reg.subject));
    assert!(!format!("{reg:?}").contains(&reg.client_id));
    other.client_id = "dynamic_agent_client".into();
    assert!(other.validate().is_err());
    let decoded: SiwcRegistration =
        serde_json::from_str(&serde_json::to_string(&reg).unwrap()).unwrap();
    assert_eq!(decoded, reg);
}
