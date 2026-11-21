use super::*;
use crate::local_pool::accounts::oauth::siwc::SiwcRegistration;

#[test]
fn siwc_identity_only_completion_roundtrips_without_an_access_token() {
    let registration = SiwcRegistration {
        client_id: "oaiapp_synthetic".into(),
        host_id: "urn:uuid:synthetic-host".into(),
        issuer: "https://auth.openai.com".into(),
        subject: "synthetic-subject".into(),
        scopes: vec!["openid".into(), "email".into()],
        earliest_refresh_at_ms: None,
        requires_reauthentication: false,
    };
    let checkpoint = OAuthCompletionCheckpoint {
        version: COMPLETION_CHECKPOINT_VERSION,
        login_id: uuid::Uuid::new_v4().to_string(),
        client_kind: OAuthClientKind::Siwc,
        access_token: String::new(),
        refresh_token: None,
        id_token: Some("synthetic-id-token".into()),
        expires_at_ms: None,
        issued_at_ms: 1_000,
        email: Some("test@example.invalid".into()),
        provider_account_id: registration.identity_key(),
        provider_user_id: Some(registration.subject.clone()),
        plan_type: None,
        subscription_active_until_ms: None,
        account_is_fedramp: false,
        siwc: Some(registration),
    };
    let encoded = encode_completion_checkpoint(&checkpoint).unwrap();
    let restored = decode_completion_checkpoint(&encoded, &checkpoint.login_id)
        .unwrap()
        .unwrap();
    let credentials = restored.to_credentials("siwc_identity", 3).unwrap();
    assert!(credentials.is_siwc_identity_only());
    assert!(credentials.authorization(1_001).is_err());
    assert_eq!(credentials.generation(), 3);
    let mut corrupted = restored;
    corrupted.refresh_token = Some("unexpected-refresh".into());
    assert!(corrupted.to_credentials("siwc_identity", 3).is_err());
}
