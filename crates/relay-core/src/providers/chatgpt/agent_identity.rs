use base64::{engine::general_purpose, Engine as _};
use chrono::{SecondsFormat, TimeZone, Utc};
use crypto_box::SecretKey as Curve25519SecretKey;
use ed25519_dalek::{pkcs8::DecodePrivateKey, Signer, SigningKey};
use reqwest::header::HeaderValue;
use serde::Serialize;
use sha2::{Digest, Sha512};
use std::fmt;
use std::time::Duration;

const MAX_PRIVATE_KEY_BYTES: usize = 4 * 1024;
const MAX_IDENTIFIER_BYTES: usize = 512;
const MAX_REGISTRATION_RESPONSE_BYTES: usize = 64 * 1024;
const AGENT_REGISTRATION_TIMEOUT: Duration = Duration::from_secs(15);
const TASK_REGISTRATION_TIMEOUT: Duration = Duration::from_secs(30);
const REGISTRATION_ATTEMPTS: usize = 3;
const TASK_REGISTRATION_BASE_URL: &str = "https://auth.openai.com/api/accounts";

mod registration;

#[cfg(test)]
use registration::{retryable_status, task_registration_url};

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
        let task_id = task_id.map(|task_id_text| task_id_text.trim().to_string());
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
        let authorization_header_value = format!(
            "AgentAssertion {}",
            general_purpose::URL_SAFE_NO_PAD.encode(encoded)
        );
        let mut header = HeaderValue::from_str(&authorization_header_value)
            .map_err(|_| AgentIdentityError::InvalidAuthorization)?;
        header.set_sensitive(true);
        Ok(header)
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

fn encode_ssh_public_key(public_key: &[u8; 32]) -> String {
    let mut blob = Vec::with_capacity(51);
    append_ssh_string(&mut blob, b"ssh-ed25519");
    append_ssh_string(&mut blob, public_key);
    format!("ssh-ed25519 {}", general_purpose::STANDARD.encode(blob))
}

fn append_ssh_string(encoded_key: &mut Vec<u8>, key_bytes: &[u8]) {
    encoded_key.extend_from_slice(&(key_bytes.len() as u32).to_be_bytes());
    encoded_key.extend_from_slice(key_bytes);
}

fn parse_key(encoded_private_key: &str) -> Result<SigningKey, AgentIdentityError> {
    if encoded_private_key.is_empty() || encoded_private_key.len() > MAX_PRIVATE_KEY_BYTES {
        return Err(AgentIdentityError::InvalidPrivateKey);
    }
    let bytes = general_purpose::STANDARD
        .decode(encoded_private_key)
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

fn validate_identifier(identifier: &str) -> Result<(), ()> {
    if identifier.is_empty()
        || identifier.len() > MAX_IDENTIFIER_BYTES
        || identifier.bytes().any(|byte| byte.is_ascii_control())
    {
        Err(())
    } else {
        Ok(())
    }
}

pub fn is_agent_identity_task_invalid_response(status: u16, response_body: &[u8]) -> bool {
    if status != 401 {
        return false;
    }
    let lower = String::from_utf8_lossy(response_body).to_ascii_lowercase();
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
mod tests;
