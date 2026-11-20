use super::*;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use jsonwebtoken::{EncodingKey, Header, encode};
use openssl::rsa::Rsa;
use serde_json::{Value, json};

fn fixture() -> (EncodingKey, JwkSet, Value, Header) {
    let rsa = Rsa::generate(2048).unwrap();
    let key = EncodingKey::from_rsa_der(&rsa.private_key_to_der().unwrap());
    let keys = serde_json::from_value(json!({"keys":[{
        "kty":"RSA", "kid":"test", "alg":"RS256", "use":"sig", "key_ops":["verify"],
        "n":URL_SAFE_NO_PAD.encode(rsa.n().to_vec()), "e":URL_SAFE_NO_PAD.encode(rsa.e().to_vec()),
    }]}))
    .unwrap();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let claims = json!({"iss":"https://issuer.example", "aud":"fixture-client", "sub":"synthetic",
        "iat":now, "exp":now+300, "nonce":oauth2::CsrfToken::new_random().secret(), "azp":"fixture-client"});
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some("test".into());
    (key, keys, claims, header)
}

fn check(token: &str, keys: &JwkSet, nonce: &str) -> Result<String, Failure> {
    subject(
        token,
        keys,
        "https://issuer.example",
        "fixture-client",
        nonce,
    )
}

#[test]
fn verified_identity_requires_exact_claims_and_live_validity() {
    let (key, keys, claims, header) = fixture();
    let nonce = claims["nonce"].as_str().unwrap().to_owned();
    let check = |token: &str, keys: &JwkSet| check(token, keys, &nonce);
    let sign = |claims: &Value| encode(&header, claims, &key).unwrap();
    assert_eq!(
        check(&sign(&claims), &keys).ok().as_deref(),
        Some("synthetic")
    );
    let now = claims["iat"].as_u64().unwrap();
    for (name, value) in [
        ("iss", json!("https://other.example")),
        ("aud", json!("other")),
        ("aud", json!(["fixture-client", "other"])),
        ("aud", json!([])),
        ("azp", json!("other")),
        ("nonce", json!("other")),
        ("exp", json!(now - 1)),
        ("nbf", json!(now + 60)),
        ("iat", json!(now - 220)),
        ("iat", json!(now + 60)),
        ("sub", json!("")),
        ("sub", json!("x".repeat(256))),
        ("sub", json!("x\n")),
    ] {
        let mut changed = claims.clone();
        changed[name] = value;
        assert!(check(&sign(&changed), &keys).is_err(), "claim {name}");
    }
    for name in ["iss", "aud", "sub", "exp", "iat", "nonce"] {
        let mut missing = claims.clone();
        missing.as_object_mut().unwrap().remove(name);
        assert!(check(&sign(&missing), &keys).is_err(), "missing {name}");
    }
    let mut single = claims;
    single["aud"] = json!(["fixture-client"]);
    single.as_object_mut().unwrap().remove("azp");
    assert!(check(&sign(&single), &keys).is_ok());
}

#[test]
fn verifier_rejects_wrong_keys_algorithms_and_ambiguous_jwks() {
    let (key, keys, claims, mut header) = fixture();
    let nonce = claims["nonce"].as_str().unwrap();
    let check = |token: &str, keys: &JwkSet| check(token, keys, nonce);
    let token = encode(&header, &claims, &key).unwrap();
    let (_, foreign, _, _) = fixture();
    assert!(check(&token, &foreign).is_err());
    assert!(check(&token, &JwkSet { keys: vec![] }).is_err());
    let mut duplicate = keys.clone();
    duplicate.keys.push(duplicate.keys[0].clone());
    assert!(check(&token, &duplicate).is_err());
    for (name, value) in [
        ("use", json!("enc")),
        ("alg", json!("RS512")),
        ("key_ops", json!(["encrypt"])),
        ("kid", json!("other")),
    ] {
        let mut changed = serde_json::to_value(&keys).unwrap();
        changed["keys"][0][name] = value;
        assert!(check(&token, &serde_json::from_value(changed).unwrap()).is_err());
    }
    header.crit = Some(vec!["unrecognized".into()]);
    assert!(check(&encode(&header, &claims, &key).unwrap(), &keys).is_err());
    header.crit = None;
    header.alg = Algorithm::RS512;
    assert!(check(&encode(&header, &claims, &key).unwrap(), &keys).is_err());
    let hmac = EncodingKey::from_secret(
        ring::digest::digest(&ring::digest::SHA256, token.as_bytes()).as_ref(),
    )
    .unwrap();
    assert!(
        check(
            &encode(&Header::new(Algorithm::HS256), &claims, &hmac).unwrap(),
            &keys
        )
        .is_err()
    );
    let parts: Vec<_> = token.split('.').collect();
    assert!(check(&format!("{}.{}.AA", parts[0], parts[1]), &keys).is_err());
    assert!(check("not.a.jwt", &keys).is_err());
}
