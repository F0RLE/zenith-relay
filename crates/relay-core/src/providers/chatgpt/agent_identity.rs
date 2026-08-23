use base64::{engine::general_purpose, Engine as _};
use chrono::{SecondsFormat, TimeZone, Utc};
use crypto_box::SecretKey as Curve25519SecretKey;
use ed25519_dalek::{pkcs8::DecodePrivateKey, Signer, SigningKey};
use futures_util::StreamExt;
use reqwest::header::HeaderValue;
use ring::{rand::SystemRandom, signature::Ed25519KeyPair};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha512};
use std::fmt;
use std::time::Duration;
use url::Url;

const MAX_PRIVATE_KEY_BYTES: usize = 4 * 1024;
const MAX_IDENTIFIER_BYTES: usize = 512;
const MAX_REGISTRATION_RESPONSE_BYTES: usize = 64 * 1024;
const AGENT_REGISTRATION_TIMEOUT: Duration = Duration::from_secs(15);
const TASK_REGISTRATION_TIMEOUT: Duration = Duration::from_secs(30);
const REGISTRATION_ATTEMPTS: usize = 3;
const TASK_REGISTRATION_BASE_URL: &str = "https://auth.openai.com/api/accounts";

#[derive(Clone)]
pub struct AgentIdentityCredential {
    private_key: String,
    runtime_id: String,
    task_id: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentIdentityError {
    InvalidPrivateKey,
    InvalidRuntimeId,
    InvalidTaskId,
    InvalidTimestamp,
    InvalidAuthorization,
    KeyGeneration,
    RegistrationTransport,
    RegistrationRejected,
    InvalidRegistrationResponse,
    RegistrationResponseTooLarge,
    InvalidEncryptedTask,
}

impl fmt::Display for AgentIdentityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidPrivateKey => "agent identity private key is invalid",
            Self::InvalidRuntimeId => "agent identity runtime id is invalid",
            Self::InvalidTaskId => "agent identity task id is invalid",
            Self::InvalidTimestamp => "agent identity timestamp is invalid",
            Self::InvalidAuthorization => "agent identity authorization is invalid",
            Self::KeyGeneration => "agent identity key generation failed",
            Self::RegistrationTransport => "agent identity task registration failed",
            Self::RegistrationRejected => "agent identity task registration was rejected",
            Self::InvalidRegistrationResponse => {
                "agent identity task registration response is invalid"
            }
            Self::RegistrationResponseTooLarge => {
                "agent identity task registration response is too large"
            }
            Self::InvalidEncryptedTask => "encrypted agent identity task is invalid",
        })
    }
}

impl std::error::Error for AgentIdentityError {}

impl AgentIdentityCredential {
    pub fn new(
        private_key: String,
        runtime_id: String,
        task_id: String,
    ) -> Result<Self, AgentIdentityError> {
        Self::from_parts(private_key, runtime_id, Some(task_id))
    }

    pub fn unregistered(
        private_key: String,
        runtime_id: String,
    ) -> Result<Self, AgentIdentityError> {
        Self::from_parts(private_key, runtime_id, None)
    }

    fn from_parts(
        private_key: String,
        runtime_id: String,
        task_id: Option<String>,
    ) -> Result<Self, AgentIdentityError> {
        let private_key = private_key.trim().to_string();
        let runtime_id = runtime_id.trim().to_string();
        let task_id = task_id.map(|value| value.trim().to_string());
        validate_identifier(&runtime_id).map_err(|_| AgentIdentityError::InvalidRuntimeId)?;
        if let Some(task_id) = task_id.as_deref() {
            validate_identifier(task_id).map_err(|_| AgentIdentityError::InvalidTaskId)?;
        }
        parse_key(&private_key)?;
        Ok(Self {
            private_key,
            runtime_id,
            task_id,
        })
    }

    pub fn private_key(&self) -> &str {
        &self.private_key
    }

    pub fn runtime_id(&self) -> &str {
        &self.runtime_id
    }

    pub fn task_id(&self) -> Option<&str> {
        self.task_id.as_deref()
    }

    pub fn with_task_id(&self, task_id: String) -> Result<Self, AgentIdentityError> {
        Self::new(self.private_key.clone(), self.runtime_id.clone(), task_id)
    }

