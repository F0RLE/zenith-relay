use super::*;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
    pub server_id: String,
    pub started_at_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeTargetSummary {
    pub kind: String,
    pub connected: bool,
    pub origin: Option<String>,
    pub server_id: Option<String>,
    pub version: Option<String>,
}

/// Validates a server-generated identifier formatted as a fixed prefix plus
/// the 32 hexadecimal characters emitted by `Uuid::simple()`.
pub fn valid_generated_id(value: &str, prefix: &str) -> bool {
    value.strip_prefix(prefix).is_some_and(|suffix| {
        suffix.len() == 32 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

pub const PROFILE_KEY_ROTATION_SCHEMA_VERSION: u16 = 1;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientWireApi {
    Responses,
    ChatCompletions,
    Messages,
    Gemini,
    Images,
}

/// Client protocols accepted by a local gateway key.
/// Image generation uses Chat Completions, so the legacy `images` scope is omitted.
pub fn local_gateway_client_wire_apis() -> Vec<ClientWireApi> {
    vec![
        ClientWireApi::Responses,
        ClientWireApi::Messages,
        ClientWireApi::ChatCompletions,
        ClientWireApi::Gemini,
    ]
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileKeyRotation {
    pub schema_version: u16,
    pub rotation_id: String,
    pub key_id: String,
    pub base_url: String,
    pub secret: String,
}

impl fmt::Debug for ProfileKeyRotation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProfileKeyRotation")
            .field("schema_version", &self.schema_version)
            .field("rotation_id", &self.rotation_id)
            .field("key_id", &self.key_id)
            .field("base_url", &self.base_url)
            .field("secret", &"[redacted]")
            .finish()
    }
}
