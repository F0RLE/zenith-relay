use super::{
    curve_secret_key, encode_ssh_public_key, parse_key, sign, validate_identifier,
    AgentIdentityCredential, AgentIdentityError, AGENT_REGISTRATION_TIMEOUT,
    MAX_REGISTRATION_RESPONSE_BYTES, REGISTRATION_ATTEMPTS, TASK_REGISTRATION_BASE_URL,
    TASK_REGISTRATION_TIMEOUT,
};
use base64::{engine::general_purpose, Engine as _};
use chrono::{SecondsFormat, Utc};
use futures_util::StreamExt;
use ring::{rand::SystemRandom, signature::Ed25519KeyPair};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use url::Url;

impl AgentIdentityCredential {
    pub async fn register_task(
        &self,
        client: &reqwest::Client,
    ) -> Result<String, AgentIdentityError> {
        self.register_task_at(client, TASK_REGISTRATION_BASE_URL)
            .await
    }

    pub(super) async fn register_task_at(
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
            let (response, permit) = crate::scheduler::refresh::http::management_http_gate()
                .send(
                    client,
                    client
                        .post(url.clone())
                        .timeout(TASK_REGISTRATION_TIMEOUT)
                        .json(&TaskRegistrationRequest {
                            timestamp,
                            signature,
                        }),
                    crate::scheduler::refresh::http::HttpClass::Auth,
                )
                .await
                .map_err(|_| AgentIdentityError::RegistrationTransport)?;
            if response.status().is_success() {
                let registration_result = decode_task_registration_response(self, response).await;
                drop(permit);
                return registration_result;
            }
            if retryable_status(response.status()) && attempt + 1 < REGISTRATION_ATTEMPTS {
                let delay = crate::transport::retry_after_ms(
                    response.headers(),
                    std::time::SystemTime::now(),
                )
                .map(Duration::from_millis)
                .unwrap_or_default()
                .max(retry_delay(attempt));
                drop(response);
                drop(permit);
                tokio::time::sleep(delay).await;
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

    pub(super) async fn register_from_oauth_at(
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
            let (response, permit) = crate::scheduler::refresh::http::management_http_gate()
                .send(
                    client,
                    builder,
                    crate::scheduler::refresh::http::HttpClass::Auth,
                )
                .await
                .map_err(|_| AgentIdentityError::RegistrationTransport)?;
            if response.status().is_success() {
                let registration_response_body = collect_registration_response(response).await?;
                drop(permit);
                let registration_response: AgentRegistrationResponse =
                    serde_json::from_slice(&registration_response_body)
                        .map_err(|_| AgentIdentityError::InvalidRegistrationResponse)?;
                let registered_runtime_id = registration_response
                    .agent_runtime_id
                    .or(registration_response.agent_runtime_id_camel)
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
            let delay =
                crate::transport::retry_after_ms(response.headers(), std::time::SystemTime::now())
                    .map(Duration::from_millis)
                    .unwrap_or_default()
                    .max(retry_delay(attempt));
            drop(response);
            drop(permit);
            tokio::time::sleep(delay).await;
        }
        let identity = Self::unregistered(
            private_key,
            runtime_id.ok_or(AgentIdentityError::InvalidRegistrationResponse)?,
        )?;
        let task_id = identity.register_task_at(client, base_url).await?;
        identity.with_task_id(task_id)
    }

    pub(super) fn decrypt_task_id(&self, encrypted: &str) -> Result<String, AgentIdentityError> {
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
    let task_registration_response_body = collect_registration_response(response).await?;
    let task_registration_response: TaskRegistrationResponse =
        serde_json::from_slice(&task_registration_response_body)
            .map_err(|_| AgentIdentityError::InvalidRegistrationResponse)?;
    if let Some(task_id) = task_registration_response
        .task_id
        .or(task_registration_response.task_id_camel)
    {
        let task_id = task_id.trim().to_string();
        validate_identifier(&task_id).map_err(|_| AgentIdentityError::InvalidTaskId)?;
        return Ok(task_id);
    }
    let encrypted = task_registration_response
        .encrypted_task_id
        .or(task_registration_response.encrypted_task_id_camel)
        .ok_or(AgentIdentityError::InvalidRegistrationResponse)?;
    credential.decrypt_task_id(&encrypted)
}

fn registration_url(base_url: &str) -> Result<Url, AgentIdentityError> {
    append_path(base_url, &["v1", "agent", "register"])
}

pub(super) fn task_registration_url(
    base_url: &str,
    runtime_id: &str,
) -> Result<Url, AgentIdentityError> {
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

pub(super) fn retryable_status(status: reqwest::StatusCode) -> bool {
    status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
}

fn retry_delay(attempt: usize) -> Duration {
    Duration::from_millis(250_u64 << attempt.min(2))
}

async fn collect_registration_response(
    response: reqwest::Response,
) -> Result<Vec<u8>, AgentIdentityError> {
    let mut registration_response_bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| AgentIdentityError::RegistrationTransport)?;
        if registration_response_bytes
            .len()
            .saturating_add(chunk.len())
            > MAX_REGISTRATION_RESPONSE_BYTES
        {
            return Err(AgentIdentityError::RegistrationResponseTooLarge);
        }
        registration_response_bytes.extend_from_slice(&chunk);
    }
    Ok(registration_response_bytes)
}