    pub fn authorization(&self, now_ms: u64) -> Result<HeaderValue, AgentIdentityError> {
        let seconds =
            i64::try_from(now_ms / 1_000).map_err(|_| AgentIdentityError::InvalidTimestamp)?;
        let timestamp = Utc
            .timestamp_opt(seconds, 0)
            .single()
            .ok_or(AgentIdentityError::InvalidTimestamp)?
            .to_rfc3339_opts(SecondsFormat::Secs, true);
        let task_id = self
            .task_id
            .as_deref()
            .ok_or(AgentIdentityError::InvalidTaskId)?;
        let key = parse_key(&self.private_key)?;
        let message = format!("{}:{task_id}:{timestamp}", self.runtime_id);
        let envelope = AgentAssertionEnvelope {
            agent_runtime_id: &self.runtime_id,
            task_id,
            timestamp: &timestamp,
            signature: general_purpose::STANDARD.encode(key.sign(message.as_bytes()).to_bytes()),
        };
        let encoded =
            serde_json::to_vec(&envelope).map_err(|_| AgentIdentityError::InvalidAuthorization)?;
        let value = format!(
            "AgentAssertion {}",
            general_purpose::URL_SAFE_NO_PAD.encode(encoded)
        );
        let mut header =
            HeaderValue::from_str(&value).map_err(|_| AgentIdentityError::InvalidAuthorization)?;
        header.set_sensitive(true);
        Ok(header)
    }

    pub async fn register_task(
        &self,
        client: &reqwest::Client,
    ) -> Result<String, AgentIdentityError> {
        self.register_task_at(client, TASK_REGISTRATION_BASE_URL)
            .await
    }

    async fn register_task_at(
        &self,
        client: &reqwest::Client,
        base_url: &str,
    ) -> Result<String, AgentIdentityError> {
        let url = task_registration_url(base_url, &self.runtime_id)?;
        for attempt in 0..REGISTRATION_ATTEMPTS {
            let timestamp = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
            let signature = sign(
                &self.private_key,
                format!("{}:{timestamp}", self.runtime_id).as_bytes(),
            )?;
            let response = client
                .post(url.clone())
                .timeout(TASK_REGISTRATION_TIMEOUT)
                .json(&TaskRegistrationRequest {
                    timestamp,
                    signature,
                })
                .send()
                .await
                .map_err(|_| AgentIdentityError::RegistrationTransport)?;
            if response.status().is_success() {
                return decode_task_registration_response(self, response).await;
            }
            if retryable_status(response.status()) && attempt + 1 < REGISTRATION_ATTEMPTS {
                tokio::time::sleep(retry_delay(attempt)).await;
                continue;
            }
            return Err(AgentIdentityError::RegistrationRejected);
        }
        Err(AgentIdentityError::RegistrationRejected)
    }

    pub async fn register_from_oauth(
        client: &reqwest::Client,
        access_token: &str,
        is_fedramp_account: bool,
        agent_version: &str,
    ) -> Result<Self, AgentIdentityError> {
        Self::register_from_oauth_at(
            client,
            access_token,
            is_fedramp_account,
            agent_version,
            TASK_REGISTRATION_BASE_URL,
        )
        .await
    }

