use super::*;
use crypto_box::aead::OsRng;
use ring::rand::SystemRandom;
use ring::signature::Ed25519KeyPair;
use serde_json::Value;
use std::io::{Read, Write};
use std::net::TcpListener;

#[test]
fn builds_sub2api_compatible_assertion_without_exposing_secrets() {
    let key = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).unwrap();
    let encoded_key = general_purpose::STANDARD.encode(key.as_ref());
    let credential = AgentIdentityCredential::new(
        encoded_key.clone(),
        "runtime-test".into(),
        "task-test".into(),
    )
    .unwrap();
    let authorization = credential.authorization(1_785_000_000_000).unwrap();
    let encoded = authorization
        .to_str()
        .unwrap()
        .strip_prefix("AgentAssertion ")
        .unwrap();
    let envelope: Value =
        serde_json::from_slice(&general_purpose::URL_SAFE_NO_PAD.decode(encoded).unwrap()).unwrap();

    assert_eq!(envelope["agent_runtime_id"], "runtime-test");
    assert_eq!(envelope["task_id"], "task-test");
    assert_eq!(envelope["timestamp"], "2026-07-25T17:20:00Z");
    assert_eq!(
        general_purpose::STANDARD
            .decode(envelope["signature"].as_str().unwrap())
            .unwrap()
            .len(),
        64
    );
    assert!(!format!("{credential:?}").contains(&encoded_key));
}

#[test]
fn rejects_non_pkcs8_and_missing_identity_parts() {
    assert_eq!(
        AgentIdentityCredential::new("not-a-key".into(), "runtime".into(), "task".into())
            .unwrap_err(),
        AgentIdentityError::InvalidPrivateKey
    );
}

#[test]
fn accepts_go_pkcs8_without_embedded_public_key() {
    let private_key = "MC4CAQAwBQYDK2VwBCIEIAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8g";
    let credential = AgentIdentityCredential::new(
        format!("\n{private_key}\r\n"),
        " runtime-test ".into(),
        " task-test\n".into(),
    )
    .unwrap();
    assert_eq!(credential.runtime_id(), "runtime-test");
    assert_eq!(credential.task_id(), Some("task-test"));
    assert!(credential.authorization(1_785_000_000_000).is_ok());
}

#[test]
fn unregistered_identity_cannot_sign_until_a_task_is_attached() {
    let private_key = "MC4CAQAwBQYDK2VwBCIEIAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8g";
    let credential =
        AgentIdentityCredential::unregistered(private_key.into(), "runtime-test".into()).unwrap();
    assert_eq!(credential.task_id(), None);
    assert_eq!(
        credential.authorization(1_785_000_000_000).unwrap_err(),
        AgentIdentityError::InvalidTaskId
    );
    assert!(credential.with_task_id("task-test".into()).is_ok());
}

#[test]
fn invalid_task_detection_is_exact_to_unauthorized_responses() {
    assert!(is_agent_identity_task_invalid_response(
        401,
        br#"{"error":{"code":"task_expired"}}"#
    ));
    assert!(is_agent_identity_task_invalid_response(
        401,
        b"unknown task id"
    ));
    assert!(!is_agent_identity_task_invalid_response(
        401,
        br#"{"error":{"code":"token_invalidated"}}"#
    ));
    assert!(!is_agent_identity_task_invalid_response(
        403,
        br#"{"error":{"code":"task_expired"}}"#
    ));
}

#[test]
fn registration_urls_encode_runtime_ids_and_retry_only_transient_statuses() {
    assert_eq!(
        task_registration_url("https://auth.openai.com/api/accounts", "runtime/test")
            .unwrap()
            .as_str(),
        "https://auth.openai.com/api/accounts/v1/agent/runtime%2Ftest/task/register"
    );
    assert!(retryable_status(reqwest::StatusCode::TOO_MANY_REQUESTS));
    assert!(retryable_status(reqwest::StatusCode::SERVICE_UNAVAILABLE));
    assert!(!retryable_status(reqwest::StatusCode::UNAUTHORIZED));
}

#[test]
fn generated_public_key_uses_openssh_ed25519_wire_format() {
    let key = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).unwrap();
    let signing = parse_key(&general_purpose::STANDARD.encode(key.as_ref())).unwrap();
    let encoded = encode_ssh_public_key(signing.verifying_key().as_bytes());
    let blob = general_purpose::STANDARD
        .decode(encoded.strip_prefix("ssh-ed25519 ").unwrap())
        .unwrap();

    assert_eq!(&blob[..4], &11_u32.to_be_bytes());
    assert_eq!(&blob[4..15], b"ssh-ed25519");
    assert_eq!(&blob[15..19], &32_u32.to_be_bytes());
    assert_eq!(&blob[19..], signing.verifying_key().as_bytes());
}

#[test]
fn decrypts_encrypted_task_registration_response() {
    let key = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).unwrap();
    let private_key = general_purpose::STANDARD.encode(key.as_ref());
    let signing = parse_key(&private_key).unwrap();
    let encrypted = curve_secret_key(&signing)
        .public_key()
        .seal(&mut OsRng, b"task-encrypted")
        .unwrap();
    let credential =
        AgentIdentityCredential::unregistered(private_key, "runtime-test".into()).unwrap();

    assert_eq!(
        credential
            .decrypt_task_id(&general_purpose::STANDARD.encode(encrypted))
            .unwrap(),
        "task-encrypted"
    );
}

#[tokio::test]
async fn missing_task_is_registered_after_a_transient_status() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        for response in [
            "HTTP/1.1 429 Too Many Requests\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                .to_string(),
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 22\r\nConnection: close\r\n\r\n{\"task_id\":\"task-new\"}"
                .to_string(),
        ] {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 4096];
            let _ = stream.read(&mut request);
            stream.write_all(response.as_bytes()).unwrap();
        }
    });
    let private_key = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).unwrap();
    let credential = AgentIdentityCredential::unregistered(
        general_purpose::STANDARD.encode(private_key.as_ref()),
        "runtime-test".into(),
    )
    .unwrap();

    assert_eq!(
        credential
            .register_task_at(
                &reqwest::Client::builder().no_proxy().build().unwrap(),
                &format!("http://{address}/api/accounts"),
            )
            .await
            .unwrap(),
        "task-new"
    );
    server.join().unwrap();
}

#[tokio::test]
async fn oauth_registration_creates_a_ready_agent_identity() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let mut requests = Vec::new();
        for body in [
            r#"{"agent_runtime_id":"runtime-new"}"#,
            r#"{"task_id":"task-new"}"#,
        ] {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 8192];
            let read = stream.read(&mut request).unwrap();
            requests.push(String::from_utf8_lossy(&request[..read]).to_string());
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).unwrap();
        }
        requests
    });
    let base_url = format!("http://{address}/api/accounts");
    let credential = AgentIdentityCredential::register_from_oauth_at(
        &reqwest::Client::builder().no_proxy().build().unwrap(),
        "oauth-access",
        false,
        "1.1.0",
        &base_url,
    )
    .await
    .unwrap();
    let requests = server.join().unwrap();

    assert_eq!(credential.runtime_id(), "runtime-new");
    assert_eq!(credential.task_id(), Some("task-new"));
    let registration = requests[0].to_ascii_lowercase();
    assert!(registration.contains("post /api/accounts/v1/agent/register "));
    assert!(registration.contains("authorization: bearer oauth-access"));
    assert!(registration.contains(r#""agent_harness_id":"zenith-relay""#));
    assert!(registration.contains(r#""capabilities":["responsesapi"]"#));
    assert!(requests[1].contains("/api/accounts/v1/agent/runtime-new/task/register"));
}