    async fn register_from_oauth_at(
        client: &reqwest::Client,
        access_token: &str,
        is_fedramp_account: bool,
        agent_version: &str,
        base_url: &str,
    ) -> Result<Self, AgentIdentityError> {
        let key = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new())
            .map_err(|_| AgentIdentityError::KeyGeneration)?;
        let private_key = general_purpose::STANDARD.encode(key.as_ref());
        let signing_key = parse_key(&private_key)?;
        let request = AgentRegistrationRequest {
            abom: AgentBillOfMaterials {
                agent_version,
                agent_harness_id: "zenith-relay",
                running_location: "local",
            },
            agent_public_key: encode_ssh_public_key(signing_key.verifying_key().as_bytes()),
            capabilities: ["responsesapi"],
            ttl: None,
        };
        let url = registration_url(base_url)?;
        let mut runtime_id = None;
        for attempt in 0..REGISTRATION_ATTEMPTS {
            let mut builder = client
                .post(url.clone())
                .timeout(AGENT_REGISTRATION_TIMEOUT)
                .bearer_auth(access_token)
                .json(&request);
            if is_fedramp_account {
                builder = builder.header("X-OpenAI-Fedramp", "true");
            }
            let response = builder
                .send()
                .await
                .map_err(|_| AgentIdentityError::RegistrationTransport)?;
            if response.status().is_success() {
                let body = collect_registration_response(response).await?;
                let response: AgentRegistrationResponse = serde_json::from_slice(&body)
                    .map_err(|_| AgentIdentityError::InvalidRegistrationResponse)?;
                let registered_runtime_id = response
                    .agent_runtime_id
                    .or(response.agent_runtime_id_camel)
                    .ok_or(AgentIdentityError::InvalidRegistrationResponse)?
                    .trim()
                    .to_string();
                validate_identifier(&registered_runtime_id)
                    .map_err(|_| AgentIdentityError::InvalidRuntimeId)?;
                runtime_id = Some(registered_runtime_id);
                break;
            }
            if !retryable_status(response.status()) || attempt + 1 >= REGISTRATION_ATTEMPTS {
                return Err(AgentIdentityError::RegistrationRejected);
            }
            tokio::time::sleep(retry_delay(attempt)).await;
        }
        let identity = Self::unregistered(
            private_key,
            runtime_id.ok_or(AgentIdentityError::InvalidRegistrationResponse)?,
        )?;
        let task_id = identity.register_task_at(client, base_url).await?;
        identity.with_task_id(task_id)
    }

    fn decrypt_task_id(&self, encrypted: &str) -> Result<String, AgentIdentityError> {
        let ciphertext = general_purpose::STANDARD
            .decode(encrypted.trim())
            .map_err(|_| AgentIdentityError::InvalidEncryptedTask)?;
        let key = parse_key(&self.private_key)?;
        let plaintext = curve_secret_key(&key)
            .unseal(&ciphertext)
            .map_err(|_| AgentIdentityError::InvalidEncryptedTask)?;
        let task_id = String::from_utf8(plaintext)
            .map_err(|_| AgentIdentityError::InvalidEncryptedTask)?
            .trim()
            .to_string();
        validate_identifier(&task_id).map_err(|_| AgentIdentityError::InvalidTaskId)?;
        Ok(task_id)
    }
}

impl fmt::Debug for AgentIdentityCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentIdentityCredential")
            .field("private_key", &"[redacted]")
            .field("runtime_id", &"[redacted]")
            .field("task_id", &self.task_id.as_ref().map(|_| "[redacted]"))
            .finish()
    }
}

#[derive(Serialize)]
struct AgentAssertionEnvelope<'a> {
    agent_runtime_id: &'a str,
    task_id: &'a str,
    timestamp: &'a str,
    signature: String,
}

#[derive(Serialize)]
struct TaskRegistrationRequest {
    timestamp: String,
    signature: String,
}

#[derive(Deserialize)]
struct TaskRegistrationResponse {
    #[serde(default)]
    task_id: Option<String>,
    #[serde(default, rename = "taskId")]
    task_id_camel: Option<String>,
    #[serde(default)]
    encrypted_task_id: Option<String>,
    #[serde(default, rename = "encryptedTaskId")]
    encrypted_task_id_camel: Option<String>,
}

#[derive(Serialize)]
struct AgentRegistrationRequest<'a> {
    abom: AgentBillOfMaterials<'a>,
    agent_public_key: String,
    capabilities: [&'a str; 1],
    ttl: Option<u64>,
}

#[derive(Serialize)]
struct AgentBillOfMaterials<'a> {
    agent_version: &'a str,
    agent_harness_id: &'a str,
    running_location: &'a str,
}

#[derive(Deserialize)]
struct AgentRegistrationResponse {
    #[serde(default)]
    agent_runtime_id: Option<String>,
    #[serde(default, rename = "agentRuntimeId")]
    agent_runtime_id_camel: Option<String>,
}

async fn decode_task_registration_response(
    credential: &AgentIdentityCredential,
    response: reqwest::Response,
) -> Result<String, AgentIdentityError> {
    let body = collect_registration_response(response).await?;
    let response: TaskRegistrationResponse = serde_json::from_slice(&body)
        .map_err(|_| AgentIdentityError::InvalidRegistrationResponse)?;
    if let Some(task_id) = response.task_id.or(response.task_id_camel) {
        let task_id = task_id.trim().to_string();
        validate_identifier(&task_id).map_err(|_| AgentIdentityError::InvalidTaskId)?;
        return Ok(task_id);
    }
    let encrypted = response
        .encrypted_task_id
        .or(response.encrypted_task_id_camel)
        .ok_or(AgentIdentityError::InvalidRegistrationResponse)?;
    credential.decrypt_task_id(&encrypted)
}

fn registration_url(base_url: &str) -> Result<Url, AgentIdentityError> {
    append_path(base_url, &["v1", "agent", "register"])
}

fn task_registration_url(base_url: &str, runtime_id: &str) -> Result<Url, AgentIdentityError> {
    append_path(base_url, &["v1", "agent", runtime_id, "task", "register"])
}

fn append_path(base_url: &str, segments: &[&str]) -> Result<Url, AgentIdentityError> {
    let mut url = Url::parse(base_url).map_err(|_| AgentIdentityError::RegistrationTransport)?;
    url.path_segments_mut()
        .map_err(|_| AgentIdentityError::RegistrationTransport)?
        .pop_if_empty()
        .extend(segments);
    Ok(url)
}

fn encode_ssh_public_key(public_key: &[u8; 32]) -> String {
    let mut blob = Vec::with_capacity(51);
    append_ssh_string(&mut blob, b"ssh-ed25519");
    append_ssh_string(&mut blob, public_key);
    format!("ssh-ed25519 {}", general_purpose::STANDARD.encode(blob))
}

fn append_ssh_string(output: &mut Vec<u8>, value: &[u8]) {
    output.extend_from_slice(&(value.len() as u32).to_be_bytes());
    output.extend_from_slice(value);
}

fn retryable_status(status: reqwest::StatusCode) -> bool {
    status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
}

fn retry_delay(attempt: usize) -> Duration {
    Duration::from_millis(250_u64 << attempt.min(2))
}

fn parse_key(value: &str) -> Result<SigningKey, AgentIdentityError> {
    if value.is_empty() || value.len() > MAX_PRIVATE_KEY_BYTES {
        return Err(AgentIdentityError::InvalidPrivateKey);
    }
    let bytes = general_purpose::STANDARD
        .decode(value)
        .map_err(|_| AgentIdentityError::InvalidPrivateKey)?;
    SigningKey::from_pkcs8_der(&bytes).map_err(|_| AgentIdentityError::InvalidPrivateKey)
}

fn sign(private_key: &str, message: &[u8]) -> Result<String, AgentIdentityError> {
    Ok(general_purpose::STANDARD.encode(parse_key(private_key)?.sign(message).to_bytes()))
}

fn curve_secret_key(signing_key: &SigningKey) -> Curve25519SecretKey {
    let digest = Sha512::digest(signing_key.to_bytes());
    let mut secret = [0_u8; 32];
    secret.copy_from_slice(&digest[..32]);
    secret[0] &= 248;
    secret[31] &= 127;
    secret[31] |= 64;
    Curve25519SecretKey::from(secret)
}

async fn collect_registration_response(
    response: reqwest::Response,
) -> Result<Vec<u8>, AgentIdentityError> {
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| AgentIdentityError::RegistrationTransport)?;
        if body.len().saturating_add(chunk.len()) > MAX_REGISTRATION_RESPONSE_BYTES {
            return Err(AgentIdentityError::RegistrationResponseTooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn validate_identifier(value: &str) -> Result<(), ()> {
    if value.is_empty()
        || value.len() > MAX_IDENTIFIER_BYTES
        || value.bytes().any(|byte| byte.is_ascii_control())
    {
        Err(())
    } else {
        Ok(())
    }
}

pub fn is_agent_identity_task_invalid_response(status: u16, body: &[u8]) -> bool {
    if status != 401 {
        return false;
    }
    let lower = String::from_utf8_lossy(body).to_ascii_lowercase();
    let compact: String = lower
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    [
        r#""code":"invalid_task_id""#,
        r#""code":"task_not_found""#,
        r#""code":"task_expired""#,
        r#""error":"invalid_task_id""#,
    ]
    .iter()
    .any(|marker| compact.contains(marker))
        || [
            "invalid task_id",
            "invalid task id",
            "task_id is invalid",
            "task id is invalid",
            "task not found",
            "task expired",
            "unknown task_id",
            "unknown task id",
        ]
        .iter()
        .any(|marker| lower.contains(marker))
}

#[cfg(test)]
mod tests {
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
            serde_json::from_slice(&general_purpose::URL_SAFE_NO_PAD.decode(encoded).unwrap())
                .unwrap();

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
            AgentIdentityCredential::unregistered(private_key.into(), "runtime-test".into())
                .unwrap();
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
}
